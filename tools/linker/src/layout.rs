use std::marker::PhantomData;

use indexmap::{IndexMap, IndexSet};

use crate::common::*;
use crate::{Database, Index};

/// Name of the page zero segment on macOS.
pub const MACOS_PAGE_ZERO_NAME: &str = "__PAGEZERO";

/// Default page zero size for the linker (only used on macOS).
pub const MACOS_PAGE_ZERO_SIZE: u64 = 0x0000_0001_0000_0000;

pub(crate) trait FileFormat {
    /// Gets the size of a file header for the given target, in bytes.
    fn file_header_size(target: Target) -> usize;

    /// Gets the size of a segment header for the given target, in bytes.
    fn segment_header_size(target: Target) -> usize;

    /// Gets the size of a section header for the given target, in bytes.
    fn section_header_size(target: Target) -> usize;

    /// Declares all the required string table entries.
    fn string_table(layout: &mut Layout<Self>)
    where
        Self: Sized,
    {
        let _ = layout;
    }

    /// Declares any additional headers which might be required for the
    /// implementing file format.
    fn additional_headers(layout: &Layout<Self>) -> Option<Vec<AdditionalHeader>>
    where
        Self: Sized,
    {
        let _ = layout;
        None
    }

    /// Declares any additional data blocks which might be required for the
    /// implementing file format.
    fn additional_data(layout: &Layout<Self>) -> Option<Vec<AdditionalData>>
    where
        Self: Sized,
    {
        let _ = layout;
        None
    }
}

pub(crate) struct AdditionalHeader {
    pub(crate) name: &'static str,
    pub(crate) size: u64,
}

pub(crate) struct AdditionalData {
    pub(crate) name: &'static str,
    pub(crate) size: u64,
}

pub(crate) struct Layout<'db, F: FileFormat> {
    pub(crate) target: Target,
    pub(crate) db: &'db mut Database,
    pub(crate) index: &'db Index,

    extra_headers: IndexMap<&'static str, AdditionalHeader>,
    extra_data: IndexMap<&'static str, AdditionalData>,

    pub(crate) string_table: IndexSet<String>,
    pub(crate) string_table_offsets: IndexMap<String, usize>,

    _marker: PhantomData<F>,
}

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

pub(crate) fn align_to(addr: u64, align: u64) -> u64 {
    if align == 0 {
        return addr;
    }

    (addr + align - 1) & !(align - 1)
}
