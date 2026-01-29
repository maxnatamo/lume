use std::path::PathBuf;

use indexmap::IndexSet;
use lume_errors::{Result, diagnostic};

use crate::Config;

pub const ALLOWED_EXTENSIONS: &[&str] = &["o", "a", "dylib", "tbd"];

pub fn default_libraries(config: &Config) -> IndexSet<String> {
    let mut libs = IndexSet::new();

    libs.insert(String::from("c"));
    libs.insert(String::from("m"));
    libs.insert(String::from("System"));

    libs.extend(config.libraries.clone());

    libs
}

pub fn search_path() -> Result<PathBuf> {
    use std::process::{Command, Stdio};

    use lume_errors::MapDiagnostic;

    let mut command = Command::new("xcrun");
    command.args(["--sdk", "macosx", "--show-sdk-path"]);

    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let output = match command.output() {
        Ok(output) => output,
        Err(err) => return Err(diagnostic!("could not invoke xcrun: {err}").into()),
    };

    if !output.status.success() {
        return Err(diagnostic!(
            "xcrun exited with status code {}\n\n{}",
            output.status.code().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }

    let path = String::from_utf8(output.stdout).map_cause("could not read output of xcrun")?;

    Ok(PathBuf::from(path.trim()).join("usr/lib"))
}
