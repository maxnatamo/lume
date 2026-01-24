use indexmap::{IndexMap, IndexSet};
use lume_errors::{Result, SimpleDiagnostic};
use lume_span::Interned;
use object::{NativeEndian as NE, macho};

use crate::common::*;
use crate::macho::{DYLINKER_NAME, Entry};
use crate::write::Writer;
use crate::{DEFAULT_ENTRY, Layout, LayoutBuilder, align_to};

pub(crate) fn declare_layout(builder: &mut LayoutBuilder<Entry>) {
    builder.declare_entry(Entry::FileHeader);
    builder.declare_entry(Entry::PageZero);

    let segments: Vec<_> = builder.segments().collect();

    for segment_name in segments {
        builder.declare_entry(Entry::SegmentHeader {
            segment_name,
            sections: builder.db.sections_in_segment(&segment_name).collect(),
        });
    }

    builder.declare_entry(Entry::LinkEdit);
    builder.declare_entry(Entry::SymbolTableHeader);
    builder.declare_entry(Entry::DynamicSymbolTableHeader);
    builder.declare_entry(Entry::LoadDylinker);

    for library_id in builder.required_library_ids() {
        builder.declare_entry(Entry::DylibHeader(library_id));
    }

    builder.declare_entry(Entry::Entrypoint);

    let section_ids: Vec<_> = builder.db.output_sections().map(|sec| sec.id).collect();

    for section_id in section_ids.iter().copied() {
        builder.declare_entry(Entry::SectionData(section_id));
    }

    builder.declare_entry(Entry::StringTable);
    builder.declare_entry(Entry::SymbolTable);
}

pub(crate) fn emit_layout<W: Writer>(writer: &mut W, layout: Layout<Entry>) -> Result<()> {
    let mut builder = Builder::new(layout);

    builder.virtual_places = layout_virtual_places(&builder, |entry| match &entry {
        Entry::PageZero => {
            if builder.target.is_64bit() {
                super::PAGE_ZERO_SIZE_64
            } else {
                super::PAGE_ZERO_SIZE_32
            }
        }

        // The `__LINKEDIT` segment currently contains the string table and the symbol table.
        Entry::LinkEdit => {
            let vbase = builder.layout.offset_of_entry(&Entry::StringTable);

            let symtab_offset = builder.layout.offset_of_entry(&Entry::SymbolTable);
            let symtab_size = builder.layout.size_of_entry(&Entry::SymbolTable);
            let vend = symtab_offset + symtab_size;

            vend - vbase
        }

        Entry::SegmentHeader { segment_name, sections } => {
            match segment_name.as_str() {
                // The __TEXT segment needs to hold the entire MachO file metadata within it to
                // be mapped correctly.
                //
                // Since sections are ordered by address, we get the offset of the first byte in
                // the first section, which gets us the size of all MachO file metadata within
                // the file.
                macho::SEG_TEXT => {
                    let first_section_id = builder.layout.db.output_sections().next().unwrap().id;

                    builder.layout.offset_of_entry(&Entry::SectionData(first_section_id))
                }

                _ => sections
                    .iter()
                    .map(|&section_id| builder.layout.size_of_entry(&Entry::SectionData(section_id)))
                    .sum::<u64>(),
            }
        }
        _ => 0,
    });

    for (entry, metadata) in builder.layout.clone_entries() {
        let alignment = builder.layout.alignment_of_entry(&entry);
        writer.align_to(usize::try_from(alignment).unwrap())?;

        let current_length = writer.len();

        let entry_offset = builder.layout.offset_of_entry(&entry);
        assert_eq!(entry_offset, current_length as u64);

        match &entry {
            Entry::FileHeader => builder.write_file_header(writer)?,
            Entry::PageZero => builder.write_page_zero(writer)?,
            Entry::SegmentHeader { segment_name, .. } => builder.write_segment_header(&entry, *segment_name, writer)?,
            Entry::LinkEdit => builder.write_linkedit(writer)?,
            Entry::SymbolTableHeader => builder.write_symtab_header(writer)?,
            Entry::DynamicSymbolTableHeader => builder.write_dysymtab_header(writer)?,
            Entry::DylibHeader(lib_id) => builder.write_dylib_header(*lib_id, writer)?,
            Entry::SectionData(section_id) => builder.write_section_data(*section_id, writer)?,
            Entry::StringTable => builder.write_string_table(writer)?,
            Entry::SymbolTable => builder.write_symbol_table(writer)?,
            Entry::Entrypoint => builder.write_entrypoint(writer)?,
            Entry::LoadDylinker => builder.write_dylinker(writer)?,
        }

        let written_bytes = writer.len() - current_length;
        assert!(
            metadata.physical_size == written_bytes as u64,
            "expected entry to be {} bytes, but wrote {} bytes: {entry:?}",
            metadata.physical_size,
            written_bytes
        );
    }

    Ok(())
}

/// Calculates the virtual placement of all entries within the builder.
///
/// For each entry, the given closure is invoked to return the virtual size of
/// the entry. The virtual address of all subsequent entries is set to the sum
/// of all previous entry sizes.
fn layout_virtual_places<F: Fn(&Entry) -> u64>(builder: &Builder, f: F) -> IndexMap<Entry, Placement> {
    let mut vmaddr = 0;
    let mut entries = IndexMap::with_capacity(builder.layout.entries.len());

    for (entry, _metadata) in builder.layout.clone_entries() {
        let vmsize = f(&entry);

        entries.insert(entry, Placement {
            offset: vmaddr,
            size: vmsize,
        });

        vmaddr += vmsize;
    }

    entries
}

/// Sorts the given iterator of symbols, depending on their linkage.
///
/// The symbol table in Mach-O expects the symbols to appear in a certain order:
/// - local debug symbols,
/// - private symbols,
/// - external symbols,
/// - undefined symbols
fn sort_symbols<I>(builder: &Builder, symbols: I) -> Vec<SymbolId>
where
    I: Iterator<Item = SymbolId>,
{
    let mut sorted_symbols: Vec<_> = symbols.collect();

    sorted_symbols.sort_by_key(|&sym_id| {
        let linkage = builder.layout.db.symbol(sym_id).unwrap().linkage;

        match linkage {
            Linkage::Local => 0,
            Linkage::Global => 1,
            Linkage::External => 2,
        }
    });

    sorted_symbols
}

struct Builder<'db> {
    target: Target,
    layout: Layout<'db, Entry>,

    /// Defines the virtual placements for each entry within the layout.
    virtual_places: IndexMap<Entry, Placement>,

    string_table: IndexMap<String, usize>,
}

impl<'db> Builder<'db> {
    pub fn new(layout: Layout<'db, Entry>) -> Self {
        Builder {
            target: layout.target,
            layout,
            virtual_places: IndexMap::new(),
            string_table: IndexMap::new(),
        }
    }

    #[inline]
    fn magic_number(&self) -> u32 {
        if self.target.is_64bit() {
            macho::MH_MAGIC_64
        } else {
            macho::MH_MAGIC
        }
    }

    #[inline]
    fn cpu_type(&self) -> u32 {
        let cpu_type = if self.target.is_arm() {
            macho::CPU_TYPE_ARM
        } else if self.target.is_x86() {
            macho::CPU_TYPE_X86
        } else {
            macho::CPU_TYPE_ANY
        };

        if self.target.is_64bit() {
            cpu_type | macho::CPU_ARCH_ABI64
        } else {
            cpu_type | macho::CPU_ARCH_ABI64_32
        }
    }

    #[inline]
    fn cpu_subtype(&self) -> u32 {
        match self.target.arch {
            Architecture::Arm | Architecture::Arm64 => macho::CPU_SUBTYPE_ARM_ALL,
            Architecture::X86 | Architecture::X86_64 => macho::CPU_SUBTYPE_X86_ALL,
        }
    }

    /// Gets the size of a segment load command, in bytes.
    #[inline]
    fn segment_hdr_size(&self) -> u64 {
        if self.target.is_64bit() {
            size_of::<macho::SegmentCommand64<NE>>() as u64
        } else {
            size_of::<macho::SegmentCommand32<NE>>() as u64
        }
    }

    /// Gets the size of a section header, in bytes.
    #[inline]
    fn section_hdr_size(&self) -> u64 {
        if self.target.is_64bit() {
            size_of::<macho::Section64<NE>>() as u64
        } else {
            size_of::<macho::Section32<NE>>() as u64
        }
    }

    /// Gets the amount of load commands in the Mach-O file.
    pub fn lc_count(&self) -> u32 {
        let count = self
            .layout
            .entries()
            .filter_map(|(entry, _meta)| entry.is_load_command().then_some(entry))
            .count();

        u32::try_from(count).unwrap()
    }

    /// Gets the size of load commands in the Mach-O file, in bytes.
    pub fn lc_size(&self) -> u32 {
        let size = self
            .layout
            .entries()
            .filter_map(|(entry, meta)| {
                // Note: should also include the size of child headers as well, such as
                //       section headers which are children of segment headers.

                if entry.is_load_command() {
                    Some(meta.physical_size)
                } else {
                    None
                }
            })
            .sum::<u64>();

        u32::try_from(size).unwrap()
    }

    /// Gets the virtual size of the given entry when loaded into memory.
    pub(crate) fn vmsize_of_entry(&self, entry: &Entry) -> u64 {
        self.virtual_places.get(entry).unwrap().size
    }

    /// Gets the virtual offset of the given entry when loaded into memory.
    pub(crate) fn vmaddr_of_entry(&self, entry: &Entry) -> u64 {
        self.virtual_places.get(entry).unwrap().offset
    }

    pub fn write_file_header<W: Writer>(&self, writer: &mut W) -> Result<()> {
        writer.write_u32(self.magic_number())?;
        writer.write_u32(self.cpu_type())?;
        writer.write_u32(self.cpu_subtype())?;

        writer.write_u32(macho::MH_EXECUTE)?;

        writer.write_u32(self.lc_count())?;
        writer.write_u32(self.lc_size())?;

        let flags = macho::MH_DYLDLINK | macho::MH_PIE | macho::MH_NOUNDEFS;
        writer.write_u32(flags)?;

        if self.target.is_64bit() {
            writer.write_u32(0)?; // reserved (64-bit only)
        }

        Ok(())
    }

    pub fn write_page_zero<W: Writer>(&self, writer: &mut W) -> Result<()> {
        let lc_size = self.layout.size_of_entry(&Entry::PageZero);
        let vmsize = self.vmsize_of_entry(&Entry::PageZero);

        writer.write_u32(macho::LC_SEGMENT_64)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        let mut segment_name_bytes = macho::SEG_PAGEZERO.as_bytes().to_vec();
        segment_name_bytes.resize(16, 0);
        writer.write(&segment_name_bytes)?;

        writer.write_u64(0x0000_0000)?; // vmaddr
        writer.write_u64(vmsize)?; // vmsize

        writer.write_u64(0)?; // fileoff
        writer.write_u64(0)?; // filesize

        writer.write_u32(0)?; // maxprot
        writer.write_u32(0)?; // initprot

        writer.write_u32(0)?; // nsects
        writer.write_u32(0x00)?; // flags

        Ok(())
    }

    pub fn write_segment_header<W: Writer>(
        &self,
        entry: &Entry,
        segment_name: Interned<String>,
        writer: &mut W,
    ) -> Result<()> {
        let segment_str: &str = segment_name.as_ref();
        let sections = self.layout.db.sections_in_segment(segment_str).collect::<Vec<_>>();

        // Add the size of the segment header itself along with all section
        // headers within it.
        let section_header_size = sections.len() as u64 * self.section_hdr_size();
        let lc_size = self.segment_hdr_size() + section_header_size;

        let seg_vmaddr = self.vmaddr_of_entry(entry);
        let seg_vmsize = self.vmsize_of_entry(entry);

        let (fileoff, filesize) = if segment_str == macho::SEG_TEXT {
            let first_section_id = self.layout.db.output_sections().next().unwrap().id;
            let metadata_size = self.layout.offset_of_entry(&Entry::SectionData(first_section_id));

            (0, metadata_size)
        } else {
            let fileoff = sections
                .first()
                .map_or(0, |&section| self.layout.offset_of_entry(&Entry::SectionData(section)));

            let filesize = sections
                .iter()
                .map(|&section_id| self.layout.size_of_entry(&Entry::SectionData(section_id)))
                .sum::<u64>();

            (fileoff, filesize)
        };

        writer.write_u32(macho::LC_SEGMENT_64)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        let mut segment_name_bytes = segment_name.as_bytes().to_vec();
        segment_name_bytes.resize(16, 0);
        writer.write(&segment_name_bytes)?;

        writer.write_u64(seg_vmaddr)?;
        writer.write_u64(seg_vmsize)?;

        writer.write_u64(fileoff)?;
        writer.write_u64(filesize)?;

        let section_prot = match segment_str {
            macho::SEG_TEXT => macho::VM_PROT_READ | macho::VM_PROT_EXECUTE,
            macho::SEG_DATA => macho::VM_PROT_READ | macho::VM_PROT_WRITE,
            _ => macho::VM_PROT_READ,
        };

        writer.write_u32(section_prot)?; // maxprot
        writer.write_u32(section_prot)?; // initprot

        writer.write_u32(u32::try_from(sections.len()).unwrap())?; // nsects
        writer.write_u32(0x00)?; // flags

        let mut section_offset = 0;

        for section_id in sections {
            let data_entry = Entry::SectionData(section_id);
            let section = self.layout.db.output_section(section_id);

            let mut section_name_bytes = section.name.section.as_bytes().to_vec();
            section_name_bytes.resize(16, 0);
            writer.write(&section_name_bytes)?;

            writer.write(&segment_name_bytes)?;

            let sec_vmaddr = seg_vmaddr + section_offset;
            let sec_vmsize = self.layout.size_of_entry(&data_entry);
            let offset = self.layout.offset_of_entry(&data_entry);

            writer.write_u64(sec_vmaddr)?; // addr
            writer.write_u64(sec_vmsize)?; // size

            writer.write_u32(u32::try_from(offset).unwrap())?; // offset
            writer.write_u32(section.alignment.ilog2())?; // align

            writer.write_u32(0)?; // reloff
            writer.write_u32(0)?; // nreloc

            let flags = match section.kind {
                SectionKind::Unknown | SectionKind::Data => macho::S_REGULAR,
                SectionKind::Text => macho::S_ATTR_SOME_INSTRUCTIONS | macho::S_ATTR_PURE_INSTRUCTIONS,
                SectionKind::ZeroFilled => macho::S_ZEROFILL,
                SectionKind::CStrings => macho::S_CSTRING_LITERALS,
                SectionKind::LumeMetadata => macho::S_ATTR_NO_DEAD_STRIP,
                SectionKind::LumeAliases => macho::S_LITERAL_POINTERS,
            };

            writer.write_u32(flags)?;

            writer.write_u32(0)?; // reserved1
            writer.write_u32(0)?; // reserved2
            writer.write_u32(0)?; // reserved3

            section_offset += sec_vmsize;
        }

        Ok(())
    }

    pub fn write_linkedit<W: Writer>(&self, writer: &mut W) -> Result<()> {
        let lc_size = self.layout.size_of_entry(&Entry::LinkEdit);

        let vmaddr = self.vmaddr_of_entry(&Entry::LinkEdit);
        let vmsize = self.vmsize_of_entry(&Entry::LinkEdit);
        let fileoff = self.layout.offset_of_entry(&Entry::StringTable);

        writer.write_u32(macho::LC_SEGMENT_64)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        let mut segment_name_bytes = macho::SEG_LINKEDIT.as_bytes().to_vec();
        segment_name_bytes.resize(16, 0);
        writer.write(&segment_name_bytes)?;

        writer.write_u64(vmaddr)?; // vmaddr
        writer.write_u64(vmsize)?; // vmsize

        writer.write_u64(fileoff)?; // fileoff
        writer.write_u64(vmsize)?; // filesize

        writer.write_u32(macho::VM_PROT_READ)?; // maxprot
        writer.write_u32(macho::VM_PROT_READ)?; // initprot

        writer.write_u32(0)?; // nsects
        writer.write_u32(0x00)?; // flags

        Ok(())
    }

    pub fn write_symtab_header<W: Writer>(&self, writer: &mut W) -> Result<()> {
        let lc_size = size_of::<macho::SymtabCommand<NE>>();

        let symoff = self.layout.offset_of_entry(&Entry::SymbolTable);
        let nsyms = self.layout.index.symbols.len();

        let stroff = self.layout.offset_of_entry(&Entry::StringTable);
        let strsize = self.layout.size_of_entry(&Entry::StringTable);

        writer.write_u32(macho::LC_SYMTAB)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        writer.write_u32(u32::try_from(symoff).unwrap())?;
        writer.write_u32(u32::try_from(nsyms).unwrap())?;

        writer.write_u32(u32::try_from(stroff).unwrap())?;
        writer.write_u32(u32::try_from(strsize).unwrap())?;

        Ok(())
    }

    pub fn write_dysymtab_header<W: Writer>(&self, writer: &mut W) -> Result<()> {
        let sorted_symbols = sort_symbols(self, self.layout.index.symbols.values().copied());

        let mut local_sym_len = 0_u32;
        let mut ext_sym_len = 0_u32;

        for symbol_id in sorted_symbols.iter().copied() {
            let linkage = self.layout.db.symbol(symbol_id).unwrap().linkage;

            match linkage {
                Linkage::Local | Linkage::Global => {
                    local_sym_len += 1;
                }
                Linkage::External => {
                    ext_sym_len += 1;
                }
            }
        }

        let lc_size = size_of::<macho::DysymtabCommand<NE>>();

        writer.write_u32(macho::LC_DYSYMTAB)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        writer.write_u32(0)?; // ilocalsym
        writer.write_u32(local_sym_len)?; // nlocalsym

        writer.write_u32(local_sym_len)?; // iextdefsym
        writer.write_u32(ext_sym_len)?; // nextdefsym

        writer.write_u32(local_sym_len)?; // iundefsym
        writer.write_u32(0)?; // nundefsym

        writer.write_u32(0)?; // tocoff
        writer.write_u32(0)?; // ntoc

        writer.write_u32(0)?; // modtaboff
        writer.write_u32(0)?; // nmodtab

        writer.write_u32(0)?; // extrefsymoff
        writer.write_u32(0)?; // nextrefsyms

        writer.write_u32(0)?; // indirectsymoff
        writer.write_u32(0)?; // nindirectsyms

        writer.write_u32(0)?; // extreloff
        writer.write_u32(0)?; // nextrel

        writer.write_u32(0)?; // locreloff
        writer.write_u32(0)?; // nlocrel

        Ok(())
    }

    pub fn write_dylib_header<W: Writer>(&self, library_id: LibraryId, writer: &mut W) -> Result<()> {
        let library = self.layout.db.library(library_id);
        let library_path = library.path.display().to_string();

        let name_size = library_path.len() + 1;

        let lc_size = size_of::<macho::DylibCommand<NE>>() + name_size;
        let lc_size = align_to(lc_size as u64, align_of::<u64>() as u64);

        writer.write_u32(macho::LC_LOAD_DYLIB)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        // The library name is placed right after the load command
        writer.write_u32(u32::try_from(size_of::<macho::DylibCommand<NE>>()).unwrap())?; // name
        writer.write_u32(0x0000_0000)?; // timestamp
        writer.write_u32(0x0000_0000)?; // current_version
        writer.write_u32(0x0000_0000)?; // compatibility_version

        writer.write(library_path.as_bytes())?;
        writer.write_u8(0)?;

        // `otool` claims the `dylib` commands must be padded to a multiple of 4 bytes,
        // while `nm` requires padding to a multiple of 8 bytes.
        writer.align_to(align_of::<u64>())?;

        Ok(())
    }

    pub fn write_section_data<W: Writer>(&self, id: OutputSectionId, writer: &mut W) -> Result<()> {
        let section = self.layout.db.output_section(id);

        for &contained_section_id in &section.merged_from {
            let contained_section = self.layout.db.input_section(contained_section_id);
            writer.write(&contained_section.data)?;
        }

        Ok(())
    }

    pub fn write_string_table<W: Writer>(&mut self, writer: &mut W) -> Result<()> {
        let strtab_base = writer.len();

        let string_capacity = 1 + self.layout.index.symbols.len() + self.layout.index.dynamic_symbols.len();
        let mut strings = IndexSet::with_capacity(string_capacity);

        // First entry is a single space, used as a null string
        strings.insert(String::from(" "));

        for symbol_name in self.layout.index.symbols.keys() {
            strings.insert(symbol_name.clone());
        }

        for symbol_name in self.layout.index.dynamic_symbols.keys() {
            strings.insert(symbol_name.clone());
        }

        for symbol_name in strings {
            let offset = writer.len() - strtab_base;

            writer.write(symbol_name.as_bytes())?;
            writer.write(&[0])?;

            self.string_table.insert(symbol_name, offset);
        }

        Ok(())
    }

    pub fn write_symbol_table<W: Writer>(&self, writer: &mut W) -> Result<()> {
        let sorted_symbols = sort_symbols(self, self.layout.index.symbols.values().copied());

        for symbol_id in sorted_symbols {
            let symbol = self.layout.db.symbol(symbol_id).unwrap();
            let nstrx = *self.string_table.get(&symbol.name).unwrap();

            let n_type = match symbol.linkage {
                crate::Linkage::External => macho::N_UNDF | macho::N_EXT,
                crate::Linkage::Global | crate::Linkage::Local => macho::N_SECT,
            };

            let section_idx = symbol.section.and_then(|id| self.section_idx_of(id)).unwrap_or(0);

            let n_desc = match symbol.linkage {
                crate::Linkage::External => macho::REFERENCE_FLAG_UNDEFINED_NON_LAZY,
                crate::Linkage::Global | crate::Linkage::Local => macho::REFERENCE_FLAG_DEFINED,
            };

            writer.write_u32(u32::try_from(nstrx).unwrap())?;
            writer.write_u8(n_type)?;
            writer.write_u8(section_idx)?;
            writer.write_u16(n_desc)?;
            writer.write_u64(symbol.address as u64)?;
        }

        Ok(())
    }

    pub fn write_entrypoint<W: Writer>(&self, writer: &mut W) -> Result<()> {
        let lc_size = size_of::<macho::EntryPointCommand<NE>>();

        writer.write_u32(macho::LC_MAIN)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        let entrypoint = self.layout.config.entry.as_deref().unwrap_or(DEFAULT_ENTRY);
        let Some(entrypoint_id) = self.layout.index.symbol_with_name(entrypoint) else {
            return Err(SimpleDiagnostic::new(format!("could not find symbol {entrypoint}")).into());
        };

        let entryoff = self.layout.offset_of_symbol(entrypoint_id).unwrap();
        let stacksize = self.layout.config.stack_size.unwrap_or(0);

        writer.write_u64(entryoff)?; // entryoff
        writer.write_u64(stacksize)?; // stacksize

        Ok(())
    }

    pub fn write_dylinker<W: Writer>(&self, writer: &mut W) -> Result<()> {
        let cmd_size = size_of::<macho::DylinkerCommand<NE>>() as u64;
        let lc_size = align_to(cmd_size + DYLINKER_NAME.len() as u64 + 1, align_of::<u64>() as u64);

        writer.write_u32(macho::LC_LOAD_DYLINKER)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        // The linker name is placed right after the load command
        writer.write_u32(u32::try_from(cmd_size).unwrap())?; // name

        writer.write(DYLINKER_NAME.as_bytes())?;
        writer.write_u8(0)?;

        writer.align_to(align_of::<u64>())?;

        Ok(())
    }

    pub fn section_idx_of(&self, id: InputSectionId) -> Option<u8> {
        for (idx, merged_section) in self.layout.db.output_sections().enumerate() {
            if merged_section.merged_from.contains(&id) {
                return Some(u8::try_from(idx).unwrap() + 1);
            }
        }

        None
    }
}
