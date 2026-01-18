use std::collections::HashMap;
use std::hash::Hash;

use indexmap::IndexMap;
use lume_errors::{MapDiagnostic, Result};
use object::{Object as _, ObjectSection, ObjectSymbol};

use crate::{
    InputFileId, Linkage, Object, ObjectId, Placement, Relocation, RelocationTarget, SectionId, SectionKind, SymbolId,
};

mod archive;

pub(crate) fn parse<N: Hash, D: AsRef<[u8]>>(file: InputFileId, name: &N, content: D) -> Result<Vec<Object>> {
    let content = content.as_ref();

    if archive::is_archive(content) {
        return archive::parse(file, content);
    }

    let object_file = object::File::parse(content).map_diagnostic()?;
    let object = object_from(file, name, object_file);

    Ok(vec![object])
}

fn object_from<N: Hash>(file: InputFileId, name: &N, object: object::File) -> Object {
    let mut sections = IndexMap::new();
    let mut symbols = IndexMap::new();

    let mut section_mapping = HashMap::new();

    let object_id = ObjectId::new(file, name);

    for obj_section in object.sections() {
        let segment_name = obj_section.segment_name().expect("segment name not UTF-8");
        let section_name = obj_section.name().expect("section name not UTF-8");
        let alignment = obj_section.align();

        let placement = obj_section.file_range().map(|(offset, size)| Placement {
            offset: usize::try_from(offset).unwrap(),
            size: usize::try_from(size).unwrap(),
        });

        let data = obj_section.data().unwrap().to_vec();
        let kind = section_kind_from(&obj_section);

        let id = SectionId::from_name(object_id, segment_name, section_name);
        section_mapping.insert(obj_section.index(), id);

        sections.insert(id, crate::Section {
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
