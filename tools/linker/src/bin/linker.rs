use std::ffi::OsStr;
use std::path::PathBuf;

use clap::Parser;
use clap::error::ContextValue;
use linker::InputFile;
use lume_errors::{DiagCtx, MapDiagnostic, Result};

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

    /// Name of the entry point symbol
    #[arg(long, value_name = "ENTRY")]
    pub entry: Option<String>,

    /// Initial stack memory size
    #[arg(long, value_name = "SIZE", value_parser = HexParser)]
    pub stack_size: Option<u64>,

    /// Print the output entries before writing the output file
    #[arg(long)]
    pub print_entries: bool,
}

fn main() {
    let args = Arguments::parse();
    let dcx = DiagCtx::new();

    let config = linker::Config {
        entry: args.entry,
        search_paths: None,
        libraries: Vec::new(),
        stack_size: args.stack_size,
        print_entries: args.print_entries,
    };

    dcx.with_opt(|_dcx| {
        let inputs = read_input_files(args.inputs)?;
        let linked = linker::link(config, inputs)?;

        std::fs::write(&args.output, linked).map_cause("could not write output file")?;

        Ok(())
    });

    let mut renderer = error_snippet::GraphicalRenderer::new();
    renderer.padding = 2;
    renderer.use_colors = true;
    renderer.highlight_source = true;

    dcx.render_stderr(&mut renderer);
    dcx.clear();
}

fn read_input_files<'data>(inputs: Vec<PathBuf>) -> Result<Vec<InputFile<'data>>> {
    let mut files = Vec::new();

    for path in inputs {
        let content = std::fs::read(&path).map_cause(format!("could not read input file {}", path.display()))?;

        files.push(InputFile {
            path,
            content: std::borrow::Cow::Owned(content),
        });
    }

    Ok(files)
}
