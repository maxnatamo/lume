use std::path::{Path, PathBuf};

use error_snippet::Severity;
use indexmap::IndexMap;
use lume_errors::{Result, SimpleDiagnostic};

use crate::Config;
use crate::common::*;

#[cfg(target_os = "macos")]
mod tbd;

#[derive(Clone)]
struct ParsedLibrary {
    pub path: PathBuf,
    pub symbols: Vec<ParsedDynamicSymbol>,
}

#[derive(Clone)]
struct ParsedDynamicSymbol {
    pub name: String,
}

pub(crate) fn read_libraries(config: &Config, target: Target) -> Result<IndexMap<LibraryId, Library>> {
    let library_names = default_libraries(config);
    let search_paths = library_search_paths(config)?;

    let mut libraries = IndexMap::new();

    for library_name in library_names {
        let Some(lib_path) = search_library(&search_paths, &library_name) else {
            return Err(
                SimpleDiagnostic::new(format!("could not find library `{library_name}`"))
                    .add_causes(search_paths.iter().map(|path| {
                        SimpleDiagnostic::new(format!("searched in {}", path.display())).with_severity(Severity::Info)
                    }))
                    .into(),
            );
        };

        for parsed_lib in read_library_symbols(&lib_path, target)? {
            let lib_id = LibraryId::new(&parsed_lib.path);
            let mut library_symbols = Vec::new();

            for symbol in parsed_lib.symbols {
                library_symbols.push(DynamicSymbol {
                    library: lib_id,
                    name: symbol.name,
                });
            }

            let entry = libraries.entry(lib_id).or_insert(Library {
                id: lib_id,
                path: parsed_lib.path,
                symbols: Vec::new(),
            });

            entry.symbols.extend(library_symbols);
        }
    }

    Ok(libraries)
}

fn default_libraries(config: &Config) -> Vec<String> {
    let mut libs = config.libraries.clone();

    libs.push(String::from("System"));
    libs.push(String::from("c"));
    libs.push(String::from("m"));

    libs
}

fn library_search_paths(config: &Config) -> Result<Vec<PathBuf>> {
    fn split_env_paths(env_var: &str) -> Vec<PathBuf> {
        let Ok(env_value) = std::env::var(env_var) else {
            return Vec::new();
        };

        env_value
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    let mut paths = config.search_paths.clone().unwrap_or_default();

    #[cfg(unix)]
    {
        paths.extend(split_env_paths("LD_LIBRARY_PATH"));
        paths.extend(split_env_paths("LIBRARY_PATH"));
    }

    #[cfg(target_os = "macos")]
    {
        paths.push(macos_sdk_path()?);
    }

    #[cfg(target_os = "windows")]
    {
        paths.extend(split_env_paths("LIB"));
    }

    Ok(paths)
}

#[cfg(target_os = "macos")]
fn macos_sdk_path() -> Result<PathBuf> {
    use std::process::{Command, Stdio};

    use lume_errors::MapDiagnostic;

    let mut command = Command::new("xcrun");
    command.args(["--sdk", "macosx", "--show-sdk-path"]);

    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let output = match command.output() {
        Ok(output) => output,
        Err(err) => return Err(SimpleDiagnostic::new(format!("could not invoke xcrun: {err}")).into()),
    };

    if !output.status.success() {
        return Err(SimpleDiagnostic::new(format!(
            "xcrun exited with status code {}\n\n{}",
            output.status.code().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }

    let path = String::from_utf8(output.stdout).map_cause("could not read output of xcrun")?;

    Ok(PathBuf::from(path.trim()).join("usr/lib"))
}

fn search_library(search_paths: &[PathBuf], name: &str) -> Option<PathBuf> {
    let mut lib_names = Vec::new();

    #[cfg(unix)]
    {
        lib_names.push(format!("lib{name}.a"));
        lib_names.push(format!("lib{name}.so"));
    }

    #[cfg(target_os = "macos")]
    {
        lib_names.push(format!("lib{name}.dylib"));
        lib_names.push(format!("lib{name}.tbd"));
    }

    #[cfg(target_os = "windows")]
    {
        lib_names.push(format!("{name}.dll"));
    }

    for lib_name in lib_names {
        for path in search_paths {
            let lib_path = path.join(&lib_name);

            if lib_path.exists() {
                return Some(lib_path);
            }
        }
    }

    None
}

fn read_library_symbols(lib_path: &Path, target: Target) -> Result<Vec<ParsedLibrary>> {
    #[cfg(target_os = "macos")]
    if lib_path.extension().is_some_and(|ext| ext.to_str() == Some("tbd")) {
        return tbd::read_symbols(lib_path, target);
    }

    Err(SimpleDiagnostic::new(format!("unsupported library format: {:?}", lib_path.extension())).into())
}
