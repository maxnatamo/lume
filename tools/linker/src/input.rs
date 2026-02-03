use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use lume_errors::{MapDiagnostic, Result, diagnostic};

use crate::*;

/// Format of an input file.
#[derive(Default, Hash, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    #[default]
    Unknown,

    /// Object file (suc as `.o`, `.obj`).
    Object(ObjectFormat),

    /// Archive file (such as `.a` files).
    Archive,

    /// Shared or static library file.
    Library,

    /// Framework file, such as `libSystem.B.tbd` (only macOS).
    Framework,
}

/// Format of an object file.
#[derive(Hash, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFormat {
    /// Mach-O binary file (mostly for macOS).
    MachO,

    /// ELF binary file (mostly for Linux).
    Elf,
}

/// Unique identifier for an input file.
#[derive(derive_more::Display, Default, Debug, Hash, Clone, Copy, PartialEq, Eq)]
#[display("file-{_0}")]
pub struct InputFileId(pub usize);

/// Represents a single input file.
#[derive(derive_more::Debug, Hash, Clone, PartialEq, Eq)]
pub struct InputFile {
    /// Defines the unique ID of the input file.
    pub id: InputFileId,

    /// Absolute path to the input file.
    #[debug("{}", path.display())]
    pub path: PathBuf,

    /// Format of the input file.
    pub format: FileFormat,

    /// Content of the input file.
    #[debug(skip)]
    pub data: Box<[u8]>,
}

#[derive(Debug, Clone)]
pub struct ParsedInputs {
    pub objects: IndexMap<ObjectId, ObjectFile>,
    pub frameworks: IndexMap<LibraryId, FrameworkLibrary>,
}

/// Magic number for `.ar` archive files.
const AR_FILE_MAGIC: [u8; 8] = *b"!<arch>\n";

/// Magic number for ELF object files.
const ELF_FILE_MAGIC: [u8; 4] = *b"\x7fELF";

/// Magic number for Mach-O object files.
const MACHO_FILE_MAGIC: [u8; 4] = *b"\xcf\xfa\xed\xfe";

/// Attempts to determine the format of the given file content.
fn determine_file_format<P, D>(path: P, content: D) -> FileFormat
where
    P: AsRef<Path>,
    D: AsRef<[u8]>,
{
    let content = content.as_ref();
    let path = path.as_ref();

    let extension = path.extension().and_then(|os_str| os_str.to_str());

    if content.starts_with(&AR_FILE_MAGIC) {
        return FileFormat::Archive;
    }

    if content.starts_with(&ELF_FILE_MAGIC) {
        return if extension == Some("so") {
            FileFormat::Library
        } else {
            FileFormat::Object(ObjectFormat::Elf)
        };
    }

    if content.starts_with(&MACHO_FILE_MAGIC) {
        return FileFormat::Object(ObjectFormat::MachO);
    }

    if extension == Some("tbd") {
        return FileFormat::Framework;
    }

    FileFormat::Unknown
}

/// Reads the given input file paths and returns a corresponding list of
/// loaded input files.
pub fn read_inputs<I>(inputs: I) -> Result<IndexMap<InputFileId, InputFile>>
where
    I: IntoIterator<Item = PathBuf>,
{
    inputs
        .into_iter()
        .enumerate()
        .map(|(index, input_file_path)| {
            let id = InputFileId(index);

            let content = std::fs::read(&input_file_path)
                .map_cause(format!("could not read input file {}", input_file_path.display()))?;

            let format = determine_file_format(&input_file_path, &content);

            Ok((id, InputFile {
                id,
                format,
                path: input_file_path,
                data: content.into_boxed_slice(),
            }))
        })
        .collect::<Result<IndexMap<InputFileId, InputFile>>>()
}

/// Parses the given iterator of input files and returns a list of parsed object
/// files.
///
/// The function skips over input files that are not object files. Conversely,
/// it concatinates multiple object files onto the list, if an input file
/// contains multiple object files.
#[allow(unused_mut, reason = "only used on macOS")]
pub fn parse_inputs<'input, I>(input_files: I, arch: Arch) -> Result<ParsedInputs>
where
    I: Iterator<Item = &'input InputFile>,
{
    let mut objects = IndexMap::new();
    let mut frameworks = IndexMap::new();

    for input_file in input_files {
        match input_file.format {
            FileFormat::Object(_) | FileFormat::Library => {
                let object_id = ObjectId {
                    file: input_file.id,
                    id: 1,
                };

                objects.insert(object_id, parse_object_file(object_id, &input_file.data)?);
            }
            FileFormat::Archive => {
                objects.extend(parse_archive(input_file)?);
            }
            FileFormat::Framework => {
                #[cfg(target_os = "macos")]
                frameworks.extend(read_framework_symbols(&input_file.path, arch)?);

                #[cfg(not(target_os = "macos"))]
                let _ = arch;
            }
            FileFormat::Unknown => {}
        }
    }

    Ok(ParsedInputs { objects, frameworks })
}

fn parse_archive(archive_file: &InputFile) -> Result<IndexMap<ObjectId, ObjectFile>> {
    use std::io::Read;

    let mut archive = ar::Archive::new(archive_file.data.as_ref());
    let mut objects = IndexMap::new();

    while let Some(entry) = archive.next_entry() {
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                return Err(
                    diagnostic!("could not parse archive entry in {}", archive_file.path.display())
                        .add_cause(err)
                        .into(),
                );
            }
        };

        let name = entry.header().identifier().to_vec();

        let object_id = ObjectId {
            file: archive_file.id,
            id: lume_span::hash_id(&name),
        };

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;

        let mut object_file = parse_object_file(object_id, &buf)?;
        object_file.archive_entry = Some(String::from_utf8_lossy(&name).to_string());

        objects.insert(object_id, object_file);
    }

    Ok(objects)
}

fn parse_object_file<D>(object_id: ObjectId, content: D) -> Result<ObjectFile>
where
    D: AsRef<[u8]>,
{
    use object::{Object as _, ObjectSection as _, ObjectSymbol as _};

    let content = content.as_ref();
    let object = object::File::parse(content).map_diagnostic()?;

    let mut sections = IndexMap::new();
    let mut symbols = IndexMap::new();

    let mut section_mapping = IndexMap::new();
    let mut symbol_mapping = IndexMap::new();

    for obj_section in object.sections() {
        let segment_name = obj_section.segment_name().expect("segment name not UTF-8");
        let section_name = obj_section.name().expect("section name not UTF-8");
        let alignment = obj_section.align();

        let placement = obj_section
            .file_range()
            .map(|(offset, size)| Placement { offset, size });

        let data = obj_section.data().unwrap().to_vec();
        let kind = section_kind_from(&obj_section);

        let mut section_flags = SectionFlags::empty();

        match obj_section.flags() {
            object::SectionFlags::Elf { sh_flags } => {
                // ELF sections are readable by default
                section_flags |= SectionFlags::Readable;

                if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0 {
                    section_flags |= SectionFlags::Allocate;
                }

                if sh_flags & u64::from(object::elf::SHF_WRITE) != 0 {
                    section_flags |= SectionFlags::Writable;
                }

                if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0 {
                    section_flags |= SectionFlags::Executable;
                }
            }
            object::SectionFlags::MachO { .. } => {}
            _ => unreachable!(),
        }

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
            flags: section_flags,
            relocations: Vec::new(),
        });
    }

    for obj_symbol in object.symbols() {
        let name = obj_symbol.name().expect("symbol name not UTF-8");
        let size = obj_symbol.size();

        let address = match obj_symbol.section() {
            object::SymbolSection::Absolute => SymbolAddress::Absolute(obj_symbol.address()),
            object::SymbolSection::Section(_) => SymbolAddress::Relative(obj_symbol.address()),
            _ => SymbolAddress::Unknown,
        };

        let section = obj_symbol
            .section_index()
            .map(|idx| *section_mapping.get(&idx).unwrap());

        let visibility = match obj_symbol.flags() {
            object::SymbolFlags::Elf { st_other, .. } => {
                if st_other & object::elf::STV_HIDDEN != 0 {
                    SymbolVisibility::Hidden
                } else if st_other & object::elf::STV_PROTECTED != 0 {
                    SymbolVisibility::Protected
                } else {
                    SymbolVisibility::Default
                }
            }
            _ => SymbolVisibility::Default,
        };

        let id = SymbolId::from_name(object_id, name);
        let linkage = symbol_linkage(&obj_symbol);

        symbol_mapping.insert(obj_symbol.index(), id);

        symbols.insert(id, crate::Symbol {
            id,
            object: object_id,
            name: SymbolName::parse(name.to_owned()),
            address,
            size: usize::try_from(size).unwrap(),
            linkage,
            visibility,
            weak: obj_symbol.is_weak(),
            section,
        });
    }

    for symbol in object.imports().unwrap_or_default() {
        let symbol_name = str::from_utf8(symbol.name()).expect("symbol name not UTF-8");
        let id = SymbolId::from_name(object_id, symbol_name);

        symbols.insert(id, crate::Symbol {
            id,
            object: object_id,
            name: SymbolName::parse(symbol_name.to_owned()),
            address: SymbolAddress::Undefined,
            size: 0,
            linkage: Linkage::External,
            visibility: SymbolVisibility::Default,
            weak: false,
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
                        let symbol_id = *symbol_mapping.get(&id).unwrap();

                        RelocationTarget::Symbol(symbol_id)
                    }
                    object::RelocationTarget::Section(id) => {
                        let section_id = *section_mapping.get(&id).unwrap();

                        RelocationTarget::InputSection(section_id)
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

    Ok(ObjectFile {
        id: object_id,
        sections,
        symbols,
        archive_entry: None,
    })
}

fn symbol_linkage(obj_symbol: &object::Symbol) -> Linkage {
    use object::ObjectSymbol as _;

    if obj_symbol.is_undefined() {
        return Linkage::External;
    }

    if let object::SymbolFlags::Elf { .. } = obj_symbol.flags()
        && obj_symbol.name() == Ok("__dso_handle")
    {
        return Linkage::Local;
    }

    if obj_symbol.is_global() {
        return Linkage::Global;
    }

    Linkage::Local
}

/// Determines the kind of section depending on the name and/or declared section
/// attributes.
fn section_kind_from(section: &object::Section) -> SectionKind {
    use object::ObjectSection as _;

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
        object::SectionKind::ReadOnlyString => SectionKind::StringTable,
        object::SectionKind::UninitializedData => SectionKind::UninitializedData,
        object::SectionKind::ReadOnlyData => SectionKind::ReadOnlyData,
        object::SectionKind::Elf(ty) => SectionKind::Elf(ty),
        object::SectionKind::Metadata if section_name == ".strtab" => SectionKind::Elf(object::elf::SHT_STRTAB),
        _ => SectionKind::Unknown,
    }
}

#[cfg(target_os = "macos")]
fn read_framework_symbols(lib_path: &Path, arch: Arch) -> Result<IndexMap<LibraryId, FrameworkLibrary>> {
    #[derive(Default, Clone, Debug, serde::Deserialize, PartialEq)]
    #[serde(default)]
    struct Document {
        pub targets: Vec<String>,

        #[serde(rename = "install-name")]
        pub install_name: String,

        #[serde(rename = "parent-umbrella")]
        pub parent_umbrella: Option<Vec<UmbrellaEntry>>,

        #[serde(default)]
        pub exports: Vec<Exports>,
    }

    impl Document {
        pub fn is_umbrella(&self) -> bool {
            self.parent_umbrella.is_none()
        }
    }

    #[derive(Default, Clone, Debug, serde::Deserialize, PartialEq)]
    #[serde(default)]
    struct UmbrellaEntry {}

    #[derive(Default, Clone, Debug, serde::Deserialize, PartialEq)]
    #[serde(default)]
    struct Exports {
        pub targets: Vec<String>,
        pub symbols: Vec<String>,
    }

    let mut libs = IndexMap::new();
    let target_name = if arch.is_arm() { "arm64-macos" } else { "x86_64-macos" };

    let tbd_content =
        std::fs::read_to_string(lib_path).map_cause(format!("failed to read library path: {}", lib_path.display()))?;

    let tbd_documents = serde_saphyr::from_multiple::<Document>(&tbd_content)
        .map_cause(format!("failed to parse TBD library path: {}", lib_path.display()))?;

    let umbrella = tbd_documents.iter().find(|doc| doc.is_umbrella()).cloned();

    for tdb_document in tbd_documents {
        if !tdb_document.targets.iter().any(|target| target.as_str() == target_name) {
            continue;
        }

        let mut symbols = IndexSet::new();

        for export in tdb_document.exports {
            // Ensure the export entry is for the current target
            if !export.targets.iter().any(|target| target.as_str() == target_name) {
                continue;
            }

            symbols.extend(export.symbols);
        }

        // If we read the symbol from a library within an umbrella library, use the path
        // of the containing library instead of the one listed within the entry.
        //
        // In practice, this turns library entries such as `libsystem_c.dylib` into
        // `libSystem.dylib`.
        let path = match umbrella.as_ref() {
            Some(umbrella) => PathBuf::from(&umbrella.install_name),
            None => PathBuf::from(&tdb_document.install_name),
        };

        let id = LibraryId::new(&path);
        let force_load = path.ends_with("libSystem.B.dylib");

        libs.insert(id, FrameworkLibrary {
            id,
            path,
            force_load,
            symbols,
        });
    }

    Ok(libs)
}
