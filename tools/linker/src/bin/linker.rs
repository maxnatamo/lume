#![allow(clippy::disallowed_macros)]

use std::ffi::OsStr;
use std::path::PathBuf;

use clap::Parser;
use clap::error::ContextValue;
use linker::{Endianess, TargetTriple, parse_target_triple};
use lume_errors::{DiagCtx, MapDiagnostic};

#[derive(Clone)]
pub(crate) struct HexParser;

impl clap::builder::TypedValueParser for HexParser {
    type Value = u64;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> std::result::Result<Self::Value, clap::Error> {
        let mut value = value.to_str().expect("invalid unicode");
        let radix = match value.get(0..2) {
            Some("0x" | "0X") => Some(16),
            Some("0o" | "0O") => Some(8),
            Some("0b" | "0B") => Some(2),
            _ => None,
        };

        if radix.is_some() {
            value = &value[2..];
        }

        u64::from_str_radix(value, radix.unwrap_or(10)).map_err(|_| {
            let mut err = clap::Error::new(clap::error::ErrorKind::InvalidValue).with_cmd(cmd);

            if let Some(arg) = arg {
                err.insert(
                    clap::error::ContextKind::InvalidArg,
                    ContextValue::String(arg.to_string()),
                );
            }

            err.insert(
                clap::error::ContextKind::InvalidValue,
                ContextValue::String(value.to_owned()),
            );

            err
        })
    }
}

#[derive(Clone)]
pub(crate) struct TargetTripleParser;

impl clap::builder::TypedValueParser for TargetTripleParser {
    type Value = TargetTriple;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> std::result::Result<Self::Value, clap::Error> {
        let value = value.to_str().expect("invalid unicode");

        match parse_target_triple(value) {
            Ok(triple) => Ok(triple),
            Err(diagnostic) => {
                let mut err = clap::Error::new(clap::error::ErrorKind::InvalidValue).with_cmd(cmd);

                if let Some(arg) = arg {
                    err.insert(
                        clap::error::ContextKind::InvalidArg,
                        ContextValue::String(arg.to_string()),
                    );
                }

                err.insert(
                    clap::error::ContextKind::Custom,
                    ContextValue::String(diagnostic.message()),
                );

                Err(err)
            }
        }
    }
}

#[derive(Debug, Parser)]
#[clap(
    name = "linker",
    version = env!("CARGO_PKG_VERSION"),
    about = "Object linker for ELF, MachO and PE",
    long_about = None
)]
#[command(arg_required_else_help(true))]
pub(crate) struct Arguments {
    /// List of inputs files
    #[arg(value_name = "INPUTS", value_hint = clap::ValueHint::FilePath)]
    pub inputs: Vec<PathBuf>,

    /// Path to output file
    #[arg(short = 'o', long, value_name = "OUTPUT", value_hint = clap::ValueHint::FilePath, required = true)]
    pub output: PathBuf,

    /// Search the given library when linking.
    #[arg(short = 'l', value_name = "LIB")]
    pub libraries: Vec<String>,

    /// Add directory to library search path
    #[arg(short = 'L', value_name = "DIR")]
    pub search_paths: Vec<PathBuf>,

    /// Name of the entry point symbol
    #[arg(long, value_name = "ENTRY")]
    pub entry: Option<String>,

    /// Target triple
    #[arg(long, value_name = "TRIPLE", value_parser = TargetTripleParser)]
    pub target: Option<TargetTriple>,

    /// Endianess of the linked file
    #[arg(long, value_name = "ENDIAN", value_parser = ["little", "big"])]
    pub endian: Option<String>,

    /// Initial stack memory size
    #[arg(long, value_name = "SIZE", value_parser = HexParser)]
    pub stack_size: Option<u64>,

    /// Print the output entries before writing the output file
    #[arg(long)]
    pub print_entries: bool,

    /// Print the search paths and exit
    #[arg(long)]
    pub print_search_paths: bool,
}

fn main() {
    let args = Arguments::parse();
    let dcx = DiagCtx::new();

    let config = linker::Config {
        entry: args.entry,
        search_paths: args.search_paths,
        libraries: args.libraries,
        stack_size: args.stack_size,
        print_entries: args.print_entries,
        target_triple: args.target,
        endianess: match args.endian.map(|e| e.to_ascii_lowercase()).as_deref() {
            Some("little") => Some(Endianess::Little),
            Some("big") => Some(Endianess::Big),
            Some(_) => unreachable!(),
            _ => None,
        },
    };

    if args.print_search_paths {
        let search_paths = linker::search_paths(&config).unwrap_or_default();
        for path in search_paths {
            println!("{}", path.display());
        }
        return;
    }

    dcx.with_opt(|dcx| {
        let inputs = args.inputs;
        let linked = linker::link(config, inputs, &dcx)?;

        std::fs::write(&args.output, linked).map_cause("could not write output file")?;

        #[cfg(unix)]
        if let Ok(metadata) = std::fs::metadata(&args.output) {
            use std::os::unix::fs::PermissionsExt;

            use lume_errors::{Severity, SimpleDiagnostic};

            let mut perms = metadata.permissions();

            // read/execute for owner
            perms.set_mode(perms.mode() | 0o500);

            if let Err(err) = std::fs::set_permissions(&args.output, perms) {
                dcx.emit_and_push(
                    SimpleDiagnostic::new("could not set output as executable")
                        .with_severity(Severity::Warning)
                        .add_cause(err)
                        .into(),
                );
            }
        }

        Ok(())
    });

    let mut renderer = error_snippet::GraphicalRenderer::new();
    renderer.padding = 2;
    renderer.use_colors = true;
    renderer.highlight_source = true;

    dcx.render_stderr(&mut renderer);
    dcx.clear();
}
