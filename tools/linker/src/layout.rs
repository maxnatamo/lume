use std::fmt::Debug;
use std::hash::Hash;

use indexmap::{IndexMap, IndexSet};

use crate::common::*;
use crate::{Config, Database, Index, Linker};

/// Default entry point symbol name.
pub const DEFAULT_ENTRY: &str = "_main";

#[derive(Hash, Debug, Clone, PartialEq, Eq)]
pub(super) enum Entry<C: CustomEntry> {
    /// Header for the file format.
    FileHeader,

    /// Header for a single segment with the given name.
    SegmentHeader(String),

    /// Header for a single section with the given ID.
    SectionHeader(MergedSectionId),

    /// Data for a single section with the given ID.
    SectionData(MergedSectionId),

    /// Table of all symbols in the file
    SymbolTable,

    /// Table of all interned strings in the file
    StringTable,

    /// Custom section kind
    Custom(C),
}

pub(crate) trait CustomEntry: Hash + Debug + Clone + PartialEq + Eq {
    /// Gets the physical size of the entry within the file.
    fn physical_size(entry: &Entry<Self>, builder: &LayoutBuilder<Self>) -> u64;

    /// Gets the virtual size of the entry when mapped into memory.
    fn virtual_size(entry: &Entry<Self>, builder: &LayoutBuilder<Self>) -> u64 {
        Self::physical_size(entry, builder)
    }

    /// Gets the requirement alignment of the entry.
    fn alignment(entry: &Entry<Self>, builder: &LayoutBuilder<Self>) -> u64 {
        match entry {
            Entry::FileHeader
            | Entry::SegmentHeader(_)
            | Entry::SectionHeader(_)
            | Entry::StringTable
            | Entry::SymbolTable
            | Entry::Custom(_) => 1,
            Entry::SectionData(section_id) => builder.db.merged_section(*section_id).alignment as u64,
        }
    }
}

pub(crate) trait EntryDisplay<C: CustomEntry> {
    fn fmt(&self, builder: &Layout<C>, w: &mut dyn std::fmt::Write) -> std::fmt::Result;
}

impl<C: CustomEntry> EntryDisplay<C> for Entry<C>
where
    C: EntryDisplay<C>,
{
    fn fmt(&self, builder: &Layout<C>, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
        match self {
            Entry::FileHeader => write!(w, "FileHeader"),
            Entry::SegmentHeader(segment_name) => write!(w, "SegmentHeader, {segment_name}"),
            Entry::SectionHeader(section_id) => {
                write!(w, "SectionHeader, {}", builder.db.merged_section(*section_id).name)
            }
            Entry::SectionData(section_id) => {
                write!(w, "SectionData, {}", builder.db.merged_section(*section_id).name)
            }
            Entry::SymbolTable => write!(w, "SymbolTable"),
            Entry::StringTable => write!(w, "StringTable"),
            Entry::Custom(custom) => EntryDisplay::fmt(custom, builder, w),
        }
    }
}

pub(crate) struct LayoutBuilder<'db, C: CustomEntry> {
    pub(crate) target: Target,
    pub(crate) db: &'db mut Database,
    pub(crate) index: &'db Index,
    pub(crate) config: &'db Config,

    entries: IndexSet<Entry<C>>,
}

impl<'db, C: CustomEntry> LayoutBuilder<'db, C> {
    /// Creates a new layout builder for the given target.
    pub(crate) fn new(linker: &'db mut Linker) -> Self {
        Self {
            target: linker.target,
            db: &mut linker.db,
            index: &linker.index,
            config: &linker.config,
            entries: IndexSet::new(),
        }
    }

    /// Declares a new entry with the given kind.
    pub(crate) fn declare_entry(&mut self, kind: Entry<C>) {
        self.entries.insert(kind);
    }

    /// Gets an iterator over all segment names in the layout.
    pub(crate) fn segments(&self) -> impl Iterator<Item = &str> {
        self.db.merged_segments.keys().map(|s| s.as_str())
    }

    /// Gets a set of all required library IDs.
    pub(crate) fn required_library_ids(&self) -> IndexSet<LibraryId> {
        self.index.dynamic_symbols.values().copied().collect::<IndexSet<_>>()
    }

    /// Gets the physical size of the section with the given ID, in bytes.
    ///
    /// If the end of the section boundary was not on a aligned boundary,
    /// the size will be rounded up to the next aligned boundary.
    pub(crate) fn size_of_section(&self, id: MergedSectionId) -> u64 {
        let merged_section = self.db.merged_section(id);
        if !merged_section.occupies_space() {
            return 0;
        }

        align_to(merged_section.size, merged_section.alignment as u64)
    }

    /// Consumes the builder and creates a new [`Layout`].
    pub(crate) fn into_layout(mut self) -> Layout<'db, C> {
        let mut entries = IndexMap::new();

        let mut physical_offset = 0;
        let mut virtual_offset = 0;

        for entry in std::mem::take(&mut self.entries) {
            let alignment = C::alignment(&entry, &self);

            let physical_size = C::physical_size(&entry, &self);
            let virtual_size = C::virtual_size(&entry, &self);

            let entry_poffset = align_to(physical_offset, alignment);
            let entry_voffset = align_to(virtual_offset, alignment);

            entries.insert(entry, EntryMetadata {
                physical_size,
                virtual_size,
                physical_offset: entry_poffset,
                virtual_offset: entry_voffset,
                alignment,
            });

            physical_offset = entry_poffset + physical_size;
            virtual_offset = entry_voffset + virtual_size;
        }

        Layout {
            target: self.target,
            db: self.db,
            index: self.index,
            config: self.config,
            entries,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntryMetadata {
    /// Defines the physical size of the entry in the output file.
    pub(crate) physical_size: u64,

    /// Defines the virtual size of the entry when loaded into memory.
    pub(crate) virtual_size: u64,

    /// Defines the offset of the entry in the output file.
    pub(crate) physical_offset: u64,

    /// Defines the virtual address of the entry when loaded into memory.
    pub(crate) virtual_offset: u64,

    /// Defines the alignment of the entry.
    pub(crate) alignment: u64,
}

pub(crate) struct Layout<'db, C: CustomEntry> {
    pub(crate) target: Target,
    pub(crate) db: &'db mut Database,
    pub(crate) index: &'db Index,
    pub(crate) config: &'db Config,

    entries: IndexMap<Entry<C>, EntryMetadata>,
}

impl<C: CustomEntry> Layout<'_, C> {
    /// Gets an iterator over all entries in the layout.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&Entry<C>, &EntryMetadata)> {
        self.entries.iter()
    }

    /// Clones the entries from the layout and returns them.
    pub(crate) fn clone_entries(&self) -> IndexMap<Entry<C>, EntryMetadata> {
        self.entries.clone()
    }

    /// Gets the physical size of the given entry in the output file.
    pub(crate) fn size_of_entry(&self, entry: &Entry<C>) -> u64 {
        self.entries.get(entry).unwrap().physical_size
    }

    /// Gets the virtual size of the given entry when loaded into memory.
    pub(crate) fn vmsize_of_entry(&self, entry: &Entry<C>) -> u64 {
        self.entries.get(entry).unwrap().virtual_size
    }

    /// Gets the physical offset of the given entry in the output file.
    pub(crate) fn offset_of_entry(&self, entry: &Entry<C>) -> u64 {
        self.entries.get(entry).unwrap().physical_offset
    }

    /// Gets the physical offset of the given entry in the output file.
    pub(crate) fn vaddr_of_entry(&self, entry: &Entry<C>) -> u64 {
        self.entries.get(entry).unwrap().virtual_offset
    }

    /// Gets the alignment of the given entry in the output file.
    pub(crate) fn alignment_of_entry(&self, entry: &Entry<C>) -> u64 {
        self.entries.get(entry).unwrap().alignment
    }
}

impl<C: CustomEntry> std::fmt::Display for Layout<'_, C>
where
    C: EntryDisplay<C>,
{
    /// Displays the layout of the entries in the standard output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (entry, metadata) in &self.entries {
            write!(f, "Entry ")?;
            EntryDisplay::fmt(entry, self, f)?;
            writeln!(f)?;

            writeln!(f, "   Alignment:     0x{:02x}", metadata.alignment)?;
            writeln!(f, "   Phys. offset:  0x{:016x}", metadata.physical_offset)?;
            writeln!(f, "   Phys. size:    0x{:016x}", metadata.physical_size)?;
            writeln!(f, "   Virt. address: 0x{:016x}", metadata.virtual_offset)?;
            writeln!(f, "   Virt. size:    0x{:016x}", metadata.virtual_size)?;
            writeln!(f)?;
        }

        Ok(())
    }
}

impl<C: CustomEntry> Layout<'_, C> {
    /// Gets the merging section of the section with the given ID, along with
    /// the index inside the merged section.
    pub(crate) fn merging_section_of(&self, id: SectionId) -> (&MergedSection, usize) {
        self.db
            .merged_sections
            .values()
            .find_map(|merged| merged.merged_from.get_index_of(&id).map(|idx| (merged, idx)))
            .unwrap()
    }

    /// Gets the file offset of the symbol with the given ID.
    pub(crate) fn offset_of_symbol(&self, id: SymbolId) -> Option<u64> {
        let symbol = self.db.symbol(id).unwrap();
        let section_id = symbol.section?;

        let (merged_section, nested_idx) = self.merging_section_of(section_id);
        let mut parent_section_offset = self.offset_of_entry(&Entry::SectionData(merged_section.id));

        for contained_section_id in merged_section.merged_from.iter().take(nested_idx + 1) {
            let contained_section = self.db.section(*contained_section_id);
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
    pub(crate) fn vaddr_of_unmerged_section(&self, id: SectionId) -> u64 {
        let (merged_section, nested_idx) = self.merging_section_of(id);
        let merged_vaddr = self.vaddr_of_entry(&Entry::SectionData(merged_section.id));

        let mut section_vaddr = merged_vaddr;

        for contained_section_id in merged_section.merged_from.iter().take(nested_idx + 1) {
            let contained_section = self.db.section(*contained_section_id);
            section_vaddr += contained_section.data.len() as u64;
        }

        section_vaddr
    }
}

/*
impl<'db, F: FileFormat> Layout<'db, F> {
    /// Creates a new layout for the given target.
    pub(crate) fn new(target: Target, db: &'db mut Database, index: &'db Index) -> Self {
        let mut layout = Self {
            target,
            db,
            index,
            extra_headers: IndexMap::new(),
            extra_data: IndexMap::new(),
            string_table: IndexSet::new(),
            string_table_offsets: IndexMap::new(),
            _marker: PhantomData,
        };

        F::string_table(&mut layout);

        layout.extra_headers = F::additional_headers(&layout)
            .unwrap_or_default()
            .into_iter()
            .map(|item| (item.name, item))
            .collect();

        layout.extra_data = F::additional_data(&layout)
            .unwrap_or_default()
            .into_iter()
            .map(|item| (item.name, item))
            .collect();

        layout
    }

    /// Gets the size of the file header for the current target, in bytes.
    pub(crate) fn file_header_size(&self) -> usize {
        F::file_header_size(self.target)
    }

    /// Gets the size of the additional headers, in bytes.
    pub(crate) fn additional_header_total_size(&self) -> usize {
        self.extra_headers
            .values()
            .map(|h| usize::try_from(h.size).unwrap())
            .sum()
    }

    /// Gets the size of the additional data blocks, in bytes.
    pub(crate) fn additional_data_total_size(&self) -> usize {
        self.extra_data.values().map(|h| usize::try_from(h.size).unwrap()).sum()
    }

    /// Gets the summed size of all headers for the current target, in bytes.
    pub(crate) fn header_size(&self) -> usize {
        let file_header = self.file_header_size();
        let segment_headers = self.segment_count() * F::segment_header_size(self.target);
        let section_headers = self.section_count() * F::section_header_size(self.target);
        let additional_headers = self.additional_header_total_size();

        file_header + segment_headers + section_headers + additional_headers
    }

    /// Gets the number of segments in the layout.
    #[inline]
    pub(crate) fn segment_count(&self) -> usize {
        self.db.merged_segments.len()
    }

    /// Gets the number of sections in the layout.
    #[inline]
    pub(crate) fn section_count(&self) -> usize {
        self.db.merged_sections.len()
    }

    /// Gets an iterator over all segment names in the layout.
    pub(crate) fn segments(&self) -> impl Iterator<Item = &str> {
        self.db.merged_segments.keys().map(|s| s.as_str())
    }

    /// Gets an iterator over all sections which appear before the given
    /// section.
    pub(crate) fn all_sections_before(&self, id: MergedSectionId) -> impl Iterator<Item = &MergedSection> {
        self.db.merged_sections().take_while(move |sec| sec.id != id)
    }

    /// Gets an iterator over the sections in the given segment.
    pub(crate) fn sections_in_segment(&self, segment: &str) -> impl Iterator<Item = MergedSectionId> {
        static EMPTY: &indexmap::set::Slice<MergedSectionId> = indexmap::set::Slice::new();

        let merged_segments = self.db.merged_segments.get(segment).map_or(EMPTY, |seg| seg.as_slice());

        merged_segments.iter().copied()
    }

    /// Gets the size of all segments in the layout, in bytes.
    ///
    /// This figure includes the content of all segments and their containing
    /// sections.
    pub(crate) fn total_segment_size(&self) -> u64 {
        let start_position = self.header_size() as u64;
        let mut total_size = start_position;

        for segment in self.segments() {
            for section in self.sections_in_segment(segment) {
                let alignment = self.alignment_of_section(section);

                total_size = align_to(total_size, alignment as u64);
                total_size += self.size_of_section(section);
            }
        }

        total_size - start_position
    }

    /// Gets the size of the segment with the given name, in bytes.
    ///
    /// The size of the segment header is not included in the size.
    pub(crate) fn size_of_segment(&self, name: &str) -> u64 {
        if self.target.has_page_zero() && name == crate::MACOS_PAGE_ZERO_NAME {
            return 0;
        }

        self.sections_in_segment(name).map(|id| self.size_of_section(id)).sum()
    }

    /// Gets the virtual size of the segment with the given name, in bytes.
    ///
    /// The size of the segment header is not included in the size.
    pub(crate) fn vmsize_of_segment(&self, name: &str) -> u64 {
        if self.target.has_page_zero() && name == crate::MACOS_PAGE_ZERO_NAME {
            return crate::MACOS_PAGE_ZERO_SIZE;
        }

        self.sections_in_segment(name).map(|id| self.size_of_section(id)).sum()
    }

    /// Gets the size of the section with the given ID, in bytes.
    ///
    /// If the end of the section boundary was not on a aligned boundary,
    /// the size will be rounded up to the next aligned boundary.
    pub(crate) fn size_of_section(&self, id: MergedSectionId) -> u64 {
        let merged_section = self.db.merged_section(id);
        if !merged_section.occupies_space() {
            return 0;
        }

        align_to(merged_section.size, merged_section.alignment as u64)
    }

    /// Gets the offset of the first section in the given segment, in bytes.
    ///
    /// If the offset of the section was not on a aligned boundary, the returned
    /// offset will be aligned to the required alignment.
    pub(crate) fn offset_of_segment(&self, name: &str) -> u64 {
        self.sections_in_segment(name)
            .map(|id| self.offset_of_section(id))
            .min()
            .unwrap_or(0)
    }

    /// Gets the offset of the section into the file, in bytes.
    ///
    /// If the offset of the section was not on a aligned boundary, the returned
    /// offset will be aligned to the required alignment.
    pub(crate) fn offset_of_section(&self, id: MergedSectionId) -> u64 {
        self.header_size() as u64
            + self
                .all_sections_before(id)
                .map(|section| self.size_of_section(section.id))
                .sum::<u64>()
    }

    /// Gets the alignment of the section with the given ID.
    pub(crate) fn alignment_of_section(&self, id: MergedSectionId) -> usize {
        self.db.merged_section(id).alignment
    }

    /// Gets the virtual address of the first section in the given segment.
    pub(crate) fn vaddr_of_segment(&self, name: &str) -> u64 {
        let mut vaddr = 0;
        for segment in self.segments().take_while(|n| *n != name) {
            vaddr += self.vmsize_of_segment(segment);
        }

        vaddr
    }

    /// Gets the virtual address of the given section.
    pub(crate) fn vaddr_of_section(&self, id: MergedSectionId) -> u64 {
        let section = self.db.merged_section(id);

        let segment_name = section.name.segment.as_deref().unwrap_or("");
        let segment_vaddr = self.vaddr_of_segment(segment_name);

        let mut vaddr_offset = 0;
        for section in self.sections_in_segment(segment_name) {
            if section == id {
                break;
            }

            vaddr_offset += self.size_of_section(section);
        }

        segment_vaddr + vaddr_offset
    }

    /// Gets the offset of the additional data which was added from
    /// [`FileFormat::additional_data`].
    ///
    /// The offset is relative to the start of the file.
    pub(crate) fn additional_data_offset(&self, key: &str) -> u64 {
        let header_size = self.header_size() as u64;
        let segment_size = self.total_segment_size();

        let prev_data_size = self
            .extra_data
            .iter()
            .take_while(|(item_key, _item)| **item_key != key)
            .map(|(_key, data)| data.size)
            .sum::<u64>();

        header_size + segment_size + prev_data_size
    }

    /// Gets the size of the additional data which was added from
    /// [`FileFormat::additional_data`].
    pub(crate) fn additional_data_size(&self, key: &str) -> u64 {
        self.extra_data.get(&key).map_or(0, |data| data.size)
    }

    /// Gets a set of all required library IDs.
    pub(crate) fn required_library_ids(&self) -> IndexSet<LibraryId> {
        self.index.dynamic_symbols.values().copied().collect::<IndexSet<_>>()
    }

    /// Gets a set of all required libraries.
    pub(crate) fn required_libraries(&self) -> impl Iterator<Item = &Library> {
        self.required_library_ids().into_iter().map(|id| self.db.library(id))
    }
}

impl<F: FileFormat> Layout<'_, F> {
    /// Adds an offset for the given string entry inside the string table.
    ///
    /// If the string is already in the table, the existing entry is unchanged.
    pub(crate) fn add_string_offset(&mut self, string: String, offset: usize) {
        self.string_table_offsets.insert(string, offset);
    }
}
*/

pub(crate) fn align_to(addr: u64, align: u64) -> u64 {
    if align == 0 {
        return addr;
    }

    (addr + align - 1) & !(align - 1)
}
