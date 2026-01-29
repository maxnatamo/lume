use std::path::PathBuf;

use indexmap::IndexSet;
use lume_errors::{MapDiagnostic, Result, diagnostic};

use crate::Config;

const GCC_LIB_PATH: &str = "/lib/gcc/";

pub const ALLOWED_EXTENSIONS: &[&str] = &["a", "so", "o"];

fn is_command_available(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--help")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut proc| proc.wait())
        .is_ok_and(|exit| exit.success())
}

pub fn default_libraries(config: &Config) -> IndexSet<String> {
    let mut libs = IndexSet::new();

    libs.insert(String::from("Scrt1"));
    libs.insert(String::from("crti"));
    libs.insert(String::from("crtbeginS"));
    libs.insert(String::from("gcc"));
    libs.insert(String::from("gcc_s"));
    libs.insert(String::from("crtendS"));
    libs.insert(String::from("crtn"));

    libs.extend(config.libraries.clone());

    libs
}

pub fn search_paths() -> Result<IndexSet<PathBuf>> {
    let mut search_paths = IndexSet::new();

    if is_command_available("ld") {
        search_paths.extend(ld_search_paths()?);
    }

    if PathBuf::from(GCC_LIB_PATH).exists() {
        search_paths.extend(gcc_search_paths());
    }

    Ok(search_paths)
}

fn ld_search_paths() -> Result<IndexSet<PathBuf>> {
    static SEARCH_PATH_PATTERN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"SEARCH_DIR\("=(?<path>[a-zA-Z0-9-_/]+)"\);"#).unwrap());

    let mut command = std::process::Command::new("ld");
    command.args(["--verbose"]);

    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let output = match command.output() {
        Ok(output) => output,
        Err(err) => return Err(diagnostic!("could not invoke ld: {err}").into()),
    };

    if !output.status.success() {
        return Err(diagnostic!(
            "ld exited with status code {}\n\n{}",
            output.status.code().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
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

fn gcc_search_paths() -> IndexSet<PathBuf> {
    let mut search_paths = IndexSet::new();

    let Ok(glob_paths) = glob::glob(&format!("{GCC_LIB_PATH}/*-linux-*/*")) else {
        return search_paths;
    };

    for entry_path in glob_paths.filter_map(|entry| entry.ok()) {
        if entry_path.is_dir() {
            search_paths.insert(entry_path);
        }
    }

    search_paths
}
