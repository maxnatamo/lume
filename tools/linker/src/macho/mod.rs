use indexmap::IndexMap;
use lume_span::{Internable, Interned};
use object::{NativeEndian as NE, macho};

use crate::common::*;
use crate::{EntryDisplay, Layout, LayoutBuilder, SizedEntry, align_to};

pub(crate) mod reloc;
pub(crate) mod write;

pub(crate) use write::{declare_layout, emit_layout};

/// Name of the dynamic linker to use.
const DYLINKER_NAME: &str = "/usr/lib/dyld";

/// Default page zero size for the linker (only used on macOS).
pub const PAGE_ZERO_SIZE_64: u64 = 0x0000_0001_0000_0000;

/// Default page zero size for the linker (only used on macOS).
pub const PAGE_ZERO_SIZE_32: u64 = 0x0000_0000_0000_1000;

#[derive(Debug, Clone)]
#[derive_where::derive_where(Hash, PartialEq, Eq)]
pub(crate) enum Entry {
    /// Header for the file format.
    FileHeader,

    /// Header for the page zero segment (__PAGEZERO).
    PageZero,

    /// Header for a single segment with the given name.
    SegmentHeader {
        segment_name: Interned<String>,

        #[derive_where(skip)]
        sections: Vec<OutputSectionId>,
    },

    /// Header for the `__LINKEDIT` segment.
    LinkEdit,

    /// Load dynamic library of the given ID
    DylibHeader(LibraryId),

    /// Load command for the symbol table
    SymbolTableHeader,

    /// Load command for the dynamic symbol table
    DynamicSymbolTableHeader,

    /// Table of all symbols in the file
    SymbolTable,

    /// Table of all interned strings in the file
    StringTable,

    /// Load command for the entrypoint address
    Entrypoint,

    /// Load command for loading the dynamic linker
    LoadDylinker,

    /// UUID load command
    Uuid,

    /// Build version load command
    BuildVersion,

    /// Source version load command
    SourceVersion,

    /// Data for a single section with the given ID.
    SectionData(OutputSectionId),
}

impl Entry {
    /// Determines if the entry is a load command.
    #[inline]
    pub fn is_load_command(&self) -> bool {
        matches!(
            self,
            Entry::PageZero
                | Entry::SegmentHeader { .. }
                | Entry::LinkEdit
                | Entry::SymbolTableHeader
                | Entry::DynamicSymbolTableHeader
                | Entry::DylibHeader(_)
                | Entry::Entrypoint
                | Entry::LoadDylinker
                | Entry::Uuid
                | Entry::BuildVersion
                | Entry::SourceVersion
        )
    }
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
            Entry::PageZero | Entry::LinkEdit => {
                if builder.target.is_64bit() {
                    size_of::<macho::SegmentCommand64<NE>>() as u64
                } else {
                    size_of::<macho::SegmentCommand32<NE>>() as u64
                }
            }
            Entry::SegmentHeader { sections, .. } => {
                let segment_size = if builder.target.is_64bit() {
                    size_of::<macho::SegmentCommand64<NE>>() as u64
                } else {
                    size_of::<macho::SegmentCommand32<NE>>() as u64
                };

                let section_size = if builder.target.is_64bit() {
                    size_of::<macho::Section64<NE>>() as u64
                } else {
                    size_of::<macho::Section32<NE>>() as u64
                };

                segment_size + section_size * sections.len() as u64
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
            Entry::DynamicSymbolTableHeader => size_of::<macho::DysymtabCommand<NE>>() as u64,
            Entry::Entrypoint => size_of::<macho::EntryPointCommand<NE>>() as u64,
            Entry::LoadDylinker => {
                let mut dylinker_size = size_of::<macho::DylinkerCommand<NE>>() as u64;
                dylinker_size += DYLINKER_NAME.len() as u64 + 1;
                dylinker_size = align_to(dylinker_size, align_of::<u64>() as u64);

                dylinker_size
            }
            Entry::Uuid => size_of::<macho::UuidCommand<NE>>() as u64,
            Entry::BuildVersion => size_of::<macho::BuildVersionCommand<NE>>() as u64,
            Entry::SourceVersion => size_of::<macho::SourceVersionCommand<NE>>() as u64,
        }
    }

    fn alignment(entry: &Self, builder: &LayoutBuilder<Self>) -> u64 {
        match entry {
            Entry::FileHeader
            | Entry::PageZero
            | Entry::SegmentHeader { .. }
            | Entry::DylibHeader(_)
            | Entry::SymbolTableHeader
            | Entry::DynamicSymbolTableHeader
            | Entry::Entrypoint
            | Entry::LoadDylinker
            | Entry::Uuid
            | Entry::BuildVersion
            | Entry::SourceVersion => 1,
            Entry::LinkEdit | Entry::StringTable | Entry::SymbolTable => 4,
            Entry::SectionData(section_id) => builder.db.output_section(*section_id).alignment as u64,
        }
    }
}

impl EntryDisplay for Entry {
    fn fmt(&self, builder: &Layout<Entry>, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
        match self {
            Self::FileHeader => write!(w, "FileHeader"),
            Self::PageZero => write!(w, "PageZero"),
            Self::SegmentHeader { segment_name, .. } => write!(w, "SegmentHeader, {segment_name}"),
            Self::LinkEdit => write!(w, "Linkedit"),
            Self::SectionData(section_id) => {
                write!(w, "SectionData, {}", builder.db.output_section(*section_id).name)
            }
            Self::SymbolTable => write!(w, "SymbolTable"),
            Self::StringTable => write!(w, "StringTable"),
            Self::DylibHeader(library_id) => {
                write!(w, "DylibHeader, {}", builder.db.library(*library_id).path.display())
            }
            Self::SymbolTableHeader => write!(w, "SymbolTableHeader"),
            Self::DynamicSymbolTableHeader => write!(w, "DynamicSymbolTableHeader"),
            Self::Entrypoint => write!(w, "Entrypoint"),
            Self::LoadDylinker => write!(w, "LoadDylinker"),
            Self::Uuid => write!(w, "Uuid"),
            Self::BuildVersion => write!(w, "BuildVersion"),
            Self::SourceVersion => write!(w, "SourceVersion"),
        }
    }
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
    fn vmsize_of_entry(&self, entry: &Entry) -> u64 {
        self.virtual_places.get(entry).unwrap().size
    }

    /// Gets the virtual offset of the given entry when loaded into memory.
    fn vmaddr_of_entry(&self, entry: &Entry) -> u64 {
        self.virtual_places.get(entry).unwrap().offset
    }

    /// Gets the file offset of the symbol with the given ID.
    ///
    /// If the symbol with the given ID has no parent section, this method
    /// returns [`None`].
    fn offset_of_symbol(&self, id: SymbolId) -> Option<u64> {
        let symbol = self.layout.db.symbol(id).unwrap();
        let section_id = symbol.section?;

        let SymbolAddress::Relative(relative_address) = symbol.address else {
            return None;
        };

        let (merged_section, nested_idx) = self.layout.input_section_of(section_id);
        let mut parent_section_offset = self.layout.offset_of_entry(&Entry::SectionData(merged_section.id));

        for contained_section_id in merged_section.merged_from.iter().take(nested_idx) {
            let contained_section = self.layout.db.input_section(*contained_section_id);
            parent_section_offset += contained_section.data.len() as u64;
        }

        Some(parent_section_offset + relative_address)
    }

    /// Gets the virtual address of the symbol with the given ID when loaded
    /// into memory.
    fn vmaddr_of_symbol(&self, id: SymbolId) -> u64 {
        let symbol = self.layout.db.symbol(id).unwrap();

        match symbol.address {
            SymbolAddress::Unknown | SymbolAddress::Undefined => 0,
            SymbolAddress::Absolute(addr) => addr,
            SymbolAddress::Relative(section_offset) => {
                let Some(section_id) = symbol.section else {
                    unreachable!("relative symbols must have parent section");
                };

                self.vmaddr_of_unmerged_section(section_id) + section_offset
            }
        }
    }

    /// Gets the virtual address of the unmerged section with the given ID when
    /// loaded into memory.
    fn vmaddr_of_unmerged_section(&self, id: InputSectionId) -> u64 {
        let (merged_section, nested_idx) = self.layout.input_section_of(id);
        let segment_header_entry = Entry::SegmentHeader {
            segment_name: merged_section.name.segment.clone().unwrap().intern(),
            sections: Vec::new(),
        };

        let merged_vaddr = self.vmaddr_of_entry(&segment_header_entry);
        let mut section_vaddr = merged_vaddr;

        for contained_section_id in merged_section.merged_from.iter().take(nested_idx) {
            let contained_section = self.layout.db.input_section(*contained_section_id);
            section_vaddr += contained_section.data.len() as u64;
        }

        section_vaddr
    }
}
