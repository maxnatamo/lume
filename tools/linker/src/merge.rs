use lume_span::Internable;

use crate::*;

const POINTER_SIZE: usize = std::mem::size_of::<*const ()>();

/// Merge all sections with the same section names into single sections.
pub fn merge_sections(db: &mut Database, layout: Layout) {
    let mut segments = IndexMap::<String, IndexSet<OutputSectionId>>::new();
    let mut sections = IndexMap::<OutputSectionId, OutputSection>::new();

    for input_section in db.input_sections() {
        // Determine which output section the input section should be merged into.
        let output_section_name = layout.output_section_of(&input_section.name).intern();
        let output_section_id = OutputSectionId::from_name(input_section.segment.as_deref(), &output_section_name);

        let segment_name = input_section.segment.clone().unwrap_or_default();
        segments.entry(segment_name).or_default().insert(output_section_id);

        let output_section = sections.entry(output_section_id).or_insert_with(|| OutputSection {
            id: output_section_id,
            name: SectionName {
                segment: input_section.segment.clone().map(|str| str.intern()),
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

    db.output_segments = segments;
    db.output_sections = sections;

    reorder_sections(db, &layout);
    add_reserved_symbols(db, &layout);
    add_boundary_symbols(db, &layout);
}

/// Reorder all the sections as is determined in the [`Layout::order`] field.
fn reorder_sections(db: &mut Database, layout: &Layout) {
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
fn add_reserved_symbols(db: &mut Database, layout: &Layout) {
    let required_sections = match layout.target.format {
        ObjectFormat::Elf => vec![
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
        ],
        ObjectFormat::MachO => Vec::new(),
    };

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

    let dummy_section_id = db.add_dummy_section(".text");

    // `__ehdr_start` is the location of ELF file headers. Note that we define
    // this symbol unconditionally even when using a linker script, which
    // differs from the behavior implemented by GNU linker which only define
    // this symbol if ELF headers are in the memory mapped segment.
    db.add_dummy_symbol("__ehdr_start", dummy_section_id, SymbolVisibility::Hidden, true);

    // `__dso_handle` symbol is passed to cxa_finalize as a marker to identify
    // each DSO. The address of the symbol doesn't matter as long as they are
    // different in different DSOs, so we chose the start address of the DSO.
    db.add_dummy_symbol("__dso_handle", dummy_section_id, SymbolVisibility::Hidden, true);

    // `_DYNAMIC` symbol is used for object files with dynamic linking.
    db.add_dummy_symbol("_DYNAMIC", dummy_section_id, SymbolVisibility::Hidden, true);
}

fn add_boundary_symbols(db: &mut Database, layout: &Layout) {
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

        let input_section_id = db.add_dummy_section(placement_section_name);

        // Ensure the matching section is added into the output section, so the
        // relocations within it are written.
        db.output_section_mut(bound_section_id)
            .merged_from
            .insert(input_section_id);

        db.add_dummy_symbol(&boundary.symbol_name, input_section_id, SymbolVisibility::Default, true);

        db.input_section_mut(input_section_id).relocations.push(Relocation {
            address: 0,
            length: u8::try_from(POINTER_SIZE).unwrap(),
            addend: 0,
            target: RelocationTarget::OutputSection(bound_section_id),
        });
    }
}

impl Database {
    /// Adds a new dummy section with the given name.
    fn add_dummy_section(&mut self, name: &str) -> InputSectionId {
        let input_section_id = InputSectionId::from_name(self.dummy_object, None, name);

        self.dummy_object_mut()
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
    fn add_dummy_symbol(&mut self, name: &str, section: InputSectionId, visibility: SymbolVisibility, weak: bool) {
        let object_id = self.dummy_object;
        let symbol_id = SymbolId::from_name(object_id, name);

        self.dummy_object_mut().symbols.insert(symbol_id, Symbol {
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
}
