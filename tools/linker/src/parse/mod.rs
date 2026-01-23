use std::collections::HashMap;
use std::hash::Hash;
use std::io::Read;

use indexmap::IndexMap;
use lume_errors::{MapDiagnostic, Result};
use object::{Object as _, ObjectSection, ObjectSymbol};

use crate::common::*;

/// Magic number for `.ar` archive files.
const AR_FILE_MAGIC: [u8; 8] = *b"!<arch>\n";

#[derive(Clone)]
pub(crate) enum ObjectFile {
    /// The parsed file contained a single object file.
    Single(Object),

    /// The parsed file was an archive containing multiple object files.
    Archive(Vec<Object>),
}

impl IntoIterator for ObjectFile {
    type IntoIter = std::vec::IntoIter<Self::Item>;
    type Item = Object;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            ObjectFile::Single(object) => vec![object].into_iter(),
            ObjectFile::Archive(objects) => objects.into_iter(),
        }
    }
}

/// Parses the given input file and returns the parsed object file, depending on
/// the format of the given content.
///
/// If the content uses an `.ar` archive header, the contained objects are
/// returned as [`ObjectFile::Archive`]. Otherwise, a single object file is
/// returned as [`ObjectFile::Single`].
pub(crate) fn parse_object_file<N: Hash, D: AsRef<[u8]>>(
    file: InputFileId,
    name: &N,
    content: D,
) -> Result<ObjectFile> {
    let content = content.as_ref();

    if is_archive(content) {
        let objects = parse_archive(file, content)?;

        return Ok(ObjectFile::Archive(objects));
    }

    let object_file = object::File::parse(content).map_diagnostic()?;
    let object = object_from(file, name, object_file);

    Ok(ObjectFile::Single(object))
}

/// Determines if the given file content is an archive.
fn is_archive(content: &[u8]) -> bool {
    content.starts_with(&AR_FILE_MAGIC)
}

/// Parses the given archive content and returns the parsed objects.
fn parse_archive<D>(file: InputFileId, content: D) -> Result<Vec<Object>>
where
    D: AsRef<[u8]>,
{
    let mut archive = ar::Archive::new(content.as_ref());
    let mut objects = Vec::new();

    while let Some(entry) = archive.next_entry() {
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                return Err(lume_errors::SimpleDiagnostic::new("could not parse archive entry")
                    .add_cause(err)
                    .into());
            }
        };

        let name = entry.header().identifier().to_vec();

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;

        let entry_object = parse_object_file(file, &name, &buf)?;
        objects.extend(entry_object);
    }

    Ok(objects)
}

/// Converts a [`object::File`] instance into an [`Object`] instance.
fn object_from<N: Hash>(file: InputFileId, name: &N, object: object::File) -> Object {
    let mut sections = IndexMap::new();
    let mut symbols = IndexMap::new();

    let mut section_mapping = HashMap::new();

    let object_id = ObjectId::new(file, name);

    for obj_section in object.sections() {
        let segment_name = obj_section.segment_name().expect("segment name not UTF-8");
        let section_name = obj_section.name().expect("section name not UTF-8");
        let alignment = obj_section.align();

        let placement = obj_section
            .file_range()
            .map(|(offset, size)| Placement { offset, size });

        let data = obj_section.data().unwrap().to_vec();
        let kind = section_kind_from(&obj_section);

        let id = InputSectionId::from_name(object_id, segment_name, section_name);
        section_mapping.insert(obj_section.index(), id);

        sections.insert(id, crate::InputSection {
            id,
            segment: segment_name.map(|name| name.to_owned()),
            name: section_name.to_owned(),
            placement,
            data,
            alignment: usize::try_from(alignment).unwrap(),
            kind,
            relocations: Vec::new(),
        });
    }

    for obj_symbol in object.symbols() {
        let name = obj_symbol.name().expect("symbol name not UTF-8");
        let size = obj_symbol.size();
        let address = obj_symbol.address();

        let section = obj_symbol
            .section_index()
            .map(|idx| *section_mapping.get(&idx).unwrap());

        let linkage = if obj_symbol.is_undefined() {
            Linkage::External
        } else if obj_symbol.is_global() {
            Linkage::Global
        } else {
            Linkage::Local
        };

        let id = SymbolId::from_name(object_id, name);

        symbols.insert(id, crate::Symbol {
            id,
            object: object_id,
            name: name.to_owned(),
            address: usize::try_from(address).unwrap(),
            size: usize::try_from(size).unwrap(),
            linkage,
            section,
        });
    }

    for symbol in object.imports().unwrap_or_default() {
        let symbol_name = str::from_utf8(symbol.name()).expect("symbol name not UTF-8");
        let id = SymbolId::from_name(object_id, symbol_name);

        symbols.insert(id, crate::Symbol {
            id,
            object: object_id,
            name: symbol_name.to_owned(),
            address: 0,
            size: 0,
            linkage: Linkage::External,
            section: None,
        });
    }

    for obj_section in object.sections() {
        let section_id = section_mapping.get(&obj_section.index()).unwrap();

        sections.get_mut(section_id).unwrap().relocations = obj_section
            .relocations()
            .map(|(address, relocation)| {
                let target = match relocation.target() {
                    object::RelocationTarget::Absolute => RelocationTarget::Absolute,
                    object::RelocationTarget::Symbol(id) => {
                        let symbol_id = *symbols.get_index(id.0).unwrap().0;

                        RelocationTarget::Symbol(symbol_id)
                    }
                    object::RelocationTarget::Section(id) => {
                        let section_id = *sections.get_index(id.0).unwrap().0;

                        RelocationTarget::Section(section_id)
                    }
                    _ => unimplemented!(),
                };

                Relocation {
                    address,
                    length: relocation.size() / 8,
                    addend: relocation.addend(),
                    target,
                }
            })
            .collect::<Vec<_>>();
    }

    let format = match object.format() {
        object::BinaryFormat::MachO => crate::Format::MachO,
        object::BinaryFormat::Elf => crate::Format::Elf,
        format => panic!("Unsupported binary format: {format:?}"),
    };

    Object {
        id: object_id,
        file,
        format,
        sections,
        symbols,
    }
}

/// Determines the kind of section depending on the name and/or declared section
/// attributes.
fn section_kind_from(section: &object::Section) -> SectionKind {
    let segment_name = section.segment_name().expect("segment name not UTF-8");
    let section_name = section.name().expect("section name not UTF-8");

    if segment_name == Some("__LUMEC") && section_name == "__metadata" {
        return SectionKind::LumeMetadata;
    } else if segment_name == Some("__LUMEC") && section_name == "__aliases" {
        return SectionKind::LumeAliases;
    }

    match section.kind() {
        object::SectionKind::Text => SectionKind::Text,
        object::SectionKind::Data => SectionKind::Data,
        object::SectionKind::ReadOnlyString => SectionKind::CStrings,
        _ => SectionKind::Unknown,
    }
}
