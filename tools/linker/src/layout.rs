use std::fmt::Debug;
use std::hash::Hash;

use indexmap::{IndexMap, IndexSet};
use lume_span::{Internable, Interned};
use regex::Regex;

use crate::*;

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

#[derive(Debug, Clone)]
pub struct Layout {
    pub(crate) target: Target,

    pub(crate) order: Vec<Ordering>,
    pub(crate) boundaries: Vec<Boundary>,
}

impl Layout {
    pub fn default_layout(target: Target) -> Self {
        let order = match target.format {
            ObjectFormat::Elf => {
                vec![
                    Ordering::section(".rela.plt", NameMatcher::literal(".rela.plt")),
                    Ordering::section(".rela.plt", NameMatcher::literal(".rela.iplt")),
                    Ordering::section(".text", NameMatcher::literal(".text")),
                    Ordering::section(".text", NameMatcher::regex(r"^\.text.*")),
                    Ordering::section(".fini", NameMatcher::literal(".fini")),
                    Ordering::section(".rodata", NameMatcher::literal(".rodata")),
                    Ordering::section(".rodata", NameMatcher::regex(r"^\.rodata.*")),
                    Ordering::section(".preinit_array", NameMatcher::literal(".preinit_array")),
                    Ordering::section(".init_array", NameMatcher::literal(".init_array")),
                    Ordering::section(".fini_array", NameMatcher::literal(".fini_array")),
                    Ordering::section(".data", NameMatcher::literal(".data")),
                    Ordering::section(".data", NameMatcher::regex(r"^\.data.*")),
                    Ordering::section(".bss", NameMatcher::literal(".bss")),
                    Ordering::section(".bss", NameMatcher::regex(r"^\.bss.*")),
                    Ordering::Remaining,
                ]
            }
            ObjectFormat::MachO => vec![Ordering::Remaining],
        };

        let boundaries = match target.format {
            #[rustfmt::skip]
            ObjectFormat::Elf => {
                vec![
                    Boundary::new("__rela_iplt_start")
                        .bound_to_start_of(".rela.iplt")
                        .placed_in(".rela.plt"),

                    Boundary::new("__rela_iplt_end")
                        .bound_to_end_of(".rela.iplt")
                        .placed_in(".rela.plt"),

                    Boundary::new("etext")
                        .bound_to_end_of(".fini")
                        .placed_in(".text"),

                    Boundary::new("__preinit_array_start")
                        .bound_to_start_of(".preinit_array")
                        .placed_in(".preinit_array"),

                    Boundary::new("__preinit_array_end")
                        .bound_to_end_of(".preinit_array")
                        .placed_in(".preinit_array"),

                    Boundary::new("__init_array_start")
                        .bound_to_start_of(".init_array")
                        .placed_in(".init_array"),

                    Boundary::new("__init_array_end")
                        .bound_to_end_of(".init_array")
                        .placed_in(".init_array"),

                    Boundary::new("__fini_array_start")
                        .bound_to_start_of(".fini_array")
                        .placed_in(".fini_array"),

                    Boundary::new("__fini_array_end")
                        .bound_to_end_of(".fini_array")
                        .placed_in(".fini_array"),

                    Boundary::new("__data_start")
                        .bound_to_start_of(".data")
                        .placed_in(".data"),

                    Boundary::new("_edata")
                        .bound_to_end_of(".data")
                        .placed_in(".data"),

                    Boundary::new("__bss_start")
                        .bound_to_start_of(".bss")
                        .placed_in(".bss"),

                    Boundary::new("__bss_end__")
                        .bound_to_end_of(".bss")
                        .placed_in(".bss"),

                    Boundary::new("_end")
                        .bound_to_end_of_file()
                        .placed_in(".bss"),

                    Boundary::new("__end__")
                        .bound_to_end_of_file()
                        .placed_in(".bss"),
                ]
            }
            ObjectFormat::MachO => Vec::new(),
        };

        Self {
            target,
            order,
            boundaries,
        }
    }

    pub(crate) fn order_of_section(&self, section_name: &str) -> Option<usize> {
        self.order.iter().position(|order| match order {
            Ordering::Section { matcher, .. } => matcher.matches(section_name),
            Ordering::Remaining => true,
        })
    }

    pub(crate) fn output_section_of(&self, input_section_name: &str) -> String {
        self.order
            .iter()
            .find_map(|ordering| {
                if let Ordering::Section { name, matcher } = ordering
                    && matcher.matches(input_section_name)
                {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| input_section_name.to_string())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Ordering {
    /// Defines a merged section with the given name, containing all sections
    /// that match the given matcher.
    Section { name: String, matcher: NameMatcher },

    /// Places all remaining sections at the location of the item.
    Remaining,
}

impl Ordering {
    pub fn section<S: Into<String>>(name: S, matcher: NameMatcher) -> Self {
        Self::Section {
            name: name.into(),
            matcher,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum NameMatcher {
    /// The operand must equal the contained string, case insensitive.
    Literal(String),

    /// The operand must match the contained regex pattern.
    Regex(Regex),
}

impl NameMatcher {
    /// Creates a new literal matcher.
    pub fn literal<S: Into<String>>(literal: S) -> Self {
        Self::Literal(literal.into())
    }

    /// Creates a new regex matcher.
    pub fn regex<P: AsRef<str>>(pattern: P) -> Self {
        Self::Regex(Regex::new(pattern.as_ref()).unwrap())
    }

    /// Determines whether the given operand matches the matcher.
    pub fn matches(&self, name: &str) -> bool {
        match self {
            NameMatcher::Literal(literal) => literal.eq_ignore_ascii_case(name),
            NameMatcher::Regex(regex) => regex.is_match(name),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Boundary {
    /// Name of the boundary symbol.
    pub symbol_name: Interned<String>,

    /// Name of the section in which the symbol is placed.
    pub placed_in: Option<Interned<String>>,

    /// Name of the section which the boundary symbol references.
    pub bound_section: Option<Interned<String>>,

    /// Where the boundary symbol should point to within the section.
    pub placement: BoundaryPlacement,
}

impl Boundary {
    pub fn new<N: Into<String>>(name: N) -> Self {
        Self {
            symbol_name: name.into().intern(),
            bound_section: None,
            placed_in: None,
            placement: BoundaryPlacement::Start,
        }
    }

    /// Bind the boundary symbol to point to the start of the given section.
    pub fn bound_to_start_of<S: Into<String>>(mut self, section_name: S) -> Self {
        self.bound_section = Some(section_name.into().intern());
        self.placement = BoundaryPlacement::Start;
        self
    }

    /// Bind the boundary symbol to point to the end of the given section.
    pub fn bound_to_end_of<S: Into<String>>(mut self, section_name: S) -> Self {
        self.bound_section = Some(section_name.into().intern());
        self.placement = BoundaryPlacement::End;
        self
    }

    /// Bind the boundary symbol to point to the end of the file.
    pub fn bound_to_end_of_file(mut self) -> Self {
        self.bound_section = None;
        self.placement = BoundaryPlacement::End;
        self
    }

    /// Sets the section in which the boundary symbol should be placed in.
    ///
    /// This differs from which section the symbol is bound to:
    /// - the bound section is used to determine the address of the boundary
    ///   symbol, whereas
    /// - the placement section defines which section the symbol itself should
    ///   be placed in.
    pub fn placed_in<S: Into<String>>(mut self, section_name: S) -> Self {
        self.placed_in = Some(section_name.into().intern());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundaryPlacement {
    /// Point to the start of the section.
    Start,

    /// Point to the end of the section.
    End,
}
