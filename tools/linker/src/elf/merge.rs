use lume_span::{Internable, Interned};
use regex::Regex;

use crate::*;

const POINTER_SIZE: usize = std::mem::size_of::<*const ()>();

/// Merge all sections with the same section names into single sections.
pub fn merge_sections(db: &mut Database) {
    let mut sections = IndexMap::<OutputSectionId, OutputSection>::new();
    let mapper = SectionMapper::default();

    for input_section in db.input_sections() {
        // Determine which output section the input section should be merged into.
        let output_section_name = mapper.output_section_of(&input_section.name).intern();
        let output_section_id = OutputSectionId::from_name(input_section.segment.as_deref(), &output_section_name);

        let output_section = sections.entry(output_section_id).or_insert_with(|| OutputSection {
            id: output_section_id,
            name: SectionName {
                segment: None,
                section: output_section_name,
            },
            placement: input_section.placement,
            size: 0,
            alignment: 1,
            kind: input_section.kind,
            flags: input_section.flags,
            merged_from: IndexSet::new(),
        });

        output_section.size += input_section.data.len() as u64;
        output_section.alignment = output_section.alignment.max(input_section.alignment);
        output_section.flags |= input_section.flags;
        output_section.merged_from.insert(input_section.id);
    }

    db.output_sections = sections;
    crate::elf::apply_rules(db);

    reorder_sections(db, &mapper);
    add_reserved_symbols(db);
    add_boundary_symbols(db, &layout);
}

/// Reorder all the sections as is determined in the [`Layout::order`] field.
fn reorder_sections(db: &mut Database, layout: &SectionMapper) {
    for (_segment_name, section_id_set) in &mut db.output_segments {
        let mut reordered_set = IndexMap::<usize, Vec<OutputSectionId>>::with_capacity(section_id_set.len());

        for &section_id in section_id_set.iter() {
            let output_section = db.output_sections.get(&section_id).unwrap();

            let ordering = layout
                .order_of_section(&output_section.name.section)
                .unwrap_or(usize::MAX);

            reordered_set.entry(ordering).or_default().push(section_id);
        }

        reordered_set.sort_unstable_keys();
        *section_id_set = reordered_set.into_values().flatten().collect();
    }
}

struct RequiredSection {
    pub(crate) name: String,

    pub(crate) kind: SectionKind,
    pub(crate) flags: SectionFlags,
    pub(crate) alignment: usize,
}

impl Default for RequiredSection {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: SectionKind::Data,
            flags: SectionFlags::None,
            alignment: 1,
        }
    }
}

/// Some sections aren't guaranteed to exist, but are still required within most
/// binaries.
///
/// For example, `libc.a` on Linux requires `.preinit_array` which is only
/// declared for some C/C++ object files.
fn add_reserved_symbols(db: &mut Database) {
    let required_sections = vec![
        RequiredSection {
            name: String::from(".init"),
            flags: SectionFlags::Allocate | SectionFlags::Executable,
            ..Default::default()
        },
        RequiredSection {
            name: String::from(".fini"),
            flags: SectionFlags::Allocate | SectionFlags::Executable,
            ..Default::default()
        },
        RequiredSection {
            name: String::from(".preinit_array"),
            flags: SectionFlags::Allocate | SectionFlags::Writable,
            ..Default::default()
        },
        RequiredSection {
            name: String::from(".init_array"),
            flags: SectionFlags::Allocate | SectionFlags::Writable,
            ..Default::default()
        },
        RequiredSection {
            name: String::from(".fini_array"),
            flags: SectionFlags::Allocate | SectionFlags::Writable,
            ..Default::default()
        },
        RequiredSection {
            name: String::from(".rela.iplt"),
            flags: SectionFlags::Allocate | SectionFlags::Writable,
            ..Default::default()
        },
    ];

    for details in required_sections {
        let id = OutputSectionId::from_name(None, &details.name);

        db.output_sections.entry(id).or_insert_with(|| OutputSection {
            id,
            name: SectionName {
                segment: None,
                section: details.name.intern(),
            },
            placement: None,
            size: 0,
            alignment: details.alignment,
            kind: details.kind,
            flags: details.flags,
            merged_from: IndexSet::new(),
        });
    }

    let dummy_section_id = add_dummy_section(db, ".text");

    // `__ehdr_start` is the location of ELF file headers. Note that we define
    // this symbol unconditionally even when using a linker script, which
    // differs from the behavior implemented by GNU linker which only define
    // this symbol if ELF headers are in the memory mapped segment.
    add_dummy_symbol(db, "__ehdr_start", dummy_section_id, SymbolVisibility::Hidden, true);

    // `__dso_handle` symbol is passed to cxa_finalize as a marker to identify
    // each DSO. The address of the symbol doesn't matter as long as they are
    // different in different DSOs, so we chose the start address of the DSO.
    add_dummy_symbol(db, "__dso_handle", dummy_section_id, SymbolVisibility::Hidden, true);

    // `_DYNAMIC` symbol is used for object files with dynamic linking.
    add_dummy_symbol(db, "_DYNAMIC", dummy_section_id, SymbolVisibility::Hidden, true);
}

fn add_boundary_symbols(db: &mut Database, layout: &SectionMapper) {
    for boundary in &layout.boundaries {
        // If the boundary isn't bound to any specific symbol, use either the first or
        // last output section, depending on the alignment of the boundary.
        let Some(bound_section_name) = boundary.bound_section.or_else(|| {
            let chosen_output_section = match boundary.placement {
                BoundaryPlacement::Start => db.output_sections.first(),
                BoundaryPlacement::End => db.output_sections.last(),
            };

            chosen_output_section.map(|last_section| last_section.1.name.section)
        }) else {
            continue;
        };

        let Some(placement_section_name) = boundary.placed_in.as_ref() else {
            continue;
        };

        let bound_section_id = match db
            .output_sections
            .values()
            .find(|section| section.name.section == bound_section_name)
        {
            Some(bound_section) => bound_section.id,
            None => continue,
        };

        let input_section_id = add_dummy_section(db, placement_section_name);

        // Ensure the matching section is added into the output section, so the
        // relocations within it are written.
        db.output_section_mut(bound_section_id)
            .merged_from
            .insert(input_section_id);

        add_dummy_symbol(
            db,
            &boundary.symbol_name,
            input_section_id,
            SymbolVisibility::Default,
            true,
        );

        db.input_section_mut(input_section_id).relocations.push(Relocation {
            address: 0,
            length: u8::try_from(POINTER_SIZE).unwrap(),
            addend: 0,
            target: RelocationTarget::OutputSection(bound_section_id),
        });
    }
}

/// Adds a new dummy section with the given name.
fn add_dummy_object(db: &mut Database, name: &str) -> InputSectionId {
    db.files.insert(db.dummy_object.file, InputFile {
        id: db.dummy_object.file,
        path: PathBuf::from("<ld-internal>"),
        format: FileFormat::Unknown,
        data: Box::new([]),
    });

    db.objects.insert(db.dummy_object, ObjectFile {
        id: db.dummy_object,
        archive_entry: None,
        sections: IndexMap::new(),
        symbols: IndexMap::new(),
    });

    input_section_id
}

/// Adds a new dummy section with the given name.
fn add_dummy_section(db: &mut Database, name: &str) -> InputSectionId {
    let input_section_id = InputSectionId::from_name(db.dummy_object, None, name);

    db.dummy_object_mut()
        .sections
        .entry(input_section_id)
        .or_insert_with(|| InputSection {
            id: input_section_id,
            segment: None,
            name: name.to_string(),
            kind: SectionKind::Data,
            flags: SectionFlags::all(),
            alignment: 1,
            relocations: Vec::new(),
            placement: None,
            data: Vec::new(),
        });

    input_section_id
}

/// Adds a new dummy symbol to the given input section.
fn add_dummy_symbol(db: &mut Database, name: &str, section: InputSectionId, visibility: SymbolVisibility, weak: bool) {
    let object_id = db.dummy_object;
    let symbol_id = SymbolId::from_name(object_id, name);

    db.dummy_object_mut().symbols.insert(symbol_id, Symbol {
        id: symbol_id,
        object: object_id,
        name: SymbolName::parse(name.to_string()),
        address: SymbolAddress::Absolute(0x0000_0000_0000_0000),
        size: POINTER_SIZE,
        linkage: Linkage::Global,
        visibility,
        weak,
        section: Some(section),
    });
}

#[derive(Debug, Clone)]
pub struct SectionMapper {
    pub(crate) order: Vec<Ordering>,
    pub(crate) boundaries: Vec<Boundary>,
}

impl SectionMapper {
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

impl Default for SectionMapper {
    fn default() -> Self {
        let order = vec![
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
        ];

        #[rustfmt::skip]
        let boundaries = vec![
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
        ];

        Self { order, boundaries }
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
