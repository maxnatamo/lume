use std::fmt::Display;
use std::hash::Hash;
use std::path::PathBuf;

use indexmap::{IndexMap, IndexSet};
use lume_span::{Internable, Interned};

use crate::*;

/// Representation of a target which is expected to run the linked executables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub arch: Architecture,
    pub format: ObjectFormat,
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

/// Unique identifier for an object file.
#[derive(derive_more::Display, Debug, Hash, Clone, Copy, PartialEq, Eq)]
#[display("obj-{}-{id}", file.0)]
pub struct ObjectId {
    /// Defines the ID of the input file, from which the object file was parsed.
    ///
    /// Note: this ID may not be unique across all objects, since multiple
    /// objects may be loaded from the same archive file (`.a` files).
    pub file: InputFileId,

    pub id: usize,
}

/// Represents a single parsed object file.
#[derive(derive_more::Debug, Clone)]
pub struct ObjectFile {
    /// Unique identifier for the object file.
    pub id: ObjectId,

    /// Defines the format of the object file.
    pub format: ObjectFormat,

    /// Name of the archive entry, if the file is part of an archive.
    pub archive_entry: Option<String>,

    /// Lists all sections within the object file.
    pub sections: IndexMap<InputSectionId, InputSection>,

    /// Lists all symbols within the object file.
    pub symbols: IndexMap<SymbolId, Symbol>,
}

/// Unique identifier for a library.
#[derive(derive_more::Display, Debug, Hash, Clone, Copy, PartialEq, Eq)]
#[display("lib-{_0}")]
pub struct LibraryId(pub usize);

impl LibraryId {
    #[allow(dead_code, reason = "only used on macOS")]
    pub fn new<N: Hash>(name: &N) -> Self {
        Self(lume_span::hash_id(name))
    }
}

/// Represents a library within a parsed framework file.
#[derive(derive_more::Debug, Clone)]
pub struct FrameworkLibrary {
    /// Defines the unique identifier for the library.
    pub id: LibraryId,

    /// Defines the full path to the library within the framework file.
    ///
    /// Note: because of how Mach-O represents dylib paths in macOS, this path
    /// likely doesn't exist (depending on the version of macOS).
    pub path: PathBuf,

    /// Determines whether the library should be loaded, even when no required
    /// symbols are found.
    pub force_load: bool,

    /// Lists all symbols within the library.
    pub symbols: IndexSet<String>,
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

#[derive(derive_more::Debug, Clone)]
pub struct InputSection {
    pub id: InputSectionId,

    pub name: String,
    pub segment: Option<String>,

    pub placement: Option<Placement>,
    pub alignment: usize,

    #[debug(skip)]
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

#[derive(derive_more::Debug, Clone)]
pub struct Symbol {
    pub id: SymbolId,
    pub object: ObjectId,

    pub name: SymbolName,
    pub address: SymbolAddress,
    pub size: usize,
    pub linkage: Linkage,
    pub section: Option<InputSectionId>,
}

#[derive(derive_more::Display, Hash, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolName {
    #[display("{base}@@{version}")]
    Versioned {
        base: Interned<String>,
        version: Interned<String>,
    },

    #[display("{name}")]
    Unversioned { name: Interned<String> },
}

impl SymbolName {
    pub fn parse(name: String) -> Self {
        let mut parts = name.split("@@");

        let base = parts.next().unwrap().to_string().intern();
        let version = parts.next().map(|v| v.to_string().intern());

        match version {
            Some(version) => Self::Versioned { base, version },
            None => Self::Unversioned { name: base },
        }
    }

    pub fn base(self) -> Interned<String> {
        match self {
            SymbolName::Versioned { base, .. } => base,
            SymbolName::Unversioned { name } => name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolAddress {
    /// The symbol address is invalid or otherwise unknown, given the input
    /// object format.
    Unknown,

    /// The symbol is not defined within this object and has no address.
    Undefined,

    /// The address of the symbol is absolute and mustn't be changed.
    Absolute(u64),

    /// The address of the symbol is relative to the start of the parent
    /// section or segment (depending on the format).
    Relative(u64),
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub enum Linkage {
    External,
    Global { weak: bool },
    Local,
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
