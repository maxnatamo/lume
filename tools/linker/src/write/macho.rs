use lume_errors::Result;
use object::{NativeEndian as NE, macho};

use crate::layout::{Layout, LayoutBuilder};
use crate::write::Writer;
use crate::*;

/// Name of the page zero segment on macOS.
pub const PAGE_ZERO_NAME: &str = "__PAGEZERO";

/// Default page zero size for the linker (only used on macOS).
pub const PAGE_ZERO_SIZE: u64 = 0x0000_0001_0000_0000;

#[derive(Hash, Debug, Clone, PartialEq, Eq)]
pub(super) enum MachoEntry {
    /// Load dynamic library of the given ID
    DylibHeader(LibraryId),

    /// Load command for the symbol header
    SymbolTableHeader,

    /// Load command for the entrypoint address
    Entrypoint,
}

impl EntryDisplay<MachoEntry> for MachoEntry {
    fn fmt(&self, builder: &Layout<MachoEntry>, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
        match self {
            Self::DylibHeader(library_id) => write!(w, "DylibHeader, {}", builder.db.library(*library_id).name),
            Self::SymbolTableHeader => write!(w, "SymbolTableHeader"),
            Self::Entrypoint => write!(w, "Entrypoint"),
        }
    }
}

impl CustomEntry for MachoEntry {
    fn physical_size(entry: &Entry<Self>, builder: &LayoutBuilder<Self>) -> u64 {
        match entry {
            Entry::FileHeader => {
                if builder.target.is_64bit() {
                    size_of::<macho::MachHeader64<NE>>() as u64
                } else {
                    size_of::<macho::MachHeader32<NE>>() as u64
                }
            }
            Entry::SegmentHeader(_) => {
                if builder.target.is_64bit() {
                    size_of::<macho::SegmentCommand64<NE>>() as u64
                } else {
                    size_of::<macho::SegmentCommand32<NE>>() as u64
                }
            }
            Entry::SectionHeader(_) => {
                if builder.target.is_64bit() {
                    size_of::<macho::Section64<NE>>() as u64
                } else {
                    size_of::<macho::Section32<NE>>() as u64
                }
            }
            Entry::SectionData(section_id) => builder.size_of_section(*section_id),
            Entry::SymbolTable => {
                let nsyms = builder.index.symbols.len() as u64;
                nsyms * size_of::<macho::Nlist64<NE>>() as u64
            }
            Entry::StringTable => {
                // First entry is a single space, used as a null string
                let mut strsize = 2_u64;

                for symbol_name in builder.index.symbols.keys() {
                    strsize += symbol_name.len() as u64 + 1;
                }

                for symbol_name in builder.index.dynamic_symbols.keys() {
                    strsize += symbol_name.len() as u64 + 1;
                }

                strsize
            }
            Entry::Custom(MachoEntry::DylibHeader(lib_id)) => {
                let library = builder.db.library(*lib_id);

                let mut dylib_size = 0;
                dylib_size += size_of::<macho::DylibCommand<NE>>() as u64;
                dylib_size += library.name.len() as u64 + 1;
                dylib_size = align_to(dylib_size, align_of::<i32>() as u64);

                dylib_size
            }
            Entry::Custom(MachoEntry::SymbolTableHeader) => size_of::<macho::SymtabCommand<NE>>() as u64,
            Entry::Custom(MachoEntry::Entrypoint) => size_of::<macho::EntryPointCommand<NE>>() as u64,
        }
    }

    fn virtual_size(entry: &Entry<Self>, builder: &LayoutBuilder<Self>) -> u64 {
        match entry {
            Entry::FileHeader
            | Entry::SectionHeader(_)
            | Entry::Custom(MachoEntry::DylibHeader(_) | MachoEntry::SymbolTableHeader | MachoEntry::Entrypoint) => 0,
            Entry::SegmentHeader(segment_name) => {
                if builder.target.has_page_zero() && segment_name == PAGE_ZERO_NAME {
                    PAGE_ZERO_SIZE
                } else {
                    0
                }
            }
            Entry::SectionData(section_id) => builder.size_of_section(*section_id),
            Entry::SymbolTable | Entry::StringTable => Self::physical_size(entry, builder),
        }
    }
}

pub(super) fn declare_layout(builder: &mut LayoutBuilder<MachoEntry>) {
    builder.declare_entry(Entry::FileHeader);
    builder.declare_entry(Entry::SegmentHeader(String::from(PAGE_ZERO_NAME)));

    let segments: Vec<_> = builder.segments().map(ToOwned::to_owned).collect();
    for segment in segments {
        builder.declare_entry(Entry::SegmentHeader(segment.clone()));

        let sections: Vec<_> = builder.db.sections_in_segment(&segment).collect();
        for section in sections {
            builder.declare_entry(Entry::SectionHeader(section));
        }
    }

    builder.declare_entry(Entry::Custom(MachoEntry::SymbolTableHeader));

    for library_id in builder.required_library_ids() {
        builder.declare_entry(Entry::Custom(MachoEntry::DylibHeader(library_id)));
    }

    builder.declare_entry(Entry::Custom(MachoEntry::Entrypoint));

    let section_ids: Vec<_> = builder.db.merged_sections().map(|sec| sec.id).collect();

    for section_id in section_ids.iter().copied() {
        builder.declare_entry(Entry::SectionData(section_id));
    }

    builder.declare_entry(Entry::StringTable);
    builder.declare_entry(Entry::SymbolTable);
}

pub(super) fn emit_layout<W: Writer>(writer: &mut W, layout: Layout<MachoEntry>) -> Result<()> {
    let mut builder = Builder::new(layout);

    for (entry, metadata) in builder.layout.clone_entries() {
        let alignment = builder.layout.alignment_of_entry(&entry);
        writer.align_to(usize::try_from(alignment).unwrap())?;

        let current_length = writer.len();

        let entry_offset = builder.layout.offset_of_entry(&entry);
        assert_eq!(entry_offset, current_length as u64);

        match &entry {
            Entry::FileHeader => builder.write_file_header(writer)?,
            Entry::SegmentHeader(segment) => builder.write_segment_header(segment, writer)?,
            Entry::SectionHeader(section_id) => builder.write_section_header(*section_id, writer)?,
            Entry::Custom(MachoEntry::SymbolTableHeader) => builder.write_symtab_header(writer)?,
            Entry::Custom(MachoEntry::DylibHeader(lib_id)) => builder.write_dylib_header(*lib_id, writer)?,
            Entry::SectionData(section_id) => builder.write_section_data(*section_id, writer)?,
            Entry::StringTable => builder.write_string_table(writer)?,
            Entry::SymbolTable => builder.write_symbol_table(writer)?,
            Entry::Custom(MachoEntry::Entrypoint) => builder.write_entrypoint(writer)?,
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

struct Builder<'db> {
    target: Target,
    layout: Layout<'db, MachoEntry>,
    string_table: IndexMap<String, usize>,
}

impl<'db> Builder<'db> {
    pub fn new(layout: Layout<'db, MachoEntry>) -> Self {
        Builder {
            target: layout.target,
            layout,
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

    /// Gets the amount of load commands in the Mach-O file.
    pub fn load_command_len(&self) -> u32 {
        fn is_lc_entry(entry: &Entry<MachoEntry>) -> bool {
            matches!(
                entry,
                Entry::SegmentHeader(_)
                    | Entry::Custom(
                        MachoEntry::SymbolTableHeader | MachoEntry::DylibHeader(_) | MachoEntry::Entrypoint
                    )
            )
        }

        let count = self
            .layout
            .entries()
            .filter_map(|(entry, _meta)| is_lc_entry(entry).then_some(entry))
            .count();

        u32::try_from(count).unwrap()
    }

    /// Gets the size of load commands in the Mach-O file, in bytes.
    pub fn load_command_size(&self) -> u32 {
        /// Note: should also include the size of child headers as well, such as
        /// section headers which are children of segment headers.
        fn is_lc_entry(entry: &Entry<MachoEntry>) -> bool {
            matches!(
                entry,
                Entry::SegmentHeader(_)
                    | Entry::SectionHeader(_)
                    | Entry::Custom(
                        MachoEntry::SymbolTableHeader | MachoEntry::DylibHeader(_) | MachoEntry::Entrypoint
                    )
            )
        }

        let size = self
            .layout
            .entries()
            .filter_map(|(entry, meta)| is_lc_entry(entry).then_some(meta.physical_size))
            .sum::<u64>();

        u32::try_from(size).unwrap()
    }

    pub fn write_file_header<W: Writer>(&self, writer: &mut W) -> Result<()> {
        writer.write_u32(self.magic_number())?;
        writer.write_u32(self.cpu_type())?;
        writer.write_u32(self.cpu_subtype())?;

        writer.write_u32(macho::MH_EXECUTE)?;

        writer.write_u32(self.load_command_len())?;
        writer.write_u32(self.load_command_size())?;

        let flags = macho::MH_DYLDLINK | macho::MH_PIE;
        writer.write_u32(flags)?;

        if self.target.is_64bit() {
            writer.write_u32(0)?; // reserved (64-bit only)
        }

        Ok(())
    }

    pub fn write_segment_header<W: Writer>(&self, segment_name: &str, writer: &mut W) -> Result<()> {
        let entry = Entry::SegmentHeader(segment_name.to_owned());

        let vaddr = self.layout.vaddr_of_segment(segment_name);
        let vsize = self.layout.vsize_of_segment(segment_name);

        let fileoff = self.layout.offset_of_entry(&entry);
        let filesize = self.layout.size_of_segment(segment_name);

        let sections = self.layout.db.sections_in_segment(segment_name).collect::<Vec<_>>();

        // Add the size of the segment header itself along with all section
        // headers within it.
        let mut lc_size = self.layout.size_of_entry(&entry);

        for &section in &sections {
            lc_size += self.layout.size_of_entry(&Entry::SectionHeader(section));
        }

        writer.write_u32(macho::LC_SEGMENT_64)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        let mut segment_name_bytes = segment_name.as_bytes().to_vec();
        segment_name_bytes.resize(16, 0);
        writer.write(&segment_name_bytes)?;

        writer.write_u64(vaddr)?;
        writer.write_u64(vsize)?;

        writer.write_u64(fileoff)?;
        writer.write_u64(filesize)?;

        let section_prot = match segment_name {
            macho::SEG_PAGEZERO => 0x0000_0000,
            macho::SEG_TEXT => macho::VM_PROT_READ | macho::VM_PROT_EXECUTE,
            macho::SEG_DATA => macho::VM_PROT_READ | macho::VM_PROT_WRITE,
            _ => macho::VM_PROT_READ,
        };

        writer.write_u32(section_prot)?; // maxprot
        writer.write_u32(section_prot)?; // initprot

        writer.write_u32(u32::try_from(sections.len()).unwrap())?; // nsects
        writer.write_u32(0x00)?; // flags

        Ok(())
    }

    pub fn write_section_header<W: Writer>(&self, section_id: MergedSectionId, writer: &mut W) -> Result<()> {
        let data_entry = Entry::SectionData(section_id);

        let section = self.layout.db.merged_section(section_id);
        let segment_name = section.name.segment.as_deref().unwrap_or("");

        let mut section_name_bytes = section.name.section.as_bytes().to_vec();
        section_name_bytes.resize(16, 0);
        writer.write(&section_name_bytes)?;

        let mut segment_name_bytes = segment_name.as_bytes().to_vec();
        segment_name_bytes.resize(16, 0);
        writer.write(&segment_name_bytes)?;

        let vaddr = self.layout.vaddr_of_entry(&data_entry);
        let vsize = self.layout.size_of_entry(&data_entry);
        let offset = self.layout.offset_of_entry(&data_entry);

        writer.write_u64(vaddr)?; // addr
        writer.write_u64(vsize)?; // size
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

    pub fn write_dylib_header<W: Writer>(&self, library_id: LibraryId, writer: &mut W) -> Result<()> {
        let library = self.layout.db.library(library_id);
        let name_size = library.name.len() + 1;

        let lc_size = size_of::<macho::DylibCommand<NE>>() + name_size;
        let lc_size = align_to(lc_size as u64, align_of::<i32>() as u64);

        writer.write_u32(macho::LC_LOAD_DYLIB)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        // The library name is placed right after the load command
        writer.write_u32(u32::try_from(size_of::<macho::DylibCommand<NE>>()).unwrap())?; // name
        writer.write_u32(0x0000_0000)?; // timestamp
        writer.write_u32(0x0000_0000)?; // current_version
        writer.write_u32(0x0000_0000)?; // compatibility_version

        writer.write(library.name.as_bytes())?;
        writer.write_u8(0)?;

        // `otool` claims the `dylib` commands must be padded to a multiple of 4 bytes
        writer.align_to(align_of::<i32>())?;

        Ok(())
    }

    pub fn write_section_data<W: Writer>(&self, id: MergedSectionId, writer: &mut W) -> Result<()> {
        let section = self.layout.db.merged_section(id);

        for &contained_section_id in &section.merged_from {
            let contained_section = self.layout.db.section(contained_section_id);
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
        for symbol_id in self.layout.index.symbols.values().copied() {
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

        let entrypoint = self.layout.config.entry.as_deref().unwrap_or(layout::DEFAULT_ENTRY);
        let Some(entrypoint_id) = self.layout.index.symbol_with_name(entrypoint) else {
            return Err(SimpleDiagnostic::new(format!("could not find symbol {entrypoint}")).into());
        };

        let entryoff = self.layout.offset_of_symbol(entrypoint_id).unwrap();
        let stacksize = self.layout.config.stack_size.unwrap_or(0);

        writer.write_u64(entryoff)?; // entryoff
        writer.write_u64(stacksize)?; // stacksize

        Ok(())
    }

    pub fn section_idx_of(&self, id: SectionId) -> Option<u8> {
        for (idx, merged_section) in self.layout.db.merged_sections().enumerate() {
            if merged_section.merged_from.contains(&id) {
                return Some(u8::try_from(idx).unwrap() + 1);
            }
        }

        None
    }
}
impl Layout<'_, MachoEntry> {
    /// Gets the virtual address of the segment with the given name, when loaded
    /// into memory.
    pub(crate) fn vaddr_of_segment(&self, name: &str) -> u64 {
        let Some(first_section_id) = self.db.sections_in_segment(name).next() else {
            return 0;
        };

        self.vaddr_of_entry(&Entry::SectionData(first_section_id))
    }

    /// Gets the physical size of the section with the given ID, in bytes.
    pub(crate) fn size_of_segment(&self, name: &str) -> u64 {
        if self.target.has_page_zero() && name == PAGE_ZERO_NAME {
            return 0;
        }

        self.db
            .sections_in_segment(name)
            .map(|id| self.size_of_entry(&Entry::SectionData(id)))
            .sum()
    }

    /// Gets the virtual size of the section with the given ID, in bytes.
    pub(crate) fn vsize_of_segment(&self, name: &str) -> u64 {
        if self.target.has_page_zero() && name == PAGE_ZERO_NAME {
            return PAGE_ZERO_SIZE;
        }

        self.db
            .sections_in_segment(name)
            .map(|id| self.vmsize_of_entry(&Entry::SectionData(id)))
            .sum()
    }
}
