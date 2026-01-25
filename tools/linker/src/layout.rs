use std::fmt::Debug;
use std::hash::Hash;

use indexmap::{IndexMap, IndexSet};
use lume_span::{Internable, Interned};

use crate::common::*;
use crate::{Config, Database, Index, Linker};

/// Default entry point symbol name.
pub const DEFAULT_ENTRY: &str = "_main";

pub(crate) trait SizedEntry: Hash + Debug + Clone + PartialEq + Eq {
    /// Gets the physical size of the entry within the file.
    fn physical_size(entry: &Self, builder: &LayoutBuilder<Self>) -> u64;

    /// Gets the requirement alignment of the entry.
    fn alignment(entry: &Self, builder: &LayoutBuilder<Self>) -> u64;
}

pub(crate) trait EntryDisplay
where
    Self: SizedEntry,
{
    /// Displays the name of the entry in a human-readable way.
    fn fmt(&self, builder: &Layout<Self>, w: &mut dyn std::fmt::Write) -> std::fmt::Result;
}

pub(crate) struct LayoutBuilder<'db, E: SizedEntry> {
    pub(crate) target: Target,
    pub(crate) db: &'db mut Database,
    pub(crate) index: &'db Index,
    pub(crate) config: &'db Config,

    entries: IndexSet<E>,
}

impl<'db, E: SizedEntry> LayoutBuilder<'db, E> {
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
    pub(crate) fn declare_entry(&mut self, kind: E) {
        self.entries.insert(kind);
    }

    /// Gets an iterator over all segment names in the layout.
    pub(crate) fn segments(&self) -> impl Iterator<Item = Interned<String>> {
        self.db.output_segments.keys().map(|s| s.intern())
    }

    /// Gets a set of all required library IDs.
    pub(crate) fn required_library_ids(&self) -> IndexSet<LibraryId> {
        let mut library_ids: IndexSet<_> = self.index.dynamic_symbols.values().copied().collect();

        for required_lib in self.db.libraries.values().filter(|lib| lib.required) {
            library_ids.insert(required_lib.id);
        }

        library_ids
    }

    /// Gets the physical size of the section with the given ID, in bytes.
    ///
    /// If the end of the section boundary was not on a aligned boundary,
    /// the size will be rounded up to the next aligned boundary.
    pub(crate) fn size_of_section(&self, id: OutputSectionId) -> u64 {
        let merged_section = self.db.output_section(id);
        if !merged_section.occupies_space() {
            return 0;
        }

        align_to(merged_section.size, merged_section.alignment as u64)
    }

    /// Consumes the builder and creates a new [`Layout`].
    pub(crate) fn into_layout(mut self) -> Layout<'db, E> {
        let mut entries = IndexMap::new();

        let mut current_offset = 0;

        for entry in std::mem::take(&mut self.entries) {
            let alignment = E::alignment(&entry, &self);
            let physical_size = E::physical_size(&entry, &self);

            let physical_offset = align_to(current_offset, alignment);

            entries.insert(entry, EntryMetadata {
                physical_size,
                physical_offset,
                alignment,
            });

            current_offset = physical_offset + physical_size;
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

    /// Defines the offset of the entry in the output file.
    pub(crate) physical_offset: u64,

    /// Defines the alignment of the entry.
    pub(crate) alignment: u64,
}

pub(crate) struct Layout<'db, E: SizedEntry> {
    pub(crate) target: Target,
    pub(crate) db: &'db mut Database,
    pub(crate) index: &'db Index,
    pub(crate) config: &'db Config,

    pub(crate) entries: IndexMap<E, EntryMetadata>,
}

impl<E: SizedEntry> Layout<'_, E> {
    /// Gets an iterator over all entries in the layout.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&E, &EntryMetadata)> {
        self.entries.iter()
    }

    /// Clones the entries from the layout and returns them.
    pub(crate) fn clone_entries(&self) -> IndexMap<E, EntryMetadata> {
        self.entries.clone()
    }

    /// Gets the physical size of the given entry in the output file.
    pub(crate) fn size_of_entry(&self, entry: &E) -> u64 {
        self.entries.get(entry).unwrap().physical_size
    }

    /// Gets the physical offset of the given entry in the output file.
    pub(crate) fn offset_of_entry(&self, entry: &E) -> u64 {
        self.entries.get(entry).unwrap().physical_offset
    }

    /// Gets the alignment of the given entry in the output file.
    pub(crate) fn alignment_of_entry(&self, entry: &E) -> u64 {
        self.entries.get(entry).unwrap().alignment
    }
}

impl<E: EntryDisplay> std::fmt::Display for Layout<'_, E> {
    /// Displays the layout of the entries in the standard output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (entry, metadata) in &self.entries {
            write!(
                f,
                "[0x{:08x} + 0x{:04x}]   ",
                metadata.physical_offset, metadata.physical_size
            )?;

            EntryDisplay::fmt(entry, self, f)?;
            writeln!(f)?;
        }

        Ok(())
    }
}

impl<E: SizedEntry> Layout<'_, E> {
    /// Gets the input section of the section with the given ID, along with
    /// the index inside the output section.
    pub(crate) fn input_section_of(&self, id: InputSectionId) -> (&OutputSection, usize) {
        self.db
            .output_sections
            .values()
            .find_map(|merged| merged.merged_from.get_index_of(&id).map(|idx| (merged, idx)))
            .unwrap()
    }
}

pub(crate) fn align_to(addr: u64, align: u64) -> u64 {
    if align == 0 {
        return addr;
    }

    (addr + align - 1) & !(align - 1)
}
