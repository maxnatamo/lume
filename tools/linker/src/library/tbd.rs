use std::path::{Path, PathBuf};

use lume_errors::{MapDiagnostic, Result};
use serde::Deserialize;

use crate::common::*;
use crate::library::{ParsedDynamicSymbol, ParsedLibrary};

#[derive(Default, Debug, Deserialize, PartialEq)]
#[serde(default)]
struct Document {
    pub targets: Vec<String>,

    #[serde(rename = "install-name")]
    pub install_name: String,

    pub exports: Vec<Exports>,
}

#[derive(Default, Debug, Deserialize, PartialEq)]
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

    for tdb_document in tbd_documents {
        if let Some(library) = read_symbols_from_entry(tdb_document, target) {
            libs.push(library);
        }
    }

    Ok(libs)
}

fn read_symbols_from_entry(entry: Document, target: Target) -> Option<ParsedLibrary> {
    let target_name = if target.arch.is_arm() {
        "arm64-macos"
    } else {
        "x86_64-macos"
    };

    if !entry.targets.iter().any(|target| target.as_str() == target_name) {
        return None;
    }

    let path = PathBuf::from(&entry.install_name);
    let name = entry.install_name;

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

    Some(ParsedLibrary { name, path, symbols })
}
