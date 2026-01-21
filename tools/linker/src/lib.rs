use std::borrow::Cow;
use std::path::PathBuf;

use indexmap::{IndexMap, IndexSet};
use lume_errors::{Result, SimpleDiagnostic};

pub(crate) mod common;
pub(crate) use common::*;

pub(crate) use crate::index::Index;

pub(crate) mod index;
pub mod layout;
pub(crate) mod library;
pub(crate) mod parse;
pub(crate) mod reloc;
pub(crate) mod write;

pub use layout::*;

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Name of the entry point symbol
    pub entry: Option<String>,

    /// List of library search paths
    pub search_paths: Option<Vec<PathBuf>>,

    /// List of libraries to link against
    pub libraries: Vec<String>,

    /// Initial stack memory size
    pub stack_size: Option<u64>,

    /// Print the output entries before writing the output file
    pub print_entries: bool,
}

#[derive(Clone)]
pub struct InputFile<'data> {
    pub path: PathBuf,
    pub content: Cow<'data, [u8]>,
}

pub fn link<'data, I>(config: Config, inputs: I) -> Result<Box<[u8]>>
where
    I: IntoIterator<Item = InputFile<'data>>,
{
    let inputs = inputs.into_iter().collect::<Vec<_>>();
    let (files, objects) = parse_objects(inputs)?;
    let target = target_from(objects.values());

    let libraries = library::read_libraries(&config, target)?;

    let mut linker = Linker {
        config,
        target,
        db: Database {
            objects,
            libraries,
            files,
            ..Database::default()
        },
        index: Index::default(),
    };

    linker.index_symbols()?;
    linker.merge_sections();

    let mut writer = write::MemoryWriter::new();
    write::write_to(&mut writer, &mut linker)?;

    Ok(writer.into_inner())
}

struct Linker {
    config: Config,
    target: Target,
    index: Index,
    db: Database,
}

impl Linker {
    pub fn db(&self) -> &Database {
        &self.db
    }
}

#[derive(Default)]
struct Database {
    objects: IndexMap<ObjectId, Object>,
    libraries: IndexMap<LibraryId, Library>,
    files: IndexMap<InputFileId, PathBuf>,

    merged_segments: IndexMap<String, IndexSet<MergedSectionId>>,
    merged_sections: IndexMap<MergedSectionId, MergedSection>,
}

impl Database {
    pub fn object(&self, id: ObjectId) -> &Object {
        self.objects.get(&id).unwrap()
    }

    pub fn object_mut(&mut self, id: ObjectId) -> &mut Object {
        self.objects.get_mut(&id).unwrap()
    }

    pub fn library(&self, id: LibraryId) -> &Library {
        self.libraries.get(&id).unwrap()
    }

    pub fn section(&self, id: SectionId) -> &Section {
        self.object(id.object).sections.get(&id).unwrap()
    }

    pub fn section_mut(&mut self, id: SectionId) -> &mut Section {
        self.object_mut(id.object).sections.get_mut(&id).unwrap()
    }

    pub fn sections(&self) -> impl Iterator<Item = &Section> {
        self.objects.values().flat_map(|object| object.sections.values())
    }

    pub fn merged_section(&self, id: MergedSectionId) -> &MergedSection {
        self.merged_sections.get(&id).unwrap()
    }

    pub fn merged_sections(&self) -> impl Iterator<Item = &MergedSection> {
        self.merged_segments
            .values()
            .flatten()
            .map(|&id| self.merged_section(id))
    }

    pub fn symbols(&self) -> impl Iterator<Item = &Symbol> {
        self.objects.values().flat_map(|object| object.symbols.values())
    }

    pub fn dynamic_symbols(&self) -> impl Iterator<Item = &DynamicSymbol> {
        self.libraries.values().flat_map(|lib| lib.symbols.iter())
    }

    pub fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.objects.get(&id.object)?.symbols.get(&id)
    }

    /// Gets an iterator over the sections in the given segment.
    pub fn sections_in_segment(&self, segment: &str) -> impl Iterator<Item = MergedSectionId> {
        static EMPTY: &indexmap::set::Slice<MergedSectionId> = indexmap::set::Slice::new();

        self.merged_segments
            .get(segment)
            .map_or(EMPTY, |seg| seg.as_slice())
            .iter()
            .copied()
    }
}

fn parse_objects(inputs: Vec<InputFile<'_>>) -> Result<(IndexMap<InputFileId, PathBuf>, IndexMap<ObjectId, Object>)> {
    let mut files = IndexMap::new();
    let mut objects = IndexMap::new();

    for input in inputs {
        let id = InputFileId(files.len());
        let file_name = input.path.display().to_string();

        let parsed_objects = match parse::parse(id, &file_name, input.content.as_ref()) {
            Ok(object) => object,
            Err(err) => {
                return Err(
                    SimpleDiagnostic::new(format!("could not parse object file {}", input.path.display()))
                        .add_cause(err)
                        .into(),
                );
            }
        };

        files.insert(id, input.path);

        for object in parsed_objects {
            objects.insert(object.id, object);
        }
    }

    Ok((files, objects))
}

fn target_from<'obj>(mut objects: impl Iterator<Item = &'obj Object>) -> Target {
    let first_object = objects.next().unwrap();
    let format = first_object.format;

    Target {
        arch: Architecture::default(),
        format,
    }
}
