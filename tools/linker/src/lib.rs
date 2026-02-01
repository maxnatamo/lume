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
pub(crate) mod merge;
pub(crate) mod native;
pub(crate) mod symbol_db;
pub(crate) mod write;

use crate::merge::merge_sections;
pub(crate) use crate::symbol_db::{SymbolDb, index_symbols};

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

    let mut db = Database {
        files: read_inputs,
        objects: parsed_inputs.objects,
        frameworks: parsed_inputs.frameworks,
        ..Database::default()
    };

    db.add_dummy_object();

    let layout = Layout::default_layout(target);
    merge_sections(&mut db, layout);

    let mut linker = Linker {
        config,
        target,
        symbols: index_symbols(&db, dcx)?,
        db,
    };

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

struct Database {
    pub files: IndexMap<InputFileId, InputFile>,
    pub objects: IndexMap<ObjectId, ObjectFile>,
    pub frameworks: IndexMap<LibraryId, FrameworkLibrary>,

    output_segments: IndexMap<String, IndexSet<OutputSectionId>>,
    output_sections: IndexMap<OutputSectionId, OutputSection>,

    dummy_object: ObjectId,
}

impl Database {
    fn add_dummy_object(&mut self) {
        self.files.insert(self.dummy_object.file, InputFile {
            id: self.dummy_object.file,
            path: PathBuf::from("<ld-internal>"),
            format: FileFormat::Unknown,
            data: Box::new([]),
        });

        self.objects.insert(self.dummy_object, ObjectFile {
            id: self.dummy_object,
            format: ObjectFormat::Elf,
            archive_entry: None,
            sections: IndexMap::new(),
            symbols: IndexMap::new(),
        });
    }

    pub fn dummy_object_mut(&mut self) -> &mut ObjectFile {
        self.objects.get_mut(&self.dummy_object).unwrap()
    }

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

    pub fn output_section_mut(&mut self, id: OutputSectionId) -> &mut OutputSection {
        self.output_sections.get_mut(&id).unwrap()
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

impl Default for Database {
    fn default() -> Self {
        Database {
            files: IndexMap::new(),
            objects: IndexMap::new(),
            frameworks: IndexMap::new(),
            output_segments: IndexMap::new(),
            output_sections: IndexMap::new(),
            dummy_object: ObjectId {
                file: InputFileId(usize::MAX),
                id: 0,
            },
        }
    }
}
