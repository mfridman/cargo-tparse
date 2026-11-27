//! Render a report as text. The text can contain ANSI styles. The caller
//! writes it through a stream that removes the styles when color is off.

use std::cmp::Ordering;
use std::fmt::Write;

use anstyle::{AnsiColor, Style};
use unicode_width::UnicodeWidthStr;

use crate::ansi;
use crate::package;
use crate::parse::{Report, Status, Target, Test};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    Markdown,
}

/// What one row of the summary table shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rows {
    Target,
    Package,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// By package name, then by target name.
    Name,
    /// The slowest first.
    Elapsed,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub format: Format,
    pub rows: Rows,
    pub sort: Sort,
    /// Show this many of the slowest passed tests of each package. Zero
    /// shows none.
    pub slow: usize,
}

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();

pub fn report(report: &Report, options: Options) -> String {
    let mut out = String::new();
    failures(&mut out, report, options.format);
    let (slow, timed) = slow_table(report, options.sort, options.slow);
    if !slow.rows.is_empty() {
        if slow.rows.len() < timed {
            let tests = if options.slow == 1 { "test" } else { "tests" };
            let note = format!(
                "Slowest {} passed {tests} of each package ({} of {timed} shown):",
                options.slow,
                slow.rows.len(),
            );
            match options.format {
                Format::Table => {
                    let _ = writeln!(out, "{DIM}{note}{DIM:#}");
                }
                Format::Markdown => {
                    let _ = writeln!(out, "{note}\n");
                }
            }
        }
        render_table(&mut out, &slow, options.format);
        out.push('\n');
    }
    let table = match options.rows {
        Rows::Target => target_table(report, options.sort),
        Rows::Package => package_table(report, options.sort),
    };
    if !table.rows.is_empty() {
        render_table(&mut out, &table, options.format);
    }
    totals(&mut out, report);
    // libtest gives the time of each test only with --report-time.
    let no_times = options.slow > 0 && report.tests().all(|(_, t)| t.elapsed.is_none());
    hints(&mut out, report, options.format, no_times);
    out
}

fn render_table(out: &mut String, table: &Table, format: Format) {
    match format {
        Format::Table => table.render_box(out),
        Format::Markdown => table.render_markdown(out),
    }
}

/// One line for a target that finished, for --progress.
pub fn progress_line(target: &Target) -> String {
    let status = status_cell(target.status());
    let failed = match target.count(Status::Fail) + target.count(Status::Abort) {
        0 => String::new(),
        n => format!("  {n} failed"),
    };
    let elapsed = target
        .elapsed()
        .map(|s| format!("  {s:.2}s"))
        .unwrap_or_default();
    format!(
        "{}  {}{failed}{elapsed}",
        status.padded(5, false),
        place(target)
    )
}

fn failures(out: &mut String, report: &Report, format: Format) {
    for target in &report.targets {
        if target.status() == Status::Pass {
            continue;
        }
        let place = place(target);
        let output = trim_blank_lines(&target.output);
        let failed: Vec<&Test> = target
            .tests
            .iter()
            .filter(|t| matches!(t.status, Status::Fail | Status::Abort))
            .collect();
        for test in &failed {
            let mut body = test.output.trim_matches('\n').to_string();
            if test.status == Status::Abort {
                body = "No result. The test binary stopped while this test ran.".to_string();
                push_exit(&mut body, target);
            }
            let rerun = rerun_command(target, test);
            block(
                out,
                format,
                test.status,
                Some(&test.name),
                &place,
                &body,
                Some(&rerun),
            );
        }
        if failed.is_empty() {
            // The binary failed with no failed test, for example a crash
            // before the first test.
            let mut body = match target.status() {
                Status::Abort => "The test binary stopped before it finished.",
                _ => "The test binary failed, but no test failed.",
            }
            .to_string();
            push_exit(&mut body, target);
            if !output.is_empty() {
                body.push_str("\n\n");
                body.push_str(&output.join("\n"));
            }
            let rerun = target.rerun_args().map(|args| format!("cargo test {args}"));
            block(
                out,
                format,
                target.status(),
                None,
                &place,
                &body,
                rerun.as_deref(),
            );
        } else {
            // Output that no test result has, for example with --nocapture,
            // and a crash after a test failed.
            let mut extra: Vec<String> = Vec::new();
            if let (Some(reason), false) = (&target.exit_reason, target.count(Status::Abort) > 0) {
                extra.push(format!("Exit: {reason}"));
            }
            extra.extend(output.iter().cloned());
            if !extra.is_empty() {
                more(out, format, &place, &extra.join("\n"));
            }
        }
    }
}

fn more(out: &mut String, format: Format, place: &str, text: &str) {
    match format {
        Format::Markdown => {
            let place = markdown_escape(place);
            let _ = writeln!(out, "More from {place}:\n\n{}\n", fenced(text));
        }
        Format::Table => {
            let _ = writeln!(out, "{DIM}More from {place}:{DIM:#}\n");
            indented(out, text);
            out.push('\n');
        }
    }
}

fn push_exit(body: &mut String, target: &Target) {
    if let Some(reason) = &target.exit_reason {
        body.push_str(&format!("\nExit: {reason}"));
    }
}

/// One failure: a status line, the output, and a rerun command.
fn block(
    out: &mut String,
    format: Format,
    status: Status,
    name: Option<&str>,
    place: &str,
    body: &str,
    rerun: Option<&str>,
) {
    match format {
        Format::Markdown => {
            let status = status_text(status);
            match name {
                Some(name) => {
                    let place = markdown_escape(place);
                    let _ = writeln!(out, "**{status}** {} ({place})\n", code_span(name));
                }
                None => {
                    let _ = writeln!(out, "**{status}** {}\n", code_span(place));
                }
            }
            if !body.is_empty() {
                let _ = writeln!(out, "{}\n", fenced(body));
            }
            if let Some(rerun) = rerun {
                let _ = writeln!(out, "Rerun: {}\n", code_span(rerun));
            }
        }
        Format::Table => {
            let status = status_cell(status).padded(0, false);
            match name {
                Some(name) => {
                    let _ = writeln!(out, "{status}  {BOLD}{name}{BOLD:#}  {DIM}{place}{DIM:#}\n");
                }
                None => {
                    let _ = writeln!(out, "{status}  {BOLD}{place}{BOLD:#}\n");
                }
            }
            if !body.is_empty() {
                indented(out, body);
                out.push('\n');
            }
            if let Some(rerun) = rerun {
                let _ = writeln!(out, "    {DIM}rerun:{DIM:#} {rerun}\n");
            }
        }
    }
}

fn indented(out: &mut String, text: &str) {
    for line in text.lines() {
        let _ = writeln!(out, "{}", format!("    {line}").trim_end());
    }
}

fn trim_blank_lines(lines: &[String]) -> &[String] {
    let blank = |line: &String| line.trim().is_empty();
    let start = lines.iter().position(|l| !blank(l)).unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|l| !blank(l))
        .map_or(start, |i| i + 1);
    &lines[start..end]
}

/// The longest run of backticks in the text.
fn longest_backtick_run(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// Escape text for Markdown, so a name like "__api__" shows as it is.
fn markdown_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in ansi::printable(text).chars() {
        if "\\`*_[]<>|~#".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A Markdown code span that shows the text as it is, even with backticks.
/// Escape sequences and control characters are removed first: a color code
/// between two backticks must not hide a run of backticks.
fn code_span(text: &str) -> String {
    let text = &ansi::printable(text);
    let ticks = "`".repeat(longest_backtick_run(text) + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{text}{pad}{ticks}")
}

/// A Markdown code block that no line in the text can close.
fn fenced(text: &str) -> String {
    let text = &ansi::printable(text);
    let fence = "`".repeat((longest_backtick_run(text) + 1).max(3));
    format!("{fence}text\n{text}\n{fence}")
}

/// Slowest first. A row with no time comes last.
fn slowest_first(a: Option<f64>, b: Option<f64>) -> Ordering {
    let time = |t: Option<f64>| t.unwrap_or(f64::NEG_INFINITY);
    time(b).total_cmp(&time(a))
}

fn target_table(report: &Report, sort: Sort) -> Table {
    let mut targets: Vec<&Target> = report.targets.iter().filter(|t| !t.is_empty()).collect();
    targets.sort_by(|a, b| (package(a), &a.label).cmp(&(package(b), &b.label)));
    if sort == Sort::Elapsed {
        // A stable sort: rows with the same time stay sorted by name.
        targets.sort_by(|a, b| slowest_first(a.elapsed(), b.elapsed()));
    }
    let show_filtered = targets.iter().any(|t| t.filtered_out() > 0);
    let mut headers = vec![
        "Status", "Elapsed", "Package", "Target", "Pass", "Fail", "Skip",
    ];
    if show_filtered {
        headers.push("Filtered");
    }
    let mut table = Table::new(&headers, &[1, 4, 5, 6, 7]);
    for target in &targets {
        let failed = target.count(Status::Fail) + target.count(Status::Abort);
        let mut row = vec![
            status_cell(target.status()),
            Cell::plain(seconds(target.elapsed())),
            Cell::plain(package(target)),
            Cell::plain(&target.label),
            Cell::plain(target.count(Status::Pass).to_string()),
            Cell::plain(failed.to_string()),
            Cell::plain(target.count(Status::Skip).to_string()),
        ];
        if show_filtered {
            row.push(Cell::plain(target.filtered_out().to_string()));
        }
        table.rows.push(row);
    }
    table
}

fn sorted_packages(report: &Report, sort: Sort) -> Vec<package::Package<'_>> {
    let targets: Vec<&Target> = report.targets.iter().filter(|t| !t.is_empty()).collect();
    let mut packages = package::packages(&targets);
    packages.sort_by(|a, b| a.name.unwrap_or("-").cmp(b.name.unwrap_or("-")));
    if sort == Sort::Elapsed {
        packages.sort_by(|a, b| slowest_first(a.elapsed(), b.elapsed()));
    }
    packages
}

/// The slowest passed tests of each package, the slowest first. Packages
/// are in the same order as in the package table. It also returns the number
/// of passed tests that have a time, shown or not.
fn slow_table(report: &Report, sort: Sort, count: usize) -> (Table, usize) {
    let mut table = Table::new(&["Elapsed", "Package", "Target", "Test"], &[0]);
    let mut timed = 0;
    if count == 0 {
        return (table, timed);
    }
    for package in sorted_packages(report, sort) {
        let mut tests: Vec<(&Target, &Test, f64)> = package
            .targets
            .iter()
            .flat_map(|target| target.tests.iter().map(move |test| (*target, test)))
            .filter(|(_, test)| test.status == Status::Pass)
            .filter_map(|(target, test)| Some((target, test, test.elapsed?)))
            .collect();
        timed += tests.len();
        // A stable sort: tests with the same time keep the order they ran in.
        tests.sort_by(|a, b| b.2.total_cmp(&a.2));
        for (target, test, elapsed) in tests.into_iter().take(count) {
            table.rows.push(vec![
                Cell::plain(seconds(Some(elapsed))),
                Cell::plain(package.name.unwrap_or("-")),
                Cell::plain(&target.label),
                Cell::plain(&test.name),
            ]);
        }
    }
    (table, timed)
}

fn package_table(report: &Report, sort: Sort) -> Table {
    let packages = sorted_packages(report, sort);
    let show_filtered = packages.iter().any(|p| p.filtered_out() > 0);
    let mut headers = vec![
        "Status", "Elapsed", "Package", "Targets", "Pass", "Fail", "Skip",
    ];
    if show_filtered {
        headers.push("Filtered");
    }
    let mut table = Table::new(&headers, &[1, 3, 4, 5, 6, 7]);
    for package in &packages {
        let failed = package.count(Status::Fail) + package.count(Status::Abort);
        let mut row = vec![
            status_cell(package.status()),
            Cell::plain(seconds(package.elapsed())),
            Cell::plain(package.name.unwrap_or("-")),
            Cell::plain(package.targets.len().to_string()),
            Cell::plain(package.count(Status::Pass).to_string()),
            Cell::plain(failed.to_string()),
            Cell::plain(package.count(Status::Skip).to_string()),
        ];
        if show_filtered {
            row.push(Cell::plain(package.filtered_out().to_string()));
        }
        table.rows.push(row);
    }
    table
}

/// The line with the totals, for example "3 passed, 1 failed, 0 skipped".
fn totals(out: &mut String, report: &Report) {
    let count = |status| report.tests().filter(|(_, t)| t.status == status).count();
    let failed = count(Status::Fail) + count(Status::Abort);
    // fold, not sum: an empty f64 sum is -0.0, which prints as "-0.00s".
    let elapsed = report
        .targets
        .iter()
        .filter_map(|t| t.elapsed())
        .fold(0.0, |total, t| total + t);
    let filtered: u64 = report.targets.iter().map(|t| t.filtered_out()).sum();
    let filtered = match filtered {
        0 => String::new(),
        n => format!(", {n} filtered out"),
    };
    let _ = writeln!(
        out,
        "\n{} passed, {failed} failed, {} skipped{filtered} in {elapsed:.2}s",
        count(Status::Pass),
        count(Status::Skip),
    );
}

fn hints(out: &mut String, report: &Report, format: Format, no_times: bool) {
    let mut hints = Vec::new();
    if no_times {
        hints.push("Test times need libtest's --report-time option. Add it after --format json.");
    }
    if report.stopped_early {
        hints.push(
            "Cargo stopped after the first failed target. Add --no-fail-fast to run all targets.",
        );
    }
    let nightly = "only accepted on the nightly compiler";
    if report
        .targets
        .iter()
        .any(|t| t.output.iter().any(|l| l.contains(nightly)))
    {
        hints.push("JSON test output needs a nightly toolchain. Use cargo +nightly test.");
    }
    if report.targets.iter().any(|t| t.is_anonymous()) {
        hints.push("Package and target names come from cargo's stderr. Pipe it with 2>&1, and do not use -q.");
    }
    for hint in hints {
        match format {
            Format::Markdown => {
                let _ = writeln!(out, "\n> {hint}");
            }
            Format::Table => {
                let _ = writeln!(out, "{DIM}{hint}{DIM:#}");
            }
        }
    }
}

fn rerun_command(target: &Target, test: &Test) -> String {
    let name = shell_quote(&test.name);
    // rustdoc matches no doc test with --exact. The full doc test name, for
    // example "src/lib.rs - add (line 1)", is specific enough without it.
    let exact = if target.is_doc() { "" } else { " --exact" };
    match target.rerun_args() {
        Some(args) => format!("cargo test {args} -- {name}{exact}"),
        None => format!("cargo test -- {name}{exact}"),
    }
}

fn shell_quote(text: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_:-./".contains(c);
    if !text.is_empty() && text.chars().all(safe) {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// The package and target, for example "alpha-core lib".
fn place(target: &Target) -> String {
    match &target.package {
        Some(package) => format!("{package} {}", target.label),
        None => target.label.clone(),
    }
}

fn package(target: &Target) -> &str {
    target.package.as_deref().unwrap_or("-")
}

fn seconds(elapsed: Option<f64>) -> String {
    match elapsed {
        Some(seconds) => format!("{seconds:.2}s"),
        None => "-".to_string(),
    }
}

fn status_text(status: Status) -> &'static str {
    match status {
        Status::Pass => "PASS",
        Status::Fail => "FAIL",
        Status::Skip => "SKIP",
        Status::Abort => "ABORT",
    }
}

fn status_cell(status: Status) -> Cell {
    let color = match status {
        Status::Pass => AnsiColor::Green,
        Status::Fail => AnsiColor::Red,
        Status::Skip => AnsiColor::Yellow,
        Status::Abort => AnsiColor::Magenta,
    };
    let style = Style::new().fg_color(Some(color.into())).bold();
    Cell::styled(status_text(status), style)
}

struct Cell {
    text: String,
    style: Style,
}

impl Cell {
    fn plain(text: impl Into<String>) -> Self {
        Self::styled(text, Style::new())
    }

    fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    fn padded(&self, width: usize, right: bool) -> String {
        let pad = " ".repeat(width.saturating_sub(self.text.width()));
        let style = self.style;
        if right {
            format!("{pad}{style}{}{style:#}", self.text)
        } else {
            format!("{style}{}{style:#}{pad}", self.text)
        }
    }
}

struct Table {
    headers: Vec<String>,
    right: Vec<bool>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    /// A table with the given columns aligned to the right.
    fn new(headers: &[&str], right: &[usize]) -> Self {
        let right = (0..headers.len()).map(|i| right.contains(&i)).collect();
        Self {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            right,
            rows: Vec::new(),
        }
    }

    fn widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.width()).collect();
        for row in &self.rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.text.width());
            }
        }
        widths
    }

    fn render_box(&self, out: &mut String) {
        let widths = self.widths();
        let rule = |left: &str, middle: &str, right: &str| {
            let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            format!("{left}{}{right}\n", parts.join(middle))
        };
        out.push_str(&rule("┌", "┬", "┐"));
        let headers: Vec<Cell> = self.headers.iter().map(|h| Cell::styled(h, BOLD)).collect();
        self.box_row(out, &headers, &widths);
        out.push_str(&rule("├", "┼", "┤"));
        for row in &self.rows {
            self.box_row(out, row, &widths);
        }
        out.push_str(&rule("└", "┴", "┘"));
    }

    fn box_row(&self, out: &mut String, row: &[Cell], widths: &[usize]) {
        let cells: Vec<String> = row
            .iter()
            .zip(widths)
            .zip(&self.right)
            .map(|((cell, width), right)| cell.padded(*width, *right))
            .collect();
        let _ = writeln!(out, "│ {} │", cells.join(" │ "));
    }

    fn render_markdown(&self, out: &mut String) {
        let headers: Vec<String> = self.headers.iter().map(|h| markdown_escape(h)).collect();
        let _ = writeln!(out, "| {} |", headers.join(" | "));
        let aligns: Vec<&str> = self
            .right
            .iter()
            .map(|right| if *right { "---:" } else { "---" })
            .collect();
        let _ = writeln!(out, "| {} |", aligns.join(" | "));
        for row in &self.rows {
            let cells: Vec<String> = row.iter().map(|c| markdown_escape(&c.text)).collect();
            let _ = writeln!(out, "| {} |", cells.join(" | "));
        }
    }
}
