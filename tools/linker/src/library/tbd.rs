use std::path::{Path, PathBuf};

use lume_errors::{MapDiagnostic, Result};
use serde::Deserialize;

use crate::common::*;
use crate::library::{ParsedDynamicSymbol, ParsedLibrary};

#[derive(Default, Clone, Debug, Deserialize, PartialEq)]
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

#[derive(Default, Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
struct UmbrellaEntry {}

#[derive(Default, Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
struct Exports {
    pub targets: Vec<String>,
    pub symbols: Vec<String>,
}

pub(super) fn read_symbols(lib_path: &Path, target: Target) -> Result<Vec<ParsedLibrary>> {
    let mut libs = Vec::new();

    let tbd_content =
        std::fs::read_to_string(lib_path).map_cause(format!("failed to read library path: {}", lib_path.display()))?;

    let tbd_documents = serde_saphyr::from_multiple::<Document>(&tbd_content)
        .map_cause(format!("failed to parse TBD library path: {}", lib_path.display()))?;

    let umbrella = tbd_documents.iter().find(|doc| doc.is_umbrella()).cloned();

    for tdb_document in tbd_documents {
        if let Some(library) = read_symbols_from_entry(umbrella.as_ref(), tdb_document, target) {
            libs.push(library);
        }
    }

    Ok(libs)
}

fn read_symbols_from_entry(umbrella: Option<&Document>, entry: Document, target: Target) -> Option<ParsedLibrary> {
    let target_name = if target.arch.is_arm() {
        "arm64-macos"
    } else {
        "x86_64-macos"
    };

    if !entry.targets.iter().any(|target| target.as_str() == target_name) {
        return None;
    }

    let mut symbols = Vec::new();

    for export in entry.exports {
        // Ensure the export entry is for the current target
        if !export.targets.iter().any(|target| target.as_str() == target_name) {
            continue;
        }

        for symbol in export.symbols {
            symbols.push(ParsedDynamicSymbol { name: symbol });
        }
    }

    // If we read the symbol from a library within an umbrella library, use the path
    // of the containing library instead of the one listed within the entry.
    //
    // In practice, this turns library entries such as `libsystem_c.dylib` into
    // `libSystem.dylib`.
    let path = match umbrella {
        Some(umbrella) => PathBuf::from(&umbrella.install_name),
        None => PathBuf::from(&entry.install_name),
    };

    Some(ParsedLibrary { path, symbols })
}
