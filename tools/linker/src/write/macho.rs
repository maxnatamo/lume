use lume_errors::Result;
use object::{NativeEndian as NE, macho};

use crate::layout::Layout;
use crate::write::Writer;
use crate::{AdditionalData, AdditionalHeader, Architecture, SectionId, Target, align_to};

pub(crate) struct FileFormat;

macro_rules! expect_len {
    ($writer:expr,$expected:expr,$fmt:expr) => {
        debug_assert_eq!($writer.len(), $expected, $fmt, $expected, $writer.len());
    };
}

impl crate::layout::FileFormat for FileFormat {
    fn file_header_size(target: Target) -> usize {
        if target.is_64bit() {
            size_of::<macho::MachHeader64<NE>>()
        } else {
            size_of::<macho::MachHeader32<NE>>()
        }
    }

    fn segment_header_size(target: Target) -> usize {
        if target.is_64bit() {
            size_of::<macho::SegmentCommand64<NE>>()
        } else {
            size_of::<macho::SegmentCommand32<NE>>()
        }
    }

    fn section_header_size(target: Target) -> usize {
        if target.is_64bit() {
            size_of::<macho::Section64<NE>>()
        } else {
            size_of::<macho::Section32<NE>>()
        }
    }

    fn string_table(layout: &mut Layout<Self>)
    where
        Self: Sized,
    {
        // First entry is a single space, used as a null string
        layout.string_table.insert(String::from(" "));

        for symbol_name in layout.index.symbols.keys() {
            layout.string_table.insert(symbol_name.to_owned());
        }

        for symbol_name in layout.index.dynamic_symbols.keys() {
            layout.string_table.insert(symbol_name.to_owned());
        }
    }

    fn additional_headers(layout: &Layout<Self>) -> Option<Vec<AdditionalHeader>>
    where
        Self: Sized,
    {
        let mut dylib_size = 0;
        for required_lib in layout.required_libraries() {
            dylib_size += size_of::<macho::DylibCommand<NE>>() as u64;
            dylib_size += required_lib.name.len() as u64 + 1;
            dylib_size = align_to(dylib_size, align_of::<i32>() as u64);
        }

        Some(vec![
            AdditionalHeader {
                name: "symtab",
                size: size_of::<macho::SymtabCommand<NE>>() as u64,
            },
            AdditionalHeader {
                name: "dylib",
                size: dylib_size,
            },
        ])
    }

    fn additional_data(layout: &Layout<Self>) -> Option<Vec<AdditionalData>>
    where
        Self: Sized,
    {
        let mut strsize = 0_u64;
        for symbol_name in &layout.string_table {
            strsize += symbol_name.len() as u64 + 1;
        }

        let nsyms = layout.index.symbols.len() as u64;

        Some(vec![
            AdditionalData {
                name: "strtab",
                size: strsize,
            },
            AdditionalData {
                name: "symtab",
                size: nsyms * size_of::<macho::Nlist64<NE>>() as u64,
            },
        ])
    }
}

pub(super) fn emit_to<W: Writer, F: crate::layout::FileFormat>(writer: &mut W, layout: Layout<F>) -> Result<()> {
    let mut builder = Builder::new(layout);

    builder.layout.apply_relocations();

    builder.write_header(writer)?;
    builder.write_segments(writer)?;

    builder.write_lc_symtab(writer)?;
    builder.write_lc_dylib(writer)?;

    expect_len!(
        writer,
        builder.layout.header_size(),
        "expected header of {} bytes, but got {}"
    );

    let written = writer.len();
    builder.write_section_data(writer)?;

    expect_len!(
        writer,
        written + usize::try_from(builder.layout.total_segment_size()).unwrap(),
        "expected body of {} bytes, but got {}"
    );

    let written = writer.len();
    builder.write_string_table(writer)?;
    builder.write_symbol_table(writer)?;

    expect_len!(
        writer,
        written + builder.layout.additional_data_total_size(),
        "expected additional data of {} bytes, but got {}"
    );

    Ok(())
}

struct Builder<'db, F: crate::layout::FileFormat> {
    target: Target,
    layout: Layout<'db, F>,
}

impl<'db, F: crate::layout::FileFormat> Builder<'db, F> {
    pub fn new(layout: Layout<'db, F>) -> Self {
        Builder {
            target: layout.target,
            layout,
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

    pub fn write_header<W: Writer>(&self, writer: &mut W) -> Result<()> {
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

    pub fn write_segments<W: Writer>(&self, writer: &mut W) -> Result<()> {
        /*
        let reloc_base = self.layout.additional_data_offset("relocs");
        let mut reloc_count = 0_u32;

        let reloc_size = u32::try_from(size_of::<macho::Relocation<NE>>()).unwrap();
         */

        for segment_name in self.layout.segments() {
            let sections_in_segment = self.layout.sections_in_segment(segment_name).collect::<Vec<_>>();
            let section_count = sections_in_segment.len();

            let segment_vmsize = self.layout.vmsize_of_segment(segment_name);
            let segment_file_size = self.layout.size_of_segment(segment_name);

            let lc_size = F::segment_header_size(self.target) + section_count * F::section_header_size(self.target);

            writer.write_u32(macho::LC_SEGMENT_64)?;
            writer.write_u32(u32::try_from(lc_size).unwrap())?;

            let mut segment_name_bytes = segment_name.as_bytes().to_vec();
            segment_name_bytes.resize(16, 0);
            writer.write(&segment_name_bytes)?;

            writer.write_u64(self.layout.vaddr_of_segment(segment_name))?; // vmaddr
            writer.write_u64(segment_vmsize)?; // vmsize

            writer.write_u64(self.layout.offset_of_segment(segment_name))?; // fileoff
            writer.write_u64(segment_file_size)?; // filesize

            writer.write_u32(0x05)?; // maxprot
            writer.write_u32(0x05)?; // initprot

            writer.write_u32(u32::try_from(section_count).unwrap())?; // nsects
            writer.write_u32(0x00)?; // flags

            for section_id in sections_in_segment {
                let section = self.layout.db.merged_section(section_id);
                let alignment = self.layout.alignment_of_section(section_id);

                let mut section_name_bytes = section.name.section.as_bytes().to_vec();
                section_name_bytes.resize(16, 0);
                writer.write(&section_name_bytes)?;
                writer.write(&segment_name_bytes)?;

                writer.write_u64(self.layout.vaddr_of_section(section_id))?; // addr
                writer.write_u64(self.layout.size_of_section(section_id))?; // size
                writer.write_u32(u32::try_from(self.layout.offset_of_section(section_id)).unwrap())?; // offset
                writer.write_u32(alignment.ilog2())?; // align

                /*
                let mut nrelocs = 0_u32;
                for &contained_section_id in &section.merged_from {
                    let contained_section = self.layout.db.section(contained_section_id);
                    nrelocs += u32::try_from(contained_section.relocations.len()).unwrap();
                }

                let reloff = u32::try_from(reloc_base).unwrap() + reloc_count * reloc_size;
                */

                writer.write_u32(0)?; // reloff
                writer.write_u32(0)?; // nreloc
                writer.write_u32(0)?; // flags
                writer.write_u32(0)?; // reserved1
                writer.write_u32(0)?; // reserved2
                writer.write_u32(0)?; // reserved3

                /*
                reloc_count += nrelocs;
                */
            }
        }
        Ok(())
    }

    pub fn write_lc_symtab<W: Writer>(&self, writer: &mut W) -> Result<()> {
        let lc_size = size_of::<macho::SymtabCommand<NE>>();

        let symoff = self.layout.additional_data_offset("symtab");
        let nsyms = self.layout.index.symbols.len();

        let stroff = self.layout.additional_data_offset("strtab");
        let strsize = self.layout.additional_data_size("strtab");

        writer.write_u32(macho::LC_SYMTAB)?;
        writer.write_u32(u32::try_from(lc_size).unwrap())?;

        writer.write_u32(u32::try_from(symoff).unwrap())?;
        writer.write_u32(u32::try_from(nsyms).unwrap())?;

        writer.write_u32(u32::try_from(stroff).unwrap())?;
        writer.write_u32(u32::try_from(strsize).unwrap())?;

        Ok(())
    }

    pub fn write_lc_dylib<W: Writer>(&self, writer: &mut W) -> Result<()> {
        let required_libraries = self.layout.required_library_ids();

        for library_id in required_libraries {
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
        }

        Ok(())
    }

    pub fn write_section_data<W: Writer>(&self, writer: &mut W) -> Result<()> {
        for merged_section in self.layout.db.merged_sections() {
            writer.align_to(merged_section.alignment)?;

            for &contained_section_id in &merged_section.merged_from {
                let contained_section = self.layout.db.section(contained_section_id);
                writer.write(&contained_section.data)?;
            }
        }

        Ok(())
    }

    pub fn write_string_table<W: Writer>(&mut self, writer: &mut W) -> Result<()> {
        let strtab_base = writer.len();

        for symbol_name in self.layout.string_table.clone() {
            let offset = writer.len() - strtab_base;

            writer.write(symbol_name.as_bytes())?;
            writer.write(&[0])?;

            self.layout.add_string_offset(symbol_name, offset);
        }

        Ok(())
    }

    pub fn write_symbol_table<W: Writer>(&self, writer: &mut W) -> Result<()> {
        for symbol_id in self.layout.index.symbols.values().copied() {
            let symbol = self.layout.db.symbol(symbol_id).unwrap();
            let nstrx = *self.layout.string_table_offsets.get(&symbol.name).unwrap();

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

    pub fn load_command_len(&self) -> u32 {
        let lc_segment = u32::try_from(self.layout.segment_count()).unwrap();
        let lc_dylib = u32::try_from(self.layout.required_library_ids().len()).unwrap();
        let lc_symtab = 1_u32;

        lc_segment + lc_dylib + lc_symtab
    }

    pub fn load_command_size(&self) -> u32 {
        u32::try_from(self.layout.header_size() - F::file_header_size(self.target)).unwrap()
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
