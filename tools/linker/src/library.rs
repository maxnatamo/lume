use std::path::PathBuf;

use indexmap::{IndexMap, IndexSet};
use lume_errors::{Result, SimpleDiagnostic, diagnostic};

use crate::Config;

pub(crate) fn search_libraries(config: &Config) -> Result<Vec<PathBuf>> {
    let library_names = default_libraries(config);
    let search_paths = library_search_paths(config)?;

    let mut lib_files = Vec::new();
    let index = LibraryIndex::create(&search_paths);

    for library_name in library_names {
        lib_files.push(index.find(&library_name)?.clone());
    }

    Ok(lib_files)
}

fn default_libraries(config: &Config) -> IndexSet<String> {
    let mut libs = IndexSet::new();

    #[cfg(unix)]
    {
        libs.insert(String::from("c"));
        libs.insert(String::from("m"));
    }

    #[cfg(target_os = "macos")]
    {
        libs.insert(String::from("System"));
    }

    libs.extend(config.libraries.clone());

    libs
}

fn library_search_paths(config: &Config) -> Result<Vec<PathBuf>> {
    fn split_env_paths(env_var: &str) -> IndexSet<PathBuf> {
        let Ok(env_value) = std::env::var(env_var) else {
            return IndexSet::new();
        };

        env_value
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    let mut paths = config.search_paths.clone();

    #[cfg(unix)]
    {
        paths.extend(split_env_paths("LD_LIBRARY_PATH"));
        paths.extend(split_env_paths("LIBRARY_PATH"));
    }

    #[cfg(target_os = "linux")]
    {
        paths.extend(linux_ld_search_paths()?);
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

#[cfg(target_os = "linux")]
fn linux_ld_search_paths() -> Result<IndexSet<PathBuf>> {
    use lume_errors::MapDiagnostic;

    static SEARCH_PATH_PATTERN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"SEARCH_DIR\("=(?<path>[a-zA-Z0-9-_/]+)"\);"#).unwrap());

    let mut command = std::process::Command::new("ld");
    command.args(["--verbose"]);

    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let output = match command.output() {
        Ok(output) => output,
        Err(err) => return Err(SimpleDiagnostic::new(format!("could not invoke ld: {err}")).into()),
    };

    if !output.status.success() {
        return Err(SimpleDiagnostic::new(format!(
            "ld exited with status code {}\n\n{}",
            output.status.code().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }

    let output = String::from_utf8(output.stdout).map_cause("could not read output of ld")?;
    let mut search_paths = IndexSet::new();

    for capture in SEARCH_PATH_PATTERN.captures_iter(&output) {
        let path = capture.name("path").unwrap().as_str();

        search_paths.insert(PathBuf::from(path));
    }

    Ok(search_paths)
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

#[derive(Default)]
struct LibraryIndex {
    inner: IndexMap<String, PathBuf>,
}

impl LibraryIndex {
    fn create(search_paths: &[PathBuf]) -> Self {
        let mut index = IndexMap::new();

        for path in search_paths {
            let Ok(dir_entries) = std::fs::read_dir(path) else {
                continue;
            };

            for dir_entry in dir_entries.filter_map(|entry| entry.ok()) {
                let entry_path = dir_entry.path();
                if entry_path.is_dir() {
                    continue;
                }

                let mut file_name = entry_path.file_name().unwrap().to_str().unwrap();

                // Strip "lib" prefix
                if file_name.starts_with("lib") {
                    file_name = &file_name[3..];
                }

                // Get only the prefix of the file name
                if let Some(dot_index) = file_name.find('.') {
                    file_name = &file_name[..dot_index];
                }

                index.insert(file_name.to_string(), entry_path);
            }
        }

        Self { inner: index }
    }

    #[inline]
    fn try_find(&self, library_name: &str) -> Option<&PathBuf> {
        self.inner.get(library_name)
    }

    #[inline]
    fn find(&self, library_name: &str) -> Result<&PathBuf> {
        if let Some(path) = self.try_find(library_name) {
            return Ok(path);
        }

        let mut closest_matches = self
            .inner
            .iter()
            .filter(|(name, _path)| levenshtein::levenshtein(name, library_name) <= 2)
            .take(3)
            .collect::<Vec<_>>();

        // Sort them by proximity, just to make it a little easier.
        closest_matches.sort_by_key(|(name, _path)| levenshtein::levenshtein(name, library_name));

        let mut diag = diagnostic!("could not find library `{library_name}`");
        for (name, path) in closest_matches {
            diag = diag.with_help(format!("did you mean {name}? ({})", path.display()));
        }

        Err(diag.into())
    }
}
