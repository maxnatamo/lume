use std::fmt::Display;
use std::hash::Hash;
use std::path::PathBuf;

use indexmap::{IndexMap, IndexSet};

/// Representation of a target which is expected to run the linked executables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub arch: Architecture,
    pub format: Format,
}

impl Target {
    pub fn is_64bit(self) -> bool {
        self.arch.is_64bit()
    }

    pub fn is_x86(self) -> bool {
        self.arch.is_x86()
    }

    pub fn is_arm(self) -> bool {
        self.arch.is_arm()
    }

    pub fn has_page_zero(self) -> bool {
        self.format == Format::MachO
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "constructed per host arch")]
pub enum Architecture {
    #[cfg_attr(target_arch = "x86", default)]
    X86,

    #[cfg_attr(target_arch = "x86_64", default)]
    X86_64,

    #[cfg_attr(target_arch = "arm", default)]
    Arm,

    #[cfg_attr(target_arch = "aarch64", default)]
    Arm64,
}

impl Architecture {
    pub fn is_64bit(self) -> bool {
        matches!(self, Architecture::X86_64 | Architecture::Arm64)
    }

    pub fn is_x86(self) -> bool {
        matches!(self, Architecture::X86 | Architecture::X86_64)
    }

    pub fn is_arm(self) -> bool {
        matches!(self, Architecture::Arm | Architecture::Arm64)
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    #[default]
    Unknown,
    MachO,
    Elf,
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub struct InputFileId(pub usize);

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub struct ObjectId {
    pub file: InputFileId,
    pub id: usize,
}

impl ObjectId {
    pub fn new<N: Hash>(file: InputFileId, name: &N) -> Self {
        Self {
            file,
            id: lume_span::hash_id(name),
        }
    }
}

/// Represents an object file which is being linked, along with zero-or-more
/// other object files and libraries.
#[derive(Clone)]
pub struct Object {
    pub id: ObjectId,

    /// The ID of the file this object was loaded from.
    ///
    /// Note: this ID may not be unique across all objects, since multiple
    /// objects may be loaded from the same archive file (`.ar` files).
    pub file: InputFileId,

    pub format: Format,
    pub sections: IndexMap<InputSectionId, InputSection>,
    pub symbols: IndexMap<SymbolId, Symbol>,
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub struct LibraryId(pub usize);

impl LibraryId {
    pub fn new<N: Hash>(name: &N) -> Self {
        Self(lume_span::hash_id(name))
    }
}

#[derive(Debug, Clone)]
pub struct Library {
    pub id: LibraryId,
    pub path: PathBuf,
    pub symbols: Vec<DynamicSymbol>,
}

#[derive(Hash, Default, Clone, PartialEq, Eq)]
pub struct SectionName {
    pub segment: Option<String>,
    pub section: String,
}

impl Display for SectionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(segment) = &self.segment {
            write!(f, "{},{}", segment, self.section)
        } else {
            write!(f, "{}", self.section)
        }
    }
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub struct InputSectionId {
    pub object: ObjectId,
    pub id: usize,
}

impl InputSectionId {
    pub fn from_name(object: ObjectId, segment_name: Option<&str>, section_name: &str) -> Self {
        Self {
            object,
            id: lume_span::hash_id(&(object, segment_name, section_name)),
        }
    }
}

#[derive(Clone)]
pub struct InputSection {
    pub id: InputSectionId,

    pub name: String,
    pub segment: Option<String>,

    pub placement: Option<Placement>,
    pub alignment: usize,
    pub data: Vec<u8>,
    pub kind: SectionKind,
    pub relocations: Vec<Relocation>,
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub struct SymbolId {
    pub object: ObjectId,
    pub id: usize,
}

impl SymbolId {
    pub fn from_name(object: ObjectId, name: &str) -> Self {
        Self {
            object,
            id: lume_span::hash_id(name),
        }
    }
}

#[derive(Clone)]
pub struct Symbol {
    pub id: SymbolId,
    pub object: ObjectId,

    pub name: String,
    pub address: usize,
    pub size: usize,
    pub linkage: Linkage,
    pub section: Option<InputSectionId>,
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub enum Linkage {
    External,
    Global,
    Local,
}

#[derive(Debug, Clone)]
pub struct DynamicSymbol {
    pub library: LibraryId,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Relocation {
    pub address: u64,
    pub length: u8,
    pub addend: i64,
    pub target: RelocationTarget,
}

#[derive(Debug, Clone)]
pub enum RelocationTarget {
    Absolute,
    Symbol(SymbolId),
    Section(InputSectionId),
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub struct OutputSectionId(usize);

impl OutputSectionId {
    pub fn from_name(segment_name: Option<&str>, section_name: &str) -> Self {
        Self(lume_span::hash_id(&(segment_name, section_name)))
    }
}

#[derive(Clone)]
pub struct OutputSection {
    pub id: OutputSectionId,
    pub name: SectionName,
    pub placement: Option<Placement>,

    pub size: u64,
    pub alignment: usize,
    pub kind: SectionKind,

    /// Defines the IDs of the sections which have been merged into this
    /// output section.
    pub merged_from: IndexSet<InputSectionId>,
}

impl OutputSection {
    /// Determines if the section occupies space in the file.
    pub fn occupies_space(&self) -> bool {
        self.placement.is_some() && self.kind != SectionKind::ZeroFilled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// Unknown section.
    Unknown,

    /// Executable code section.
    Text,

    /// Data section.
    Data,

    /// Zero-filled section.
    ZeroFilled,

    /// Section of null-terminated strings.
    CStrings,

    /// Metadata section for Lume programs.
    LumeMetadata,

    /// Metadata alias section for Lume programs.
    LumeAliases,
}
