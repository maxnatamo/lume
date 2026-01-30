use std::fmt::Debug;
use std::hash::Hash;

use indexmap::{IndexMap, IndexSet};
use lume_span::{Internable, Interned};

use crate::*;

/// Default entry point symbol name.
pub const DEFAULT_ENTRY: &str = "_main";

pub(crate) trait SizedEntry: Hash + Debug + Clone + PartialEq + Eq {
    /// Gets the physical size of the entry within the file.
    fn physical_size(entry: &Self, ctx: &Context<Self>) -> u64;

    /// Gets the requirement alignment of the entry.
    fn alignment(entry: &Self, ctx: &Context<Self>) -> u64;
}

pub(crate) trait EntryDisplay
where
    Self: SizedEntry,
{
    /// Displays the name of the entry in a human-readable way.
    fn fmt(&self, ctx: &Context<Self>, w: &mut dyn std::fmt::Write) -> std::fmt::Result;
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

pub(crate) struct Context<'db, E: SizedEntry> {
    pub(crate) target: Target,
    pub(crate) db: &'db mut Database,
    pub(crate) symbols: &'db SymbolDb,
    pub(crate) config: &'db Config,

    current_offset: u64,
    entries: IndexMap<E, EntryMetadata>,
}

impl<'db, E: SizedEntry> Context<'db, E> {
    /// Creates a new layout builder for the given target.
    pub(crate) fn new(linker: &'db mut Linker) -> Self {
        Self {
            target: linker.target,
            db: &mut linker.db,
            symbols: &linker.symbols,
            config: &linker.config,
            current_offset: 0,
            entries: IndexMap::new(),
        }
    }

    /// Declares a new entry with the given kind.
    pub(crate) fn declare_entry(&mut self, entry: E) {
        let alignment = E::alignment(&entry, self);
        let physical_size = E::physical_size(&entry, self);

        let physical_offset = align_to(self.current_offset, alignment);

        self.entries.insert(entry, EntryMetadata {
            physical_size,
            physical_offset,
            alignment,
        });

        self.current_offset = physical_offset + physical_size;
    }

    /// Gets an iterator over all segment names in the layout.
    pub(crate) fn segments(&self) -> impl Iterator<Item = Interned<String>> {
        self.db.output_segments.keys().map(|s| s.intern())
    }

    /// Gets a set of all required library IDs.
    pub(crate) fn required_library_ids(&self) -> IndexSet<LibraryId> {
        let mut library_ids: IndexSet<_> = self.symbols.dynamic().map(|(_name, lib_id)| lib_id).collect();

        for required_lib in self.db.frameworks.values().filter(|lib| lib.force_load) {
            library_ids.insert(required_lib.id);
        }

        library_ids
    }

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

impl<E: SizedEntry> Context<'_, E> {
    /// Gets an iterator over all entries in the layout.
    pub(crate) fn iter_entries(&self) -> impl Iterator<Item = (&E, &EntryMetadata)> {
        self.entries.iter()
    }

    /// Gets a reference to the entries in the layout.
    pub(crate) fn entries(&self) -> &IndexMap<E, EntryMetadata> {
        &self.entries
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

impl<E: EntryDisplay> std::fmt::Display for Context<'_, E> {
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

/// Aligns the given address up to the given alignment.
///
/// The returned address is guaranteed to be greater than or equal to the given
/// address.
pub(crate) fn align_to(addr: u64, align: u64) -> u64 {
    assert!(align.is_power_of_two(), "`align` must be a power of two");

    if align == 0 {
        return addr;
    }

    (addr + align - 1) & !(align - 1)
}

/// Aligns the given address up to the current page size.
///
/// The returned address is guaranteed to be greater than or equal to the given
/// address.
pub(crate) fn page_align(addr: u64) -> u64 {
    align_to(addr, crate::native::page_size())
}
