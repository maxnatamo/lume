use indexmap::IndexMap;
use lume_span::Interned;
use object::elf;

use crate::elf::{Entry, StringTable, SymbolTable};
use crate::*;

const SECT_INTERP: &str = ".interp";
const SECT_DYNAMIC: &str = ".dynamic";

bitflags::bitflags! {
    #[derive(Hash, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct SegmentType: u32 {
        const PHDR = elf::PT_PHDR;
        const INTERP = elf::PT_INTERP;
        const LOAD = elf::PT_LOAD;
        const DYNAMIC = elf::PT_DYNAMIC;
        const GNU_EH_FRAME = elf::PT_GNU_EH_FRAME;
        const GNU_STACK = elf::PT_GNU_STACK;
        const GNU_RELRO = elf::PT_GNU_RELRO;
    }
}

bitflags::bitflags! {
    #[derive(Hash, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct SegmentFlags: u32 {
        /// Segment is readable.
        const R = elf::PF_R;

        /// Segment is writable.
        const W = elf::PF_W;

        /// Segment is executable.
        const X = elf::PF_X;

        /// Segment is readable, writable.
        const RW = elf::PF_R | elf::PF_W;

        /// Segment is readable, executable.
        const RX = elf::PF_R | elf::PF_X;
    }
}

#[derive(Hash, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramSegmentId(pub usize);

pub const PID_PHDR: ProgramSegmentId = ProgramSegmentId(0);
pub const PID_INTERP: ProgramSegmentId = ProgramSegmentId(1);
pub const PID_TEXT: ProgramSegmentId = ProgramSegmentId(2);
pub const PID_DATA: ProgramSegmentId = ProgramSegmentId(3);
pub const PID_DYNAMIC: ProgramSegmentId = ProgramSegmentId(4);
pub const PID_GNU_EH_FRAME: ProgramSegmentId = ProgramSegmentId(5);
pub const PID_GNU_STACK: ProgramSegmentId = ProgramSegmentId(6);
pub const PID_GNU_RELRO: ProgramSegmentId = ProgramSegmentId(7);

#[derive(Debug, Clone)]
#[derive_where::derive_where(Hash, PartialEq, Eq)]
pub struct ProgramSegmentDefinition {
    pub id: ProgramSegmentId,

    pub segment_type: SegmentType,
    pub segment_flags: SegmentFlags,

    pub alignment: usize,

    #[derive_where(skip)]
    pub sections: Vec<OutputSectionId>,
}

impl ProgramSegmentDefinition {
    /// Determines if a segment should contain a given section, given the
    /// segment type and required permissions.
    fn should_contain_section(&self, section: &OutputSection) -> bool {
        match self.segment_type {
            SegmentType::LOAD => {
                section.flags.contains(SectionFlags::Allocate)
                    && section.flags.contains(SectionFlags::Executable) == self.is_executable()
                    && section.flags.contains(SectionFlags::Writable) == self.is_writable()
            }
            SegmentType::INTERP => *section.name.section == SECT_INTERP,
            SegmentType::DYNAMIC => *section.name.section == SECT_DYNAMIC,
            SegmentType::GNU_RELRO => {
                section.flags.contains(SectionFlags::Readable)
                    && !section.flags.contains(SectionFlags::Writable)
                    && !section.flags.contains(SectionFlags::Executable)
            }
            _ => {
                section.flags.contains(SectionFlags::Executable) == self.is_executable()
                    && section.flags.contains(SectionFlags::Writable) == self.is_writable()
            }
        }
    }
}

pub(crate) const PROGRAM_SEGMENT_DEFS: &[ProgramSegmentDefinition] = &[
    ProgramSegmentDefinition {
        id: PID_PHDR,
        segment_type: SegmentType::PHDR,
        segment_flags: SegmentFlags::R,
        alignment: 0,
        sections: Vec::new(),
    },
    ProgramSegmentDefinition {
        id: PID_INTERP,
        segment_type: SegmentType::INTERP,
        segment_flags: SegmentFlags::R,
        alignment: 0,
        sections: Vec::new(),
    },
    ProgramSegmentDefinition {
        id: PID_TEXT,
        segment_type: SegmentType::LOAD,
        segment_flags: SegmentFlags::RX,
        alignment: 0,
        sections: Vec::new(),
    },
    ProgramSegmentDefinition {
        id: PID_DATA,
        segment_type: SegmentType::LOAD,
        segment_flags: SegmentFlags::RW,
        alignment: 0,
        sections: Vec::new(),
    },
    ProgramSegmentDefinition {
        id: PID_DYNAMIC,
        segment_type: SegmentType::DYNAMIC,
        segment_flags: SegmentFlags::RW,
        alignment: 0,
        sections: Vec::new(),
    },
    ProgramSegmentDefinition {
        id: PID_GNU_EH_FRAME,
        segment_type: SegmentType::GNU_EH_FRAME,
        segment_flags: SegmentFlags::R,
        alignment: 0,
        sections: Vec::new(),
    },
    ProgramSegmentDefinition {
        id: PID_GNU_STACK,
        segment_type: SegmentType::GNU_STACK,
        segment_flags: SegmentFlags::RW,
        alignment: 0,
        sections: Vec::new(),
    },
    ProgramSegmentDefinition {
        id: PID_GNU_RELRO,
        segment_type: SegmentType::GNU_RELRO,
        segment_flags: SegmentFlags::R,
        alignment: 0,
        sections: Vec::new(),
    },
];

impl ProgramSegmentDefinition {
    #[inline]
    pub(crate) fn is_writable(&self) -> bool {
        self.segment_flags.contains(SegmentFlags::W)
    }

    #[inline]
    pub(crate) fn is_executable(&self) -> bool {
        self.segment_flags.contains(SegmentFlags::X)
    }
}

#[derive(Hash, Debug, Clone, PartialEq, Eq)]
pub struct SectionEntryDefinition {
    pub id: OutputSectionId,
    pub name: Interned<String>,

    pub sh_flags: u32,
    pub sh_type: u32,
    pub sh_size: u64,
    pub sh_align: u64,
    pub sh_link: u32,
    pub sh_info: u32,
}

pub struct Layout<'db> {
    pub(crate) ctx: Context<'db, Entry>,

    pub(crate) string_table: StringTable,
    pub(crate) symbol_table: SymbolTable,
    pub(crate) entrypoint: SymbolId,

    pub(crate) segments: Vec<ProgramSegmentDefinition>,
    pub(crate) sections: Vec<SectionEntryDefinition>,

    /// Defines the virtual placements for each entry within the layout.
    pub(crate) virtual_places: IndexMap<Entry, Placement>,
}

impl<'db> Layout<'db> {
    pub fn new(
        ctx: Context<'db, Entry>,
        string_table: StringTable,
        symbol_table: SymbolTable,
        entrypoint: SymbolId,
    ) -> Self {
        Layout {
            ctx,
            string_table,
            symbol_table,
            entrypoint,
            segments: Vec::new(),
            sections: Vec::new(),
            virtual_places: IndexMap::new(),
        }
    }

    #[inline]
    pub fn target(&self) -> TargetTriple {
        self.ctx.target
    }

    #[inline]
    pub fn elf_class(&self) -> u8 {
        if self.target().arch.is_64bit() {
            elf::ELFCLASS64
        } else {
            elf::ELFCLASS32
        }
    }

    #[inline]
    pub fn data_encoding(&self) -> u8 {
        if self.ctx.endian == Endianess::Little {
            elf::ELFDATA2LSB
        } else {
            elf::ELFDATA2MSB
        }
    }

    #[inline]
    pub fn machine(&self) -> u16 {
        match self.target().arch {
            Arch::Arm => elf::EM_ARM,
            Arch::Arm64 => elf::EM_AARCH64,
            Arch::X86 => elf::EM_386,
            Arch::X86_64 => elf::EM_X86_64,
        }
    }

    /// Gets the virtual size of the given entry when loaded into memory.
    pub fn vmsize_of_entry(&self, entry: &Entry) -> u64 {
        self.virtual_places.get(entry).unwrap().size
    }

    /// Gets the virtual address of the given entry when loaded into memory.
    pub fn vmaddr_of_entry(&self, entry: &Entry) -> u64 {
        self.virtual_places.get(entry).unwrap().offset
    }

    /// Gets the virtual address of the symbol with the given ID when loaded
    /// into memory.
    pub fn vmaddr_of_symbol(&self, id: SymbolId) -> u64 {
        let symbol = self.ctx.db.symbol(id).unwrap();

        match symbol.address {
            SymbolAddress::Unknown | SymbolAddress::Undefined => 0,
            SymbolAddress::Absolute(addr) => addr,
            SymbolAddress::Relative(section_offset) => {
                let Some(section_id) = symbol.section else {
                    unreachable!("relative symbols must have parent section");
                };

                self.vmaddr_of_input_section(section_id) + section_offset
            }
        }
    }

    /// Gets the virtual address of an input section with the given ID when
    /// loaded into memory.
    pub fn vmaddr_of_input_section(&self, id: InputSectionId) -> u64 {
        let (output_section, nested_idx) = self.ctx.input_section_of(id);

        let base_vaddr = self.vmaddr_of_entry(&Entry::SectionData(output_section.id));
        let mut section_vaddr = base_vaddr;

        for contained_section_id in output_section.merged_from.iter().take(nested_idx) {
            let contained_section = self.ctx.db.input_section(*contained_section_id);
            section_vaddr += contained_section.data.len() as u64;
        }

        section_vaddr
    }
}

impl Layout<'_> {
    pub(crate) fn declare_layout(&mut self) {
        for segment in PROGRAM_SEGMENT_DEFS {
            let sections = self
                .ctx
                .db
                .output_sections()
                .filter(|section| segment.should_contain_section(section))
                .collect::<Vec<_>>();

            if sections.is_empty() {
                continue;
            }

            let mut segment_definition = segment.clone();

            for section in sections {
                segment_definition.alignment = segment_definition.alignment.max(section.alignment);
                segment_definition.sections.push(section.id);
            }

            self.segments.push(segment_definition);
        }

        self.sections = self.section_details();

        self.ctx.declare_entry(Entry::FileHeader);
        self.ctx.declare_entry(Entry::ProgramTable {
            count: self.segments.len() as u64,
        });

        let output_section_ids = self
            .ctx
            .db
            .output_sections()
            .map(|section| section.id)
            .collect::<Vec<_>>();

        for &section_id in &output_section_ids {
            self.ctx.declare_entry(Entry::SectionData(section_id));
        }

        self.ctx.declare_entry(Entry::SectionTable {
            count: self.sections.len() as u64 + 1,
        });

        self.ctx.declare_entry(Entry::SymbolTable);
        self.ctx.declare_entry(Entry::StringTable(self.string_table.clone()));
    }

    fn section_details(&self) -> Vec<SectionEntryDefinition> {
        let mut entries = Vec::new();

        for segment in &self.segments {
            for &output_section_id in &segment.sections {
                let output_section = self.ctx.db.output_section(output_section_id);

                let sh_flags = output_section
                    .flags
                    .iter()
                    .map(|flag| match flag {
                        SectionFlags::Writable => elf::SHF_WRITE,
                        SectionFlags::Executable => elf::SHF_EXECINSTR,
                        SectionFlags::Allocate => elf::SHF_ALLOC,
                        SectionFlags::Merge => elf::SHF_MERGE,
                        SectionFlags::TLS => elf::SHF_TLS,
                        _ => 0,
                    })
                    .sum();

                let sh_type = match output_section.kind {
                    SectionKind::Unknown
                    | SectionKind::Text
                    | SectionKind::Data
                    | SectionKind::ReadOnlyData
                    | SectionKind::LumeMetadata
                    | SectionKind::LumeAliases => elf::SHT_PROGBITS,
                    SectionKind::ZeroFilled | SectionKind::UninitializedData => elf::SHT_NOBITS,
                    SectionKind::Elf(ty) => ty,
                    SectionKind::StringTable => elf::SHT_STRTAB,
                };

                entries.push(SectionEntryDefinition {
                    id: output_section_id,
                    name: output_section.name.section,
                    sh_flags,
                    sh_type,
                    sh_align: output_section.alignment as u64,
                    sh_info: 0,
                    sh_link: 0,
                    sh_size: output_section.size,
                });
            }
        }

        entries
    }
}
