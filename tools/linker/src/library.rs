use std::path::PathBuf;

use indexmap::{IndexMap, IndexSet};
use lume_errors::{Result, diagnostic};

use crate::Config;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
mod linux;

pub(crate) fn search_libraries(config: &Config) -> Result<Vec<PathBuf>> {
    let allowed_extensions = allow_library_extensions();
    let search_paths = search_paths(config)?;

    let index = LibraryIndex::create(&search_paths, allowed_extensions);

    default_libraries(config)
        .into_iter()
        .map(|library_name| index.find(&library_name).cloned())
        .collect()
}

fn default_libraries(config: &Config) -> IndexSet<String> {
    #[cfg(target_os = "linux")]
    return linux::default_libraries(config);

    #[cfg(target_os = "macos")]
    return macos::default_libraries(config);
}

fn allow_library_extensions() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    return linux::ALLOWED_EXTENSIONS;

    #[cfg(target_os = "macos")]
    return macos::ALLOWED_EXTENSIONS;
}

pub fn search_paths(config: &Config) -> Result<Vec<PathBuf>> {
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
        paths.extend(linux::search_paths()?);
    }

    #[cfg(target_os = "macos")]
    {
        paths.push(macos::search_path()?);
    }

    #[cfg(target_os = "windows")]
    {
        paths.extend(split_env_paths("LIB"));
    }

    Ok(paths)
}

#[derive(Default)]
struct LibraryIndex {
    inner: IndexMap<String, PathBuf>,
}

impl LibraryIndex {
    fn create(search_paths: &[PathBuf], allowed_extensions: &[&str]) -> Self {
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
                let Some(extension) = unversioned_extension(file_name) else {
                    continue;
                };

                if !allowed_extensions.contains(&extension) {
                    continue;
                }

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

/// Extracts the unversioned extension from a file name.
///
/// Returns [`None`] if no extension was found or if the extension is not
/// alphabetic.
///
/// # Examples
///
/// ```
/// assert_eq!(unversioned_extension("libc.a"), Some("a"));
/// assert_eq!(unversioned_extension("libfoo.so.3.1.10"), Some("so"));
/// assert_eq!(unversioned_extension("libbar.G4.o"), Some("o"));
/// assert_eq!(unversioned_extension("libbaz.threads.dylib"), Some("dylib"));
/// assert_eq!(unversioned_extension("cpp"), None);
/// ```
fn unversioned_extension(file_name: &str) -> Option<&str> {
    let mut file_name_component_iter = file_name.split('.');

    // Skip the file prefix before the first period
    file_name_component_iter.next();

    file_name_component_iter.rfind(|comp| comp.chars().all(|c| c.is_alphabetic()))
}
