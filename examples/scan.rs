//! The protocol scanner as a command: protocol XML in, a Rust module out.
//!
//! ```text
//! cargo run --example scan -- <protocol.xml> [--runtime PATH] [--import PATH]...
//!                             [--source TEXT] [--out FILE]
//! ```
//!
//! `--runtime` is how the generated code names this crate (default
//! `::wayland`); `--import` names a module whose interfaces the protocol
//! refers to (most need `::wayland::protocol::wayland`); `--source` is a
//! note for the header comment; without `--out` the module goes to
//! standard output.

use std::process::ExitCode;
use wayland::scan::{generate, parse, Options};

fn run() -> Result<(), String> {
    let mut options = Options::default();
    let mut input = None;
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{what} needs a value"));
        match arg.as_str() {
            "--runtime" => options.runtime = value("--runtime")?,
            "--import" => options.imports.push(value("--import")?),
            "--source" => options.source = value("--source")?,
            "--out" => out = Some(value("--out")?),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ if input.is_none() => input = Some(arg),
            _ => return Err("more than one input file".to_string()),
        }
    }
    let input = input.ok_or("usage: scan <protocol.xml> [--runtime PATH] [--import PATH]... [--source TEXT] [--out FILE]")?;
    let xml = std::fs::read_to_string(&input).map_err(|e| format!("{input}: {e}"))?;
    let protocol = parse(&xml).map_err(|e| format!("{input}: {e}"))?;
    let code = generate(&protocol, &options);
    match out {
        Some(path) => std::fs::write(&path, code).map_err(|e| format!("{path}: {e}"))?,
        None => print!("{code}"),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("scan: {e}");
            ExitCode::FAILURE
        }
    }
}
