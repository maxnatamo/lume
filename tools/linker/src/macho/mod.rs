use object::{NativeEndian as NE, macho};

use crate::common::*;
use crate::{EntryDisplay, Layout, LayoutBuilder, SizedEntry, align_to};

pub(crate) mod reloc;
pub(crate) mod write;

pub(crate) use write::{declare_layout, emit_layout};

/// Default page zero size for the linker (only used on macOS).
pub const PAGE_ZERO_SIZE: u64 = 0x0000_0001_0000_0000;

#[derive(Hash, Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    /// Header for the file format.
    FileHeader,

    /// Header for a single segment with the given name.
    SegmentHeader(String),

    /// Header for a single section with the given ID.
    SectionHeader(OutputSectionId),

    /// Data for a single section with the given ID.
    SectionData(OutputSectionId),

    /// Table of all symbols in the file
    SymbolTable,

    /// Table of all interned strings in the file
    StringTable,

    /// Load dynamic library of the given ID
    DylibHeader(LibraryId),

    /// Load command for the symbol header
    SymbolTableHeader,

    /// Load command for the entrypoint address
    Entrypoint,
}

impl SizedEntry for Entry {
    fn physical_size(entry: &Self, builder: &LayoutBuilder<Self>) -> u64 {
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
            Entry::DylibHeader(lib_id) => {
                let library = builder.db.library(*lib_id);

                let mut dylib_size = 0;
                dylib_size += size_of::<macho::DylibCommand<NE>>() as u64;
                dylib_size += library.path.display().to_string().len() as u64 + 1;
                dylib_size = align_to(dylib_size, align_of::<u64>() as u64);

                dylib_size
            }
            Entry::SymbolTableHeader => size_of::<macho::SymtabCommand<NE>>() as u64,
            Entry::Entrypoint => size_of::<macho::EntryPointCommand<NE>>() as u64,
        }
    }

    fn virtual_size(entry: &Self, builder: &LayoutBuilder<Self>) -> u64 {
        match entry {
            Entry::FileHeader
            | Entry::SectionHeader(_)
            | Entry::DylibHeader(_)
            | Entry::SymbolTableHeader
            | Entry::Entrypoint => 0,
            Entry::SegmentHeader(segment_name) => {
                if builder.target.has_page_zero() && segment_name == macho::SEG_PAGEZERO {
                    PAGE_ZERO_SIZE
                } else {
                    0
                }
            }
            Entry::SectionData(section_id) => builder.size_of_section(*section_id),
            Entry::SymbolTable | Entry::StringTable => Self::physical_size(entry, builder),
        }
    }

    fn alignment(entry: &Self, builder: &LayoutBuilder<Self>) -> u64 {
        match entry {
            Entry::FileHeader
            | Entry::SegmentHeader(_)
            | Entry::SectionHeader(_)
            | Entry::StringTable
            | Entry::SymbolTable
            | Entry::DylibHeader(_)
            | Entry::SymbolTableHeader
            | Entry::Entrypoint => 1,
            Entry::SectionData(section_id) => builder.db.output_section(*section_id).alignment as u64,
        }
    }
}

impl EntryDisplay for Entry {
    fn fmt(&self, builder: &Layout<Entry>, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
        match self {
            Self::FileHeader => write!(w, "FileHeader"),
            Self::SegmentHeader(segment_name) => write!(w, "SegmentHeader, {segment_name}"),
            Self::SectionHeader(section_id) => {
                write!(w, "SectionHeader, {}", builder.db.output_section(*section_id).name)
            }
            Self::SectionData(section_id) => {
                write!(w, "SectionData, {}", builder.db.output_section(*section_id).name)
            }
            Self::SymbolTable => write!(w, "SymbolTable"),
            Self::StringTable => write!(w, "StringTable"),
            Self::DylibHeader(library_id) => {
                write!(w, "DylibHeader, {}", builder.db.library(*library_id).path.display())
            }
            Self::SymbolTableHeader => write!(w, "SymbolTableHeader"),
            Self::Entrypoint => write!(w, "Entrypoint"),
        }
    }
}

impl Layout<'_, Entry> {
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
        if self.target.has_page_zero() && name == macho::SEG_PAGEZERO {
            return 0;
        }

        self.db
            .sections_in_segment(name)
            .map(|id| self.size_of_entry(&Entry::SectionData(id)))
            .sum()
    }

    /// Gets the virtual size of the section with the given ID, in bytes.
    pub(crate) fn vsize_of_segment(&self, name: &str) -> u64 {
        if self.target.has_page_zero() && name == macho::SEG_PAGEZERO {
            return PAGE_ZERO_SIZE;
        }

        self.db
            .sections_in_segment(name)
            .map(|id| self.vmsize_of_entry(&Entry::SectionData(id)))
            .sum()
    }

    /// Gets the file offset of the symbol with the given ID.
    pub(crate) fn offset_of_symbol(&self, id: SymbolId) -> Option<u64> {
        let symbol = self.db.symbol(id).unwrap();
        let section_id = symbol.section?;

        let (merged_section, nested_idx) = self.input_section_of(section_id);
        let mut parent_section_offset = self.offset_of_entry(&Entry::SectionData(merged_section.id));

        for contained_section_id in merged_section.merged_from.iter().take(nested_idx + 1) {
            let contained_section = self.db.input_section(*contained_section_id);
            parent_section_offset += contained_section.data.len() as u64;
        }

        Some(parent_section_offset + symbol.address as u64)
    }

    /// Gets the virtual address of the symbol with the given ID when loaded
    /// into memory.
    pub(crate) fn vaddr_of_symbol(&self, id: SymbolId) -> u64 {
        let symbol = self.db.symbol(id).unwrap();

        let Some(section_id) = symbol.section else {
            return symbol.address as u64;
        };

        let section_vaddr = self.vaddr_of_unmerged_section(section_id);
        let symbol_addr = symbol.address as u64;

        section_vaddr + symbol_addr
    }

    /// Gets the virtual address of the unmerged section with the given ID when
    /// loaded into memory.
    pub(crate) fn vaddr_of_unmerged_section(&self, id: InputSectionId) -> u64 {
        let (merged_section, nested_idx) = self.input_section_of(id);
        let merged_vaddr = self.vaddr_of_entry(&Entry::SectionData(merged_section.id));

        let mut section_vaddr = merged_vaddr;

        for contained_section_id in merged_section.merged_from.iter().take(nested_idx + 1) {
            let contained_section = self.db.input_section(*contained_section_id);
            section_vaddr += contained_section.data.len() as u64;
        }

        section_vaddr
    }
}
