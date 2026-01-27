use indexmap::IndexSet;

use crate::macho::*;
use crate::{Context, Target, page_align};

pub struct Layout<'db> {
    pub(crate) ctx: Context<'db, Entry>,

    pub(crate) string_table: StringTable,
    pub(crate) symbol_table: SymbolTable,
    pub(crate) libraries: IndexSet<LibraryId>,
    pub(crate) entrypoint: SymbolId,

    /// Defines the virtual placements for each entry within the layout.
    pub(crate) virtual_places: IndexMap<Entry, Placement>,
}

impl<'db> Layout<'db> {
    pub fn new(
        ctx: Context<'db, Entry>,
        string_table: StringTable,
        symbol_table: SymbolTable,
        libraries: IndexSet<LibraryId>,
        entrypoint: SymbolId,
    ) -> Self {
        Layout {
            ctx,
            string_table,
            symbol_table,
            libraries,
            entrypoint,
            virtual_places: IndexMap::new(),
        }
    }

    #[inline]
    pub fn target(&self) -> Target {
        self.ctx.target
    }

    #[inline]
    pub fn magic_number(&self) -> u32 {
        if self.target().is_64bit() {
            macho::MH_MAGIC_64
        } else {
            macho::MH_MAGIC
        }
    }

    #[inline]
    pub fn cpu_type(&self) -> u32 {
        let cpu_type = if self.target().is_arm() {
            macho::CPU_TYPE_ARM
        } else if self.target().is_x86() {
            macho::CPU_TYPE_X86
        } else {
            macho::CPU_TYPE_ANY
        };

        if self.target().is_64bit() {
            cpu_type | macho::CPU_ARCH_ABI64
        } else {
            cpu_type | macho::CPU_ARCH_ABI64_32
        }
    }

    #[inline]
    pub fn cpu_subtype(&self) -> u32 {
        match self.target().arch {
            Architecture::Arm | Architecture::Arm64 => macho::CPU_SUBTYPE_ARM_ALL,
            Architecture::X86 | Architecture::X86_64 => macho::CPU_SUBTYPE_X86_ALL,
        }
    }

    /// Gets the amount of load commands in the Mach-O file.
    pub fn lc_count(&self) -> u32 {
        let count = self
            .ctx
            .iter_entries()
            .filter_map(|(entry, _meta)| entry.is_load_command().then_some(entry))
            .count();

        u32::try_from(count).unwrap()
    }

    /// Gets the size of load commands in the Mach-O file, in bytes.
    pub fn lc_size(&self) -> u32 {
        let size = self
            .ctx
            .iter_entries()
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

    /// Gets the physical size of the entire Mach-O file metadata within the
    /// file.
    pub fn size_of_metadata(&self) -> u64 {
        let first_data_entry = self.ctx.entries().keys().find(|entry| entry.is_section_data());

        self.ctx
            .offset_of_entry(first_data_entry.expect("at least one data section"))
    }

    /// Gets the physical size of the given segment, aligned to the required
    /// alignment.
    pub fn aligned_segment_size(&self, segment_name: &str) -> u64 {
        let mut total_section_size = 0;

        // The `__TEXT` segment needs to hold the entire Mach-O file metadata within it
        // to be mapped correctly.
        if segment_name == macho::SEG_TEXT {
            total_section_size += self.size_of_metadata();
        }

        for section_id in self.ctx.db.sections_in_segment(segment_name).collect::<Vec<_>>() {
            total_section_size += self.ctx.db.size_of_section(section_id);
        }

        page_align(total_section_size)
    }

    /// Gets the virtual size of the given entry when loaded into memory.
    pub fn vmsize_of_entry(&self, entry: &Entry) -> u64 {
        self.virtual_places.get(entry).unwrap().size
    }

    /// Gets the virtual address of the given entry when loaded into memory.
    pub fn vmaddr_of_entry(&self, entry: &Entry) -> u64 {
        self.virtual_places.get(entry).unwrap().offset
    }

    /// Gets the virtual address of the given section's data when loaded into
    /// memory.
    pub fn vmaddr_of_section_data(&self, segment: &SegmentContent, section: OutputSectionId) -> u64 {
        let seg_vmaddr = self.vmaddr_of_entry(&Entry::SegmentHeader(segment.clone()));

        // Since the `__TEXT` segment also holds the entire Mach-O file metadata within
        // it, we need to account for the size of the metadata when getting the offset
        // of section data.
        let mut section_offset = if segment.is_text() { self.size_of_metadata() } else { 0 };

        for &section_id in &segment.sections {
            if section_id == section {
                break;
            }

            section_offset += self.ctx.size_of_entry(&Entry::SectionData(section_id));
        }

        seg_vmaddr + section_offset
    }

    /// Gets the file offset of the symbol with the given ID.
    ///
    /// If the symbol with the given ID has no parent section, this method
    /// returns [`None`].
    pub fn offset_of_symbol(&self, id: SymbolId) -> Option<u64> {
        let symbol = self.ctx.db.symbol(id).unwrap();
        let section_id = symbol.section?;

        let SymbolAddress::Relative(relative_address) = symbol.address else {
            return None;
        };

        let (merged_section, nested_idx) = self.ctx.input_section_of(section_id);
        let mut parent_section_offset = self.ctx.offset_of_entry(&Entry::SectionData(merged_section.id));

        for contained_section_id in merged_section.merged_from.iter().take(nested_idx) {
            let contained_section = self.ctx.db.input_section(*contained_section_id);
            parent_section_offset += contained_section.data.len() as u64;
        }

        Some(parent_section_offset + relative_address)
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

                self.vmaddr_of_unmerged_section(section_id) + section_offset
            }
        }
    }

    /// Gets the virtual address of the unmerged section with the given ID when
    /// loaded into memory.
    pub fn vmaddr_of_unmerged_section(&self, id: InputSectionId) -> u64 {
        let (merged_section, nested_idx) = self.ctx.input_section_of(id);
        let segment_name = merged_section.name.segment.clone().unwrap().intern();

        let (segment_entry, _metadata) = self
            .ctx
            .entries()
            .get_key_value(&Entry::SegmentHeader(SegmentContent::new(segment_name)))
            .unwrap();

        let Entry::SegmentHeader(segment_content) = segment_entry else {
            unreachable!()
        };

        let merged_vaddr = self.vmaddr_of_section_data(segment_content, merged_section.id);
        let mut section_vaddr = merged_vaddr;

        for contained_section_id in merged_section.merged_from.iter().take(nested_idx) {
            let contained_section = self.ctx.db.input_section(*contained_section_id);
            section_vaddr += contained_section.data.len() as u64;
        }

        section_vaddr
    }
}

impl Layout<'_> {
    pub(crate) fn declare_layout(&mut self) {
        self.ctx.declare_entry(Entry::FileHeader);
        self.ctx.declare_entry(Entry::PageZero);

        for segment_name in self.ctx.segments().collect::<Vec<_>>() {
            let mut data_size = 0;
            let sections = self.ctx.db.sections_in_segment(&segment_name).collect::<Vec<_>>();

            for &section_id in &sections {
                data_size += self.ctx.db.size_of_section(section_id);
            }

            let total_size = page_align(data_size);

            self.ctx.declare_entry(Entry::SegmentHeader(SegmentContent {
                name: segment_name,
                sections,
                data_size,
                total_size,
            }));
        }

        self.ctx.declare_entry(Entry::LinkEdit);
        self.ctx.declare_entry(Entry::SymtabHeader);
        self.ctx.declare_entry(Entry::DysymtabHeader);
        self.ctx.declare_entry(Entry::LoadDylinker);
        self.ctx.declare_entry(Entry::Uuid);
        self.ctx.declare_entry(Entry::BuildVersion);
        self.ctx.declare_entry(Entry::SourceVersion);

        for library_id in self.libraries.iter().copied() {
            let library_name = self.ctx.db.library(library_id).path.display().to_string();

            self.ctx
                .declare_entry(Entry::DylibHeader(library_id, library_name.intern()));
        }

        self.ctx.declare_entry(Entry::Entrypoint);

        for segment_name in self.ctx.segments().collect::<Vec<_>>() {
            let mut total_section_size = 0;

            for section_id in self.ctx.db.sections_in_segment(&segment_name).collect::<Vec<_>>() {
                self.ctx.declare_entry(Entry::SectionData(section_id));
                total_section_size += self.ctx.db.size_of_section(section_id);
            }

            let aligned_size = page_align(total_section_size);
            let padding_size = aligned_size - total_section_size;

            if padding_size > 0 {
                self.ctx.declare_entry(Entry::Padding(padding_size));
            }
        }

        self.ctx.declare_entry(Entry::StringTable);
        self.ctx.declare_entry(Entry::SymbolTable);

        let linkedit_size = self.ctx.size_of_entry(&Entry::StringTable) + self.ctx.size_of_entry(&Entry::SymbolTable);
        let padding_size = page_align(linkedit_size) - linkedit_size;

        if padding_size > 0 {
            self.ctx.declare_entry(Entry::Padding(padding_size));
        }
    }
}
