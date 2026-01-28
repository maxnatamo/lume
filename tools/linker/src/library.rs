use std::path::PathBuf;

use error_snippet::Severity;
use indexmap::IndexSet;
use lume_errors::{Result, SimpleDiagnostic};

use crate::Config;

pub(crate) fn search_libraries(config: &Config) -> Result<Vec<PathBuf>> {
    let library_names = default_libraries(config);
    let search_paths = library_search_paths(config)?;

    let mut lib_files = Vec::new();

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

        lib_files.push(lib_path);
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
