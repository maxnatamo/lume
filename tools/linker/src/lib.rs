use std::path::PathBuf;

use indexmap::{IndexMap, IndexSet};
use lume_errors::{DiagCtxHandle, Result};

pub(crate) mod common;
pub(crate) use common::*;

pub(crate) mod input;
pub(crate) use input::*;

pub(crate) mod layout;
pub(crate) use layout::*;

pub mod library;
pub(crate) mod macho;
pub(crate) mod native;
pub(crate) mod symbol_db;
pub(crate) mod write;

pub(crate) use crate::symbol_db::{SymbolDb, create_symbol_db};

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Name of the entry point symbol
    pub entry: Option<String>,

    /// List of library search paths
    pub search_paths: Vec<PathBuf>,

    /// List of libraries to link against
    pub libraries: Vec<String>,

    /// Initial stack memory size
    pub stack_size: Option<u64>,

    /// Print the output entries before writing the output file
    pub print_entries: bool,
}

pub fn link<I>(config: Config, input_files: I, dcx: &DiagCtxHandle) -> Result<Box<[u8]>>
where
    I: IntoIterator<Item = PathBuf>,
{
    let arch = Architecture::default();

    let mut inputs = input_files.into_iter().collect::<Vec<_>>();
    inputs.extend(library::search_libraries(&config)?);

    let read_inputs = input::read_inputs(inputs)?;
    let parsed_inputs = input::parse_inputs(read_inputs.values(), arch)?;

    let target = Target {
        arch,
        format: parsed_inputs.objects.values().next().unwrap().format,
    };

    let db = Database {
        files: read_inputs,
        objects: parsed_inputs.objects,
        frameworks: parsed_inputs.frameworks,
        ..Database::default()
    };

    let symbol_db = create_symbol_db(&db, dcx)?;

    let mut linker = Linker {
        config,
        target,
        db,
        symbols: symbol_db,
    };

    linker.merge_sections();

    let mut writer = write::MemoryWriter::new();
    write::write_to(&mut writer, &mut linker)?;

    Ok(writer.into_inner())
}

struct Linker {
    config: Config,
    target: Target,
    symbols: SymbolDb,
    db: Database,
}

impl Linker {
    /// Merge all sections with the same section names into single sections.
    pub fn merge_sections(&mut self) {
        let mut segments = IndexMap::<String, IndexSet<OutputSectionId>>::new();
        let mut sections = IndexMap::<OutputSectionId, OutputSection>::new();

        for input_section in self.db.input_sections() {
            let id = OutputSectionId::from_name(input_section.segment.as_deref(), &input_section.name);

            if let Some(segment_name) = input_section.segment.clone() {
                segments.entry(segment_name).or_default().insert(id);
            }

            let output_section = sections.entry(id).or_insert_with(|| OutputSection {
                id,
                name: SectionName {
                    segment: input_section.segment.clone(),
                    section: input_section.name.clone(),
                },
                placement: input_section.placement,
                size: 0,
                alignment: 0,
                kind: input_section.kind,
                merged_from: IndexSet::new(),
            });

            output_section.size += input_section.data.len() as u64;
            output_section.alignment = output_section.alignment.max(input_section.alignment);
            output_section.merged_from.insert(input_section.id);
        }

        self.db.output_segments = segments;
        self.db.output_sections = sections;
    }
}

#[derive(Default)]
struct Database {
    pub files: IndexMap<InputFileId, InputFile>,
    pub objects: IndexMap<ObjectId, ObjectFile>,
    pub frameworks: IndexMap<LibraryId, FrameworkLibrary>,

    output_segments: IndexMap<String, IndexSet<OutputSectionId>>,
    output_sections: IndexMap<OutputSectionId, OutputSection>,
}

impl Database {
    pub fn object(&self, id: ObjectId) -> &ObjectFile {
        self.objects.get(&id).unwrap()
    }

    pub fn object_mut(&mut self, id: ObjectId) -> &mut ObjectFile {
        self.objects.get_mut(&id).unwrap()
    }

    pub fn object_path(&self, id: ObjectId) -> String {
        let obj = self.object(id);
        let file = self.files.get(&id.file).unwrap();

        match obj.archive_entry.as_deref() {
            Some(entry) => format!("{}({entry})", file.path.display()),
            None => file.path.display().to_string(),
        }
    }

    pub fn framework(&self, id: LibraryId) -> &FrameworkLibrary {
        self.frameworks.get(&id).unwrap()
    }

    pub fn input_section(&self, id: InputSectionId) -> &InputSection {
        self.object(id.object).sections.get(&id).unwrap()
    }

    pub fn input_section_mut(&mut self, id: InputSectionId) -> &mut InputSection {
        self.object_mut(id.object).sections.get_mut(&id).unwrap()
    }

    pub fn input_sections(&self) -> impl Iterator<Item = &InputSection> {
        self.objects.values().flat_map(|object| object.sections.values())
    }

    pub fn output_section(&self, id: OutputSectionId) -> &OutputSection {
        self.output_sections.get(&id).unwrap()
    }

    pub fn output_sections(&self) -> impl Iterator<Item = &OutputSection> {
        self.output_segments
            .values()
            .flatten()
            .map(|&id| self.output_section(id))
    }

    pub fn symbols(&self) -> impl Iterator<Item = &Symbol> {
        self.objects.values().flat_map(|object| object.symbols.values())
    }

    pub fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.objects.get(&id.object)?.symbols.get(&id)
    }

    /// Gets the physical size of the section with the given ID, in bytes.
    ///
    /// If the end of the section boundary was not on a aligned boundary,
    /// the size will be rounded up to the next aligned boundary.
    pub fn size_of_section(&self, id: OutputSectionId) -> u64 {
        let merged_section = self.output_section(id);
        if !merged_section.occupies_space() {
            return 0;
        }

        align_to(merged_section.size, merged_section.alignment as u64)
    }

    /// Gets an iterator over the sections in the given segment.
    pub fn sections_in_segment(&self, segment: &str) -> impl Iterator<Item = OutputSectionId> {
        static EMPTY: &indexmap::set::Slice<OutputSectionId> = indexmap::set::Slice::new();

        self.output_segments
            .get(segment)
            .map_or(EMPTY, |seg| seg.as_slice())
            .iter()
            .copied()
    }
}
