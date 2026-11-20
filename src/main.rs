mod ansi;
mod parse;
mod render;

use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;

use anstream::{AutoStream, ColorChoice};
use clap::{Parser as _, ValueEnum};

use crate::parse::{Parser, Report};
use crate::render::Format;

const RUN: &str =
    "cargo +nightly test --no-fail-fast -- -Z unstable-options --format json 2>&1 | cargo tparse";

#[derive(Debug, clap::Parser)]
#[command(
    name = "cargo-tparse",
    bin_name = "cargo tparse",
    version,
    about = "Summarize cargo test output in tables",
    after_help = format!("Example:\n  {RUN}")
)]
struct Args {
    /// Output format
    #[arg(long, value_enum, default_value_t = FormatArg::Table)]
    format: FormatArg,

    /// Print one line for each target when it finishes
    #[arg(long)]
    progress: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum FormatArg {
    Table,
    Markdown,
}

fn main() -> ExitCode {
    // Cargo runs "cargo-tparse tparse [args]" for "cargo tparse [args]".
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "tparse") {
        args.remove(1);
    }
    let args = Args::parse_from(args);
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        // For example "cargo tparse | head". The results are not known.
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::from(1),
        Err(err) => {
            eprintln!("cargo-tparse: {err}");
            ExitCode::from(2)
        }
    }
}

/// Run returns false when a target failed or the input has no test events.
fn run(args: &Args) -> io::Result<bool> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Err(io::Error::other(format!(
            "no input; pipe test output to cargo tparse:\n  {RUN}"
        )));
    }
    // Colors are on for a terminal and off for a pipe or file. NO_COLOR and
    // CLICOLOR_FORCE change this.
    let mut stdout = AutoStream::new(io::stdout(), ColorChoice::Auto);

    let report = read(stdin.lock(), args.progress, &mut stdout)?;
    if report.targets.is_empty() {
        // Usually a build error. Show what cargo printed.
        for line in &report.other_lines {
            writeln!(stdout, "{line}")?;
        }
        eprintln!("cargo-tparse: no test events found. Run:\n  {RUN}");
        return Ok(false);
    }

    if args.progress && report.targets.iter().any(|t| !t.is_empty()) {
        writeln!(stdout)?;
    }
    let format = match args.format {
        FormatArg::Table => Format::Table,
        FormatArg::Markdown => Format::Markdown,
    };
    stdout.write_all(render::report(&report, format).as_bytes())?;
    Ok(!report.failed())
}

fn read(mut input: impl BufRead, progress: bool, stdout: &mut impl Write) -> io::Result<Report> {
    let mut parser = Parser::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let finished = if input.read_until(b'\n', &mut buf)? == 0 {
            None
        } else {
            let line = String::from_utf8_lossy(&buf);
            Some(parser.line(line.trim_end_matches(['\n', '\r'])))
        };
        let done = finished.is_none();
        let index = finished.unwrap_or_else(|| parser.flush());
        if let (true, Some(index)) = (progress, index) {
            let target = &parser.report().targets[index];
            if !target.is_empty() {
                writeln!(stdout, "{}", render::progress_line(target))?;
                stdout.flush()?;
            }
        }
        if done {
            return Ok(parser.finish());
        }
    }
}
