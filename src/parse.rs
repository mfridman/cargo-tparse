//! Parse libtest JSON events, mixed with cargo's stderr lines, into a report.
//!
//! The input is the output of:
//!
//!   cargo test -- -Z unstable-options --format json 2>&1
//!
//! The JSON events do not say which test binary they come from. Cargo prints
//! a "Running" or "Doc-tests" line to stderr before each binary, so when
//! stderr is in the stream we use those lines to name each target.
//!
//! With --nocapture, test output is in the same stream. While a test binary
//! runs, lines that look like cargo lines or suite events are test output.
//! When test output and real events disagree, the last event wins.

use std::collections::HashMap;

use serde::Deserialize;

use crate::ansi;

/// libtest writes each JSON event with this text at the start.
const EVENT_START: &str = "{ \"type\": \"";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Skip,
    Fail,
    /// The test binary stopped before it reported a result.
    Abort,
}

#[derive(Debug)]
pub struct Test {
    pub name: String,
    pub status: Status,
    pub elapsed: Option<f64>,
    pub output: String,
}

/// The end of one libtest suite. Rustdoc can run more than one suite for
/// one target.
#[derive(Debug)]
struct Suite {
    failed: bool,
    filtered_out: u64,
    elapsed: Option<f64>,
}

#[derive(Debug)]
pub struct Target {
    /// Package name, from cargo's "Running" or "Doc-tests" line, or from the
    /// "-p" argument in cargo's rerun hint. When cargo does not give it, it
    /// is a guess from a crate name.
    pub package: Option<String>,
    /// Cargo gave the package name. A guessed name is not used for "-p".
    pub package_known: bool,
    /// Crate name of the test binary, or of the lib for a doc target.
    pub krate: Option<String>,
    /// The target is the bin from src/main.rs.
    pub main_bin: bool,
    /// Short name of the target, for example "lib" or "test api".
    pub label: String,
    /// Cargo arguments that select this target, for example "--test api".
    pub selector: Option<String>,
    /// Arguments that cargo printed to rerun this target after a failure.
    pub rerun: Option<String>,
    /// Why the test binary exited, for example "signal: 6, SIGABRT".
    pub exit_reason: Option<String>,
    /// Lines that are not test events, printed while this target ran. For
    /// example test output with --nocapture, or a message before a crash.
    pub output: Vec<String>,
    pub test_count: u64,
    pub finished: bool,
    /// Cargo said that the test binary failed.
    pub cargo_failed: bool,
    pub tests: Vec<Test>,
    suites: Vec<Suite>,
    /// The target came from a header line, not only from events.
    has_header: bool,
    /// libtest sent "started" events, so a result needs one.
    saw_started: bool,
    /// Index of the first test of the suite that runs now.
    suite_start: usize,
    /// Names before a cargo hint changed them, for when the hint was test
    /// output.
    before_hint: Option<(Option<String>, bool, String, Option<String>)>,
}

impl Target {
    pub fn status(&self) -> Status {
        if !self.finished {
            Status::Abort
        } else if self.cargo_failed
            || self.suites.iter().any(|s| s.failed)
            || self.tests.iter().any(|t| t.status == Status::Fail)
        {
            Status::Fail
        } else {
            Status::Pass
        }
    }

    /// A target that ran no tests and did not fail.
    pub fn is_empty(&self) -> bool {
        self.test_count == 0 && self.tests.is_empty() && self.status() == Status::Pass
    }

    pub fn count(&self, status: Status) -> usize {
        self.tests.iter().filter(|t| t.status == status).count()
    }

    pub fn elapsed(&self) -> Option<f64> {
        let times = self.suites.iter().filter_map(|s| s.elapsed);
        times.reduce(|total, time| total + time)
    }

    pub fn filtered_out(&self) -> u64 {
        self.suites.iter().map(|s| s.filtered_out).sum()
    }

    pub fn is_doc(&self) -> bool {
        self.label == "doc"
    }

    /// The target came from a cargo "Running" or "Doc-tests" line.
    pub fn has_header(&self) -> bool {
        self.has_header
    }

    /// The target has no header line, so it has no names.
    pub fn is_anonymous(&self) -> bool {
        self.label.starts_with('#')
    }

    /// Cargo arguments that run only this target.
    pub fn rerun_args(&self) -> Option<String> {
        if let Some(rerun) = &self.rerun {
            return Some(rerun.clone());
        }
        let selector = self.selector.as_ref()?;
        Some(match (&self.package, self.package_known) {
            (Some(package), true) => format!("-p {package} {selector}"),
            _ => selector.clone(),
        })
    }

    fn new(header: Header, test_count: u64, output: Vec<String>) -> Self {
        let has_header = !header.label.starts_with('#');
        Self {
            package: header.package,
            package_known: header.package_known,
            krate: header.krate,
            main_bin: header.main_bin,
            label: header.label,
            selector: header.selector,
            rerun: None,
            exit_reason: None,
            output,
            test_count,
            finished: false,
            cargo_failed: false,
            tests: Vec::new(),
            suites: Vec::new(),
            has_header,
            saw_started: false,
            suite_start: 0,
            before_hint: None,
        }
    }

    /// Use the target name from a cargo hint, for example "--test my-api".
    fn name_from_hint(&mut self, args: &str) {
        self.before_hint = Some((
            self.package.clone(),
            self.package_known,
            self.label.clone(),
            self.selector.clone(),
        ));
        let (package, selector) = split_package(args);
        // With cargo -q, doc suites with no header can come from more than
        // one package. Then the hint's package is only right for the last.
        let one_package = self.has_header || self.suites.len() <= 1;
        if let (Some(package), true) = (package, one_package) {
            self.package = Some(package.to_string());
            self.package_known = true;
        }
        // Cargo knows the target. The binary name has '_' for '-', and a
        // custom path can make the source path look like another kind.
        if !selector.is_empty() {
            self.label = selector.trim_start_matches("--").to_string();
            self.selector = Some(selector);
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub targets: Vec<Target>,
    /// Lines that are not test events and not from a test binary, for
    /// example compiler errors.
    pub other_lines: Vec<String>,
    /// Cargo stopped after the first failed target (no --no-fail-fast).
    pub stopped_early: bool,
    /// Seconds that cargo took to build, from its "Finished" line.
    pub build_time: Option<f64>,
}

impl Report {
    pub fn tests(&self) -> impl Iterator<Item = (&Target, &Test)> {
        self.targets
            .iter()
            .flat_map(|target| target.tests.iter().map(move |test| (target, test)))
    }

    pub fn failed(&self) -> bool {
        self.targets.iter().any(|t| t.status() != Status::Pass)
    }
}

#[derive(Debug, Default)]
pub struct Parser {
    report: Report,
    /// The last header line, before its suite starts.
    pending: Option<Header>,
    /// Lines that came after the pending header.
    pending_output: Vec<String>,
    /// The target whose suite runs now.
    open: Option<usize>,
    /// The target that gets other lines, from its suite start to the next
    /// header line.
    current: Option<usize>,
    /// The target that a late test event can reopen, up to the next header
    /// line.
    last: Option<usize>,
    /// Rustdoc can run more than one suite after one "Doc-tests" line.
    doc_continues: bool,
    /// A target that finished a suite. It is done when the next cargo line
    /// comes, or at the end of the input.
    done_later: Option<usize>,
    /// Library crate name to package name, for "Doc-tests <crate>" lines.
    packages: HashMap<String, String>,
    saw_failed_summary: bool,
    /// A target that a rerun hint closed while its suite ran: a crash, or a
    /// hint that a test printed.
    crashed: Option<usize>,
    /// The length of other_lines at that hint.
    other_mark: usize,
    /// The text of the pending header line.
    pending_line: String,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn report(&self) -> &Report {
        &self.report
    }

    /// Guess the package names that cargo did not give, from the targets
    /// so far. Call it before a progress line. finish does it again with all
    /// targets.
    pub fn name_packages(&mut self) {
        crate::package::name_unknown(&mut self.report.targets);
    }

    /// Return a cargo status line, for example "Compiling demo v0.1.0", with
    /// no colors. While a test binary runs, such lines are test output.
    /// "Running" and "Doc-tests" are not status lines here: the progress
    /// line of each target shows them.
    pub fn cargo_status(&self, line: &str) -> Option<String> {
        const VERBS: [&str; 10] = [
            "Adding",
            "Blocking",
            "Checking",
            "Compiling",
            "Downloaded",
            "Downloading",
            "Finished",
            "Fresh",
            "Locking",
            "Updating",
        ];
        if self.open.is_some() {
            return None;
        }
        let clean = ansi::strip(line);
        let (verb, rest) = clean.trim().split_once(' ')?;
        (VERBS.contains(&verb) && !rest.trim().is_empty()).then(|| clean.trim_end().to_string())
    }

    /// Call at the end of the input. It returns a target that finished
    /// but was not returned yet.
    pub fn flush(&mut self) -> Option<usize> {
        self.done_later.take()
    }

    pub fn finish(mut self) -> Report {
        // A header with no events after it, at the end of the input.
        // The test binary was still running when the input ended.
        if let Some(header) = self.pending.take() {
            self.push_target(header, 0);
        }
        crate::package::name_unknown(&mut self.report.targets);
        let failed = self.report.targets.iter().any(|t| t.cargo_failed);
        self.report.stopped_early = failed && !self.saw_failed_summary;
        self.report
    }

    /// Handle one line of input. It returns the index of the target that
    /// finished on this line, if any.
    pub fn line(&mut self, line: &str) -> Option<usize> {
        // Find events before removing escape sequences: an escape sequence
        // with no end can take the first character of an event.
        let raw = line.trim();
        if let Some((start, event, text)) = find_event(raw) {
            if start > 0 {
                self.other_line(&ansi::strip(&raw[..start]));
            }
            return self.event(event, text);
        }
        let clean = ansi::strip(line);
        let trimmed = clean.trim();
        let idle = self.open.is_none();
        if let (true, Some(rest)) = (idle, trimmed.strip_prefix("Running ")) {
            let header = Header::running(rest);
            if let (Some(krate), Some(package), "lib") =
                (&header.krate, &header.package, header.label.as_str())
            {
                self.packages.insert(krate.clone(), package.clone());
            }
            self.pending_line = trimmed.to_string();
            return self.start_header(header);
        }
        if let (true, Some(name)) = (idle, trimmed.strip_prefix("Doc-tests ")) {
            let header = Header::doc(name, self.packages.get(name));
            self.pending_line = trimmed.to_string();
            return self.start_header(header);
        }
        if let Some((before, args)) = rerun_hint(trimmed) {
            if !before.is_empty() {
                self.other_line(before);
            }
            return self.rerun_hint(args);
        }
        // Cargo prints the next lines only after a test binary exits.
        // While one runs, they are test output.
        if idle
            && trimmed.starts_with("error: ")
            && (trimmed.ends_with(" target failed:") || trimmed.ends_with(" targets failed:"))
        {
            self.saw_failed_summary = true;
            self.current = None;
            return self.done_later.take();
        }
        if let (true, Some(seconds)) = (idle, build_time(trimmed)) {
            *self.report.build_time.get_or_insert(0.0) += seconds;
        }
        if let (true, Some(reason)) = (idle, exit_reason(trimmed)) {
            if let Some(target) = self.current.map(|i| &mut self.report.targets[i]) {
                target.exit_reason = Some(reason);
                target.cargo_failed = true;
            }
        } else if !(idle && trimmed == "Caused by:") {
            self.other_line(clean.trim_end());
        }
        None
    }

    fn start_header(&mut self, header: Header) -> Option<usize> {
        self.pending = Some(header);
        self.pending_output.clear();
        self.current = None;
        self.last = None;
        self.doc_continues = false;
        self.done_later.take()
    }

    /// Cargo prints the rerun hint after a test binary exits with a failure.
    /// A test can also print it with --nocapture. Then more events come for
    /// the same target, and they reopen it.
    fn rerun_hint(&mut self, args: &str) -> Option<usize> {
        // The binary failed before its first event, so it has no target yet.
        if let Some(header) = self.pending.take() {
            self.push_target(header, 0);
        }
        // If the binary crashed, its suite is still open.
        if let Some(open) = self.open.take() {
            self.crashed = Some(open);
            self.other_mark = self.report.other_lines.len();
        }
        self.doc_continues = false;
        let index = self.current.or(self.report.targets.len().checked_sub(1));
        if let Some(target) = index.map(|i| &mut self.report.targets[i]) {
            target.rerun = Some(args.to_string());
            target.cargo_failed = true;
            target.name_from_hint(args);
        }
        // A target that finished its suite is done now. A crashed target is
        // done at the next cargo line, unless more events reopen it.
        let finished = self.done_later.take();
        if finished.is_some() && finished == index {
            return finished;
        }
        self.done_later = index;
        finished
    }

    fn other_line(&mut self, line: &str) {
        if let Some(index) = self.current {
            self.report.targets[index].output.push(line.to_string());
        } else if self.pending.is_some() {
            self.pending_output.push(line.to_string());
        } else {
            self.report.other_lines.push(line.to_string());
        }
    }

    fn push_target(&mut self, header: Header, test_count: u64) -> usize {
        self.doc_continues = header.label == "doc";
        let output = std::mem::take(&mut self.pending_output);
        self.report
            .targets
            .push(Target::new(header, test_count, output));
        let index = self.report.targets.len() - 1;
        self.current = Some(index);
        self.last = Some(index);
        index
    }

    fn event(&mut self, event: Event, text: &str) -> Option<usize> {
        match event {
            Event::Suite(suite) if suite.event == "started" => {
                // A suite starts only when no test binary runs. Otherwise
                // this is test output.
                if self.open.is_some() {
                    self.other_line(text);
                    return None;
                }
                let count = suite.test_count.unwrap_or(0);
                let finished = self.done_later.take();
                self.crashed = None;
                let index = if let Some(header) = self.pending.take() {
                    self.push_target(header, count)
                } else if let (true, Some(index)) = (self.doc_continues, self.current) {
                    // One more suite for the same doc target.
                    let target = &mut self.report.targets[index];
                    target.finished = false;
                    target.test_count += count;
                    target.suite_start = target.tests.len();
                    self.open = Some(index);
                    return None;
                } else {
                    let number = self.report.targets.len() + 1;
                    self.push_target(Header::anonymous(number), count)
                };
                self.open = Some(index);
                finished
            }
            Event::Suite(suite) => {
                let Some(index) = self.open else {
                    self.other_line(text);
                    return None;
                };
                let target = &mut self.report.targets[index];
                // libtest ends the suite after all tests finish. If a test
                // has no result, this suite end is test output, unless its
                // counts match the tests with a result: then the tests with
                // no result were printed start events.
                let tests = &target.tests[target.suite_start.min(target.tests.len())..];
                let running = tests.iter().filter(|t| t.status == Status::Abort).count();
                if running > 0 {
                    let counts = [suite.passed, suite.failed, suite.ignored, suite.measured];
                    let reported = counts
                        .into_iter()
                        .map(Option::unwrap_or_default)
                        .sum::<u64>();
                    if reported != (tests.len() - running) as u64 {
                        self.other_line(text);
                        return None;
                    }
                    let start = target.suite_start;
                    let mut i = 0;
                    target.tests.retain(|t| {
                        i += 1;
                        i <= start || t.status != Status::Abort
                    });
                }
                self.open = None;
                target.finished = true;
                target.suites.push(Suite {
                    failed: suite.event == "failed",
                    filtered_out: suite.filtered_out.unwrap_or(0),
                    elapsed: suite.exec_time,
                });
                // With cargo -q there are no "Doc-tests" lines. Doc test
                // names show the doc target, and more suites can follow.
                let doc_names = !target.tests.is_empty() && target.tests.iter().all(is_doc_test);
                if !target.has_header && doc_names {
                    target.label = "doc".to_string();
                    target.selector.get_or_insert_with(|| "--doc".to_string());
                }
                self.doc_continues = target.is_doc();
                // The target is done at the next cargo line. Cargo can still
                // say that the binary failed, and more doc suites can follow.
                self.done_later = Some(index);
                None
            }
            Event::Test(event) => {
                if self.open.is_none() {
                    // A late event for a target that looked crashed, even
                    // after a header line, or for the last target.
                    let index = match (self.crashed, self.pending.is_some()) {
                        (Some(index), _) => index,
                        (None, false) => self.last?,
                        (None, true) => return None,
                    };
                    self.reopen(index);
                }
                let target = &mut self.report.targets[self.open?];
                let status = match event.event.as_str() {
                    "started" => {
                        // A test can print a copy of its own start event.
                        let running = |t: &Test| t.name == event.name && t.status == Status::Abort;
                        if target.tests.iter().any(running) {
                            return None;
                        }
                        target.saw_started = true;
                        target.tests.push(Test {
                            name: event.name,
                            status: Status::Abort,
                            elapsed: None,
                            output: String::new(),
                        });
                        return None;
                    }
                    "ok" => Status::Pass,
                    "failed" => Status::Fail,
                    "ignored" => Status::Skip,
                    // For example "timeout", a warning for a slow test.
                    _ => return None,
                };
                let output = [event.stdout, event.message, event.reason]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n");
                let known = target.tests.iter().any(|t| t.name == event.name);
                if target.saw_started && !known {
                    // libtest starts each test before its result. A result
                    // with no start is test output.
                    self.other_line(text);
                    return None;
                }
                // The last result for a test wins.
                match target.tests.iter_mut().rev().find(|t| t.name == event.name) {
                    Some(test) => {
                        test.status = status;
                        test.elapsed = event.exec_time;
                        test.output = output;
                    }
                    None => target.tests.push(Test {
                        name: event.name,
                        status,
                        elapsed: event.exec_time,
                        output,
                    }),
                }
                None
            }
            // Rustdoc's report comes after all suites of one doc target.
            Event::Report(_) if self.open.is_none() => {
                self.doc_continues = false;
                None
            }
            Event::Bench(_) => None,
            Event::Report(_) => {
                self.other_line(text);
                None
            }
            Event::Other => {
                self.other_line(text);
                None
            }
        }
    }

    /// A test event came after its suite ended, or after a rerun hint. The
    /// earlier end was test output, so the suite still runs. The last suite
    /// end, cargo lines, and hint were not real.
    fn reopen(&mut self, index: usize) {
        if self.crashed.take() == Some(index) {
            // A header line and the lines after the hint were test output.
            let mut lines: Vec<String> = self.report.other_lines.drain(self.other_mark..).collect();
            if self.pending.take().is_some() {
                lines.push(std::mem::take(&mut self.pending_line));
                lines.append(&mut self.pending_output);
            }
            self.report.targets[index].output.extend(lines);
        }
        let target = &mut self.report.targets[index];
        if target.finished {
            target.suites.pop();
        }
        target.finished = false;
        target.cargo_failed = false;
        target.rerun = None;
        target.exit_reason = None;
        if let Some((package, known, label, selector)) = target.before_hint.take() {
            target.package = package;
            target.package_known = known;
            target.label = label;
            target.selector = selector;
        }
        self.doc_continues = target.is_doc();
        // Cargo lines since the suite end were test output too.
        self.saw_failed_summary = false;
        self.current = Some(index);
        self.last = Some(index);
        self.open = Some(index);
        self.done_later = None;
    }
}

/// Rustdoc names doc tests like "src/lib.rs - add (line 1)".
fn is_doc_test(test: &Test) -> bool {
    test.name.contains(" - ") && test.name.contains(" (line ") && test.name.ends_with(')')
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Event {
    Suite(SuiteEvent),
    Test(TestEvent),
    /// The time of all doc tests, at the end of rustdoc's output.
    Report(#[allow(dead_code)] ReportEvent),
    Bench(#[allow(dead_code)] BenchEvent),
    #[serde(other)]
    Other,
}

// The fields make sure that a test's own JSON with the same "type" is not
// taken as an event.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ReportEvent {
    total_time: f64,
    compilation_time: f64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BenchEvent {
    name: String,
    median: f64,
}

#[derive(Debug, Deserialize)]
struct SuiteEvent {
    event: String,
    test_count: Option<u64>,
    passed: Option<u64>,
    failed: Option<u64>,
    ignored: Option<u64>,
    measured: Option<u64>,
    filtered_out: Option<u64>,
    exec_time: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct TestEvent {
    event: String,
    name: String,
    exec_time: Option<f64>,
    stdout: Option<String>,
    message: Option<String>,
    /// For example "time limit exceeded".
    reason: Option<String>,
}

/// Find a JSON event in a line. Test output with no final newline can put
/// text before the event, for example `no newline{ "type": "test", ... }`.
/// It returns where the event starts, the event, and its text.
fn find_event(line: &str) -> Option<(usize, Event, &str)> {
    let first = line.starts_with('{').then_some(0);
    let starts = first
        .into_iter()
        .chain(line.match_indices(EVENT_START).map(|(i, _)| i));
    starts
        .filter_map(|start| {
            let text = &line[start..];
            let event = serde_json::from_str::<Event>(text).ok()?;
            Some((start, event, text))
        })
        .next()
}

#[derive(Debug)]
struct Header {
    package: Option<String>,
    package_known: bool,
    /// Crate name of the test binary, for example "alpha_core".
    krate: Option<String>,
    main_bin: bool,
    label: String,
    selector: Option<String>,
}

impl Header {
    /// Parse the text after "Running", for example
    /// "unittests src/lib.rs (target/debug/deps/foo-0123abcd)".
    fn running(rest: &str) -> Self {
        let rest = rest.strip_prefix("unittests ").unwrap_or(rest);
        let (source, binary) = match rest.rsplit_once(" (") {
            Some((source, binary)) => (source, Some(binary.trim_end_matches(')'))),
            None => (rest, None),
        };
        let (package, krate) = binary.map(binary_names).unwrap_or_default();
        let (label, selector) = target_from_source(source, package.as_deref(), krate.as_deref());
        let source = source.replace('\\', "/");
        Self {
            package_known: package.is_some(),
            package,
            krate,
            main_bin: source == "src/main.rs" || source.ends_with("/src/main.rs"),
            label,
            selector,
        }
    }

    /// A "Doc-tests <crate>" line. The package is known when the lib's
    /// binary path gave it. Otherwise the crate name is the guess.
    fn doc(krate: &str, package: Option<&String>) -> Self {
        Self {
            package: Some(package.map_or(krate, String::as_str).to_string()),
            package_known: package.is_some(),
            krate: Some(krate.to_string()),
            main_bin: false,
            label: "doc".to_string(),
            selector: Some("--doc".to_string()),
        }
    }

    fn anonymous(number: usize) -> Self {
        Self {
            package: None,
            package_known: false,
            krate: None,
            main_bin: false,
            label: format!("#{number}"),
            selector: None,
        }
    }
}

/// Find the package and crate names in a test binary path. Newer cargo puts
/// binaries in ".../build/<package>/<hash>/out/<crate>-<hash>". Older cargo
/// puts them in ".../deps/<crate>-<hash>", which has no package name.
fn binary_names(path: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<&str> = path.split(['/', '\\']).collect();
    let file = parts.last().map(|f| f.trim_end_matches(".exe"));
    let krate = file
        .and_then(|f| f.rsplit_once('-'))
        .map(|(name, _hash)| name.to_string());
    let n = parts.len();
    let package = (n >= 5 && parts[n - 2] == "out" && parts[n - 5] == "build")
        .then(|| parts[n - 4].to_string());
    (package, krate)
}

fn target_from_source(
    source: &str,
    package: Option<&str>,
    krate: Option<&str>,
) -> (String, Option<String>) {
    let normalize = |name: &str| name.replace('-', "_");
    let source = source.replace('\\', "/");
    if source == "src/lib.rs" || source.ends_with("/src/lib.rs") {
        return ("lib".to_string(), Some("--lib".to_string()));
    }
    if source == "src/main.rs" || source.ends_with("/src/main.rs") {
        // The bin name is the package name unless Cargo.toml sets another.
        return match (package, krate) {
            (Some(package), Some(krate)) if normalize(package) != normalize(krate) => {
                (format!("bin {krate}"), Some(format!("--bin {krate}")))
            }
            (Some(package), _) => ("bin".to_string(), Some(format!("--bin {package}"))),
            (None, Some(krate)) => (format!("bin {krate}"), None),
            (None, None) => ("bin".to_string(), None),
        };
    }
    let kinds = [
        ("src/bin/", "bin", "--bin"),
        ("tests/", "test", "--test"),
        ("benches/", "bench", "--bench"),
        ("examples/", "example", "--example"),
    ];
    for (dir, kind, flag) in kinds {
        let rest = source
            .strip_prefix(dir)
            .or_else(|| source.split_once(&format!("/{dir}")).map(|(_, rest)| rest));
        let Some(rest) = rest else { continue };
        let from_source = rest
            .split('/')
            .next()
            .unwrap_or(rest)
            .trim_end_matches(".rs");
        // Cargo.toml can give the target a name that is not the file name.
        // The crate name has '_' for '-', so keep the file name when they
        // match.
        let name = match krate {
            Some(krate) if normalize(krate) != normalize(from_source) => krate,
            _ => from_source,
        };
        return (format!("{kind} {name}"), Some(format!("{flag} {name}")));
    }
    (source, None)
}

/// Find cargo's rerun hint, for example "error: test failed, to rerun pass
/// `--lib`". Test output with no final newline can come before it. It
/// returns that text and the arguments.
fn rerun_hint(line: &str) -> Option<(&str, &str)> {
    let starts = [
        "error: test failed, to rerun pass `",
        "error: doctest failed, to rerun pass `",
    ];
    starts.iter().find_map(|start| {
        let index = line.find(start)?;
        let args = line[index + start.len()..].strip_suffix('`')?;
        Some((&line[..index], args))
    })
}

/// Split "-p alpha --doc" into the package "alpha" and the rest, "--doc".
fn split_package(args: &str) -> (Option<&str>, String) {
    let mut package = None;
    let mut rest = Vec::new();
    let mut words = args.split_whitespace();
    while let Some(word) = words.next() {
        if word == "-p" || word == "--package" {
            package = words.next();
        } else {
            rest.push(word);
        }
    }
    (package, rest.join(" "))
}

/// Take the build time, in seconds, from cargo's line "Finished `test`
/// profile [unoptimized + debuginfo] target(s) in 1m 14s".
fn build_time(line: &str) -> Option<f64> {
    let rest = line.strip_prefix("Finished ")?;
    let (_, time) = rest.rsplit_once(" target(s) in ")?;
    // Cargo writes "29.84s" for less than a minute, and "1m 14s" after.
    match time.split_once("m ") {
        Some((minutes, seconds)) => {
            let minutes: f64 = minutes.parse().ok()?;
            let seconds: f64 = seconds.strip_suffix('s')?.parse().ok()?;
            Some(minutes * 60.0 + seconds)
        }
        None => time.strip_suffix('s')?.parse().ok(),
    }
}

/// Take "signal: 6, SIGABRT: process abort signal" from cargo's
/// "process didn't exit successfully: `...` (signal: 6, ...)" line.
fn exit_reason(line: &str) -> Option<String> {
    let index = line.find("process didn't exit successfully: `")?;
    let (_, reason) = line[index..].rsplit_once("` (")?;
    Some(reason.trim_end_matches(')').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Report {
        let mut parser = Parser::new();
        for line in input.lines() {
            parser.line(line);
        }
        parser.finish()
    }

    fn fixture(name: &str) -> Report {
        let path = format!("{}/tests/testdata/{name}", env!("CARGO_MANIFEST_DIR"));
        parse(&std::fs::read_to_string(path).unwrap())
    }

    fn labels(report: &Report) -> Vec<(Option<&str>, &str, Status)> {
        let targets = report.targets.iter();
        targets
            .map(|t| (t.package.as_deref(), t.label.as_str(), t.status()))
            .collect()
    }

    #[test]
    fn single_crate() {
        let report = fixture("single.txt");
        assert_eq!(
            labels(&report),
            [
                (Some("demo"), "lib", Status::Fail),
                (Some("demo"), "test api", Status::Pass),
                (Some("demo"), "doc", Status::Pass),
            ]
        );
        let lib = &report.targets[0];
        assert_eq!(lib.count(Status::Pass), 1);
        assert_eq!(lib.count(Status::Fail), 1);
        assert_eq!(lib.count(Status::Skip), 1);
        assert_eq!(lib.rerun_args().as_deref(), Some("--lib"));
        let bad = lib.tests.iter().find(|t| t.name == "tests::bad").unwrap();
        assert!(bad.output.contains("some output"));
        assert!(bad.elapsed.is_some());
        assert!(!report.stopped_early);
        assert!(report.failed());
    }

    #[test]
    fn workspace() {
        let report = fixture("workspace.txt");
        assert_eq!(
            labels(&report),
            [
                (Some("alpha-core"), "lib", Status::Fail),
                (Some("beta"), "bin", Status::Pass),
                (Some("alpha-core"), "doc", Status::Pass),
            ]
        );
        let rerun = report.targets[0].rerun_args();
        assert_eq!(rerun.as_deref(), Some("-p alpha-core --lib"));
        assert_eq!(
            report.targets[1].rerun_args().as_deref(),
            Some("-p beta --bin beta")
        );
    }

    #[test]
    fn old_layout_package_names() {
        // Binaries in target/debug/deps have no package name. The rerun
        // hint names alpha-core, so its other targets get that name. The
        // others get the crate name of their lib or src/main.rs bin.
        let report = fixture("workspace-deps.txt");
        assert_eq!(
            labels(&report),
            [
                (Some("alpha-core"), "lib", Status::Pass),
                (Some("alpha-core"), "test api", Status::Pass),
                (Some("alpha-core"), "test cli", Status::Fail),
                (Some("beta"), "bin beta", Status::Pass),
                (Some("beta"), "test cli", Status::Pass),
                (Some("gamma"), "lib", Status::Pass),
                // A src/main.rs bin after a lib with another name starts a
                // new package.
                (Some("delta"), "bin delta", Status::Pass),
                (Some("alpha-core"), "doc", Status::Pass),
                (Some("gamma"), "doc", Status::Pass),
            ]
        );
        // A guessed name can be wrong, for example a crate name with '_'
        // for a package name with '-'. Only cargo's names go in "-p".
        let reruns: Vec<_> = report.targets.iter().map(|t| t.rerun_args()).collect();
        let reruns: Vec<_> = reruns.iter().map(|r| r.as_deref()).collect();
        assert_eq!(
            reruns,
            [
                Some("--lib"),
                Some("--test api"),
                Some("-p alpha-core --test cli"),
                // The bin name can have '-' where the crate name has '_'.
                None,
                Some("--test cli"),
                Some("--lib"),
                None,
                Some("--doc"),
                Some("--doc"),
            ]
        );
    }

    #[test]
    fn hint_with_another_package_keeps_the_guess() {
        // A lib with `test = false` has no unit test binary, so its
        // integration test joins the package before it. Its hint names its
        // own package, which does not rename the lib of the other package.
        let input = r#"Running unittests src/lib.rs (target/debug/deps/alpha-1)
{ "type": "suite", "event": "started", "test_count": 0 }
{ "type": "suite", "event": "ok", "passed": 0, "failed": 0 }
Running tests/api.rs (target/debug/deps/api-2)
{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "event": "failed", "name": "bad" }
{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }
error: test failed, to rerun pass `-p beta --test api`
"#;
        let report = parse(input);
        assert_eq!(
            labels(&report),
            [
                (Some("alpha"), "lib", Status::Pass),
                (Some("beta"), "test api", Status::Fail),
            ]
        );
    }

    #[test]
    fn build_time_from_finished_line() {
        let cases = [
            (
                "Finished `test` profile [unoptimized + debuginfo] target(s) in 0.00s",
                Some(0.0),
            ),
            (
                "Finished `test` profile [unoptimized] target(s) in 29.84s",
                Some(29.84),
            ),
            (
                "Finished `release` profile [optimized] target(s) in 1m 14s",
                Some(74.0),
            ),
            (
                "Finished `test` profile [unoptimized] target(s) in 74m 05s",
                Some(4445.0),
            ),
            ("Finished something else", None),
        ];
        for (line, seconds) in cases {
            assert_eq!(build_time(line), seconds, "{line}");
        }
        // A test that prints the line while it runs does not count.
        let input = r#"    Finished `test` profile [unoptimized] target(s) in 2.50s
{ "type": "suite", "event": "started", "test_count": 1 }
    Finished `test` profile [unoptimized] target(s) in 9.00s
{ "type": "test", "name": "works", "event": "ok" }
{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }
"#;
        assert_eq!(parse(input).build_time, Some(2.5));
    }

    #[test]
    fn stdout_only() {
        let report = fixture("stdout-only.txt");
        assert_eq!(
            labels(&report),
            [
                (None, "#1", Status::Fail),
                (None, "#2", Status::Pass),
                // Doc test names show that this suite is the doc tests.
                (None, "doc", Status::Pass),
            ]
        );
        assert_eq!(report.targets[0].rerun_args(), None);
    }

    #[test]
    fn fail_fast() {
        let report = fixture("fail-fast.txt");
        assert_eq!(report.targets.len(), 1);
        assert!(report.stopped_early);
    }

    #[test]
    fn abort() {
        let report = fixture("abort.txt");
        let crash = &report.targets[1];
        assert_eq!(crash.label, "test crash");
        assert_eq!(crash.status(), Status::Abort);
        assert_eq!(crash.count(Status::Abort), 1);
        assert_eq!(crash.count(Status::Pass), 1);
        let reason = crash.exit_reason.as_deref().unwrap();
        assert!(reason.starts_with("signal: 6"), "{reason}");
        assert_eq!(report.targets[2].status(), Status::Pass);
        assert!(!report.stopped_early);
    }

    #[test]
    fn build_error() {
        let report = fixture("build-error.txt");
        assert!(report.targets.is_empty());
        assert!(
            report
                .other_lines
                .iter()
                .any(|l| l.contains("error[E0308]"))
        );
    }

    #[test]
    fn target_names() {
        let cases = [
            ("src/bin/tool.rs", "bin tool", Some("--bin tool")),
            ("src/bin/tool/main.rs", "bin tool", Some("--bin tool")),
            ("tests/api/main.rs", "test api", Some("--test api")),
            ("benches/speed.rs", "bench speed", Some("--bench speed")),
            ("lib/custom.rs", "lib/custom.rs", None),
        ];
        for (source, label, selector) in cases {
            let (got_label, got_selector) = target_from_source(source, Some("pkg"), None);
            assert_eq!(got_label, label);
            assert_eq!(got_selector.as_deref(), selector);
        }
    }

    #[test]
    fn binary_names_from_paths() {
        let cases = [
            (
                "target/debug/build/alpha-core/df0a/out/alpha_core-df0a",
                Some("alpha-core"),
            ),
            // An earlier directory named "build" is not the layout.
            (
                "/build/checkout/target/debug/build/demo/123/out/demo-123",
                Some("demo"),
            ),
            // The old layout has no package name.
            ("target/debug/deps/alpha_core-0123abcd", None),
            ("target\\debug\\deps\\alpha_core-0123abcd.exe", None),
        ];
        for (path, package) in cases {
            let (got_package, got_krate) = binary_names(path);
            assert_eq!(got_package.as_deref(), package, "{path}");
            assert!(got_krate.is_some(), "{path}");
        }
    }

    #[test]
    fn custom_names_and_failure_shapes() {
        let report = fixture("edge.txt");
        assert_eq!(
            labels(&report),
            [
                (Some("my-tool"), "lib", Status::Fail),
                (Some("my-tool"), "bin mt", Status::Fail),
                (Some("my-tool"), "test it", Status::Fail),
                (Some("my-tool"), "doc", Status::Fail),
            ]
        );
        let reruns: Vec<_> = report.targets.iter().map(|t| t.rerun_args()).collect();
        let reruns: Vec<_> = reruns.iter().map(|r| r.as_deref()).collect();
        assert_eq!(
            reruns,
            [
                Some("--lib"),
                Some("--bin mt"),
                Some("--test it"),
                Some("--doc")
            ]
        );
        let no_panic = &report.targets[0].tests;
        let no_panic = no_panic
            .iter()
            .find(|t| t.name == "tests::no_panic")
            .unwrap();
        assert!(no_panic.output.contains("did not panic as expected"));
        assert!(!report.stopped_early);
    }

    #[test]
    fn nocapture_output_that_looks_like_cargo() {
        // A test prints a fake "Running" line, a fake suite event, and text
        // with no final newline.
        let report = fixture("edge-nocapture.txt");
        let lib = &report.targets[0];
        assert_eq!(report.targets.len(), 4);
        assert_eq!(lib.label, "lib");
        assert_eq!(lib.status(), Status::Fail);
        assert_eq!(lib.count(Status::Pass), 2);
        assert_eq!(lib.count(Status::Fail), 4);
        assert_eq!(lib.count(Status::Skip), 1);
        assert_eq!(lib.count(Status::Abort), 0);
    }

    #[test]
    fn crash_without_suite_end_then_next_binary() {
        let input = r#"     Running tests/a.rs (target/debug/deps/a-0001)
{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "event": "started", "name": "boom" }
error: test failed, to rerun pass `--test a`
     Running tests/b.rs (target/debug/deps/b-0002)
{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "event": "started", "name": "fine" }
{ "type": "test", "name": "fine", "event": "ok" }
{ "type": "suite", "event": "ok", "passed": 1, "failed": 0, "ignored": 0 }
"#;
        let report = parse(input);
        assert_eq!(
            labels(&report),
            [
                (None, "test a", Status::Abort),
                (None, "test b", Status::Pass),
            ]
        );
    }

    const LIB: &str = "Running unittests src/lib.rs (target/debug/build/demo/1/out/demo-1)";

    #[test]
    fn crash_before_first_event_fails() {
        let input = format!(
            r#"{LIB}
{{ "type": "suite", "event": "started", "test_count": 1 }}
{{ "type": "test", "event": "ok", "name": "works" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
Running tests/crash.rs (target/debug/build/demo/2/out/crash-2)
error: test failed, to rerun pass `--test crash`
  process didn't exit successfully: `crash-2` (signal: 11, SIGSEGV: invalid memory reference)
error: 1 target failed:
    `--test crash`
"#
        );
        let report = parse(&input);
        assert!(report.failed());
        let crash = &report.targets[1];
        assert_eq!(crash.label, "test crash");
        assert_eq!(crash.status(), Status::Abort);
        assert!(!crash.is_empty());
        assert!(crash.exit_reason.as_deref().unwrap().contains("SIGSEGV"));
    }

    #[test]
    fn crash_after_suite_ok_fails() {
        let input = format!(
            r#"{LIB}
{{ "type": "suite", "event": "started", "test_count": 1 }}
{{ "type": "test", "event": "ok", "name": "works" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
error: test failed, to rerun pass `--lib`
"#
        );
        let report = parse(&input);
        assert_eq!(report.targets[0].status(), Status::Fail);
        assert!(report.failed());
    }

    #[test]
    fn later_result_wins_over_printed_events() {
        // With --nocapture, a test prints a fake ok event and suite end.
        let input = format!(
            r#"{LIB}
{{ "type": "suite", "event": "started", "test_count": 1 }}
{{ "type": "test", "event": "started", "name": "bad" }}
{{ "type": "test", "event": "ok", "name": "bad" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
{{ "type": "test", "event": "failed", "name": "bad", "stdout": "real failure" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
"#
        );
        let report = parse(&input);
        let target = &report.targets[0];
        assert_eq!(target.status(), Status::Fail);
        assert_eq!(target.count(Status::Fail), 1);
        assert_eq!(target.count(Status::Pass), 0);
        assert_eq!(target.tests[0].output, "real failure");
    }

    #[test]
    fn nocapture_output_is_kept_on_the_target() {
        let input = format!(
            r#"{LIB}
{{ "type": "suite", "event": "started", "test_count": 1 }}
{{ "type": "test", "event": "started", "name": "bad" }}
thread 'bad' panicked at src/lib.rs:3:1:
assertion failed: critical invariant
{{ "type": "test", "event": "failed", "name": "bad" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
"#
        );
        let report = parse(&input);
        let output = report.targets[0].output.join("\n");
        assert!(output.contains("critical invariant"), "{output}");
    }

    #[test]
    fn event_after_partial_json_text() {
        let input = format!(
            r#"{LIB}
{{ "type": "suite", "event": "started", "test_count": 1 }}
{{ "type": "test", "event": "started", "name": "works" }}
log {{ "type": "unfinished" {{ "type": "test", "event": "ok", "name": "works" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
"#
        );
        let report = parse(&input);
        assert_eq!(report.targets[0].status(), Status::Pass);
        assert_eq!(report.targets[0].count(Status::Pass), 1);
    }

    #[test]
    fn two_doc_suites_are_one_target() {
        let input = r#"Doc-tests demo
{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "event": "ok", "name": "src/lib.rs - f (line 1)" }
{ "type": "suite", "event": "ok", "passed": 1, "failed": 0, "exec_time": 0.5 }
{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "event": "failed", "name": "src/lib.rs - g (line 10)" }
{ "type": "suite", "event": "failed", "passed": 0, "failed": 1, "exec_time": 0.25 }
error: doctest failed, to rerun pass `--doc`
"#;
        let report = parse(input);
        assert_eq!(labels(&report), [(Some("demo"), "doc", Status::Fail)]);
        assert_eq!(report.targets[0].tests.len(), 2);
        assert_eq!(report.targets[0].elapsed(), Some(0.75));
    }

    #[test]
    fn target_name_from_cargo_toml() {
        let header =
            Header::running("tests/smoke.rs (target/debug/build/demo/1/out/api_contract-1)");
        assert_eq!(header.label, "test api_contract");
        assert_eq!(header.selector.as_deref(), Some("--test api_contract"));
        // A file name with '-' keeps it, since the crate name has '_'.
        let header = Header::running("tests/my-api.rs (target/debug/build/demo/1/out/my_api-1)");
        assert_eq!(header.label, "test my-api");
    }

    #[test]
    fn doc_package_from_lib_only_and_rerun_hint() {
        let input = r#"Running unittests src/lib.rs (target/debug/build/alpha/1/out/shared-1)
{ "type": "suite", "event": "started", "test_count": 0 }
{ "type": "suite", "event": "ok", "passed": 0, "failed": 0 }
Running tests/shared.rs (target/debug/build/beta/2/out/shared-2)
{ "type": "suite", "event": "started", "test_count": 0 }
{ "type": "suite", "event": "ok", "passed": 0, "failed": 0 }
Doc-tests shared
{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "event": "failed", "name": "src/lib.rs - f (line 1)" }
{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }
error: doctest failed, to rerun pass `-p alpha --doc`
"#;
        let report = parse(input);
        assert_eq!(report.targets[2].package.as_deref(), Some("alpha"));
    }

    #[test]
    fn timeout_reason_is_output() {
        let input = r#"{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "event": "failed", "name": "slow", "exec_time": 61, "reason": "time limit exceeded" }
{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }
"#;
        let report = parse(input);
        assert_eq!(report.targets[0].tests[0].output, "time limit exceeded");
    }

    #[test]
    fn colored_cargo_lines() {
        let input = "\x1b[1m\x1b[32m     Running\x1b[0m tests/api.rs (target/debug/build/demo/1/out/api-1)\n\
            { \"type\": \"suite\", \"event\": \"started\", \"test_count\": 0 }\n\
            { \"type\": \"suite\", \"event\": \"ok\", \"passed\": 0, \"failed\": 0 }\n";
        let report = parse(input);
        assert_eq!(labels(&report), [(Some("demo"), "test api", Status::Pass)]);
    }

    #[test]
    fn quiet_names_from_rerun_hints() {
        let input = r#"{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "event": "failed", "name": "src/lib.rs - f (line 1)" }
{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }
error: doctest failed, to rerun pass `-p alpha --doc`
"#;
        let report = parse(input);
        assert_eq!(labels(&report), [(Some("alpha"), "doc", Status::Fail)]);
        assert!(report.targets[0].is_doc());
    }

    #[test]
    fn rerun_hint_after_text_without_newline() {
        let input = format!(
            r#"{LIB}
{{"type":"suite","event":"started","test_count":1}}
{{"type":"test","event":"ok","name":"works"}}
{{"type":"suite","event":"ok","passed":1,"failed":0}}
byeerror: test failed, to rerun pass `--lib`
  process didn't exit successfully: `demo-1` (signal: 11, SIGSEGV)
error: 1 target failed:
    `--lib`
"#
        );
        let report = parse(&input);
        let target = &report.targets[0];
        assert_eq!(target.status(), Status::Fail);
        assert_eq!(target.exit_reason.as_deref(), Some("signal: 11, SIGSEGV"));
        assert!(target.output.iter().any(|l| l == "bye"));
    }

    #[test]
    fn reopened_suite_replaces_printed_suite_end() {
        let input = r#"{"type":"suite","event":"started","test_count":1}
{"type":"test","event":"started","name":"works"}
{"type":"test","event":"ok","name":"works"}
{"type":"suite","event":"failed","passed":0,"failed":1,"filtered_out":9,"exec_time":10}
{"type":"test","event":"ok","name":"works"}
{"type":"suite","event":"ok","passed":1,"failed":0,"filtered_out":0,"exec_time":1}
"#;
        let report = parse(input);
        let target = &report.targets[0];
        assert_eq!(target.status(), Status::Pass);
        assert_eq!(target.elapsed(), Some(1.0));
        assert_eq!(target.filtered_out(), 0);
    }

    #[test]
    fn printed_rerun_hint_does_not_fail_a_passing_target() {
        let input = format!(
            r#"{LIB}
{{"type":"suite","event":"started","test_count":1}}
{{"type":"test","event":"started","name":"works"}}
error: test failed, to rerun pass `--lib`
{{"type":"test","event":"ok","name":"works"}}
{{"type":"suite","event":"ok","passed":1,"failed":0}}
"#
        );
        let report = parse(&input);
        assert_eq!(report.targets[0].status(), Status::Pass);
        assert!(!report.stopped_early);
    }

    #[test]
    fn unfinished_escape_before_event() {
        let input = "{\"type\":\"suite\",\"event\":\"started\",\"test_count\":1}\n\
            {\"type\":\"test\",\"event\":\"started\",\"name\":\"works\"}\n\
            \x1b[{ \"type\": \"test\", \"event\": \"ok\", \"name\": \"works\" }\n\
            {\"type\":\"suite\",\"event\":\"ok\",\"passed\":1,\"failed\":0}\n";
        let report = parse(input);
        assert_eq!(report.targets[0].status(), Status::Pass);
    }

    #[test]
    fn json_lines_that_are_not_events_are_output() {
        let input = r#"{"type":"suite","event":"started","test_count":1}
{"type":"test","event":"started","name":"bad"}
{"type":"error","message":"database unavailable"}
{"type":"test","event":"failed","name":"bad"}
{"type":"suite","event":"failed","passed":0,"failed":1}
"#;
        let report = parse(input);
        let output = report.targets[0].output.join("\n");
        assert!(output.contains("database unavailable"), "{output}");
    }

    #[test]
    fn quiet_doc_suites_are_one_target() {
        let input = r#"{"type":"suite","event":"started","test_count":1}
{"type":"test","event":"failed","name":"src/lib.rs - f (line 1)"}
{"type":"suite","event":"failed","passed":0,"failed":1}
{"type":"suite","event":"started","test_count":1}
{"type":"test","event":"ok","name":"src/lib.rs - g (line 10)"}
{"type":"suite","event":"ok","passed":1,"failed":0}
error: doctest failed, to rerun pass `--doc`
"#;
        let report = parse(input);
        assert_eq!(labels(&report), [(None, "doc", Status::Fail)]);
        assert_eq!(report.targets[0].tests.len(), 2);
    }

    #[test]
    fn hint_restores_hyphen_in_target_name() {
        let input = r#"Running tests/smoke.rs (target/debug/build/demo/1/out/api_contract-1)
{"type":"suite","event":"started","test_count":1}
{"type":"test","event":"failed","name":"bad"}
{"type":"suite","event":"failed","passed":0,"failed":1}
error: test failed, to rerun pass `--test api-contract`
"#;
        let report = parse(input);
        assert_eq!(report.targets[0].label, "test api-contract");
    }

    #[test]
    fn header_at_end_of_input_is_abort() {
        let report = parse(&format!("{LIB}\n"));
        assert_eq!(labels(&report), [(Some("demo"), "lib", Status::Abort)]);
    }

    #[test]
    fn doc_target_finishes_once() {
        let input = r#"Doc-tests demo
{"type":"suite","event":"started","test_count":1}
{"type":"test","event":"ok","name":"works"}
{"type":"suite","event":"ok","passed":1,"failed":0}
{"type":"suite","event":"started","test_count":1}
{"type":"test","event":"failed","name":"bad"}
{"type":"suite","event":"failed","passed":0,"failed":1}
"#;
        let mut parser = Parser::new();
        let mut finished: Vec<usize> = input.lines().filter_map(|l| parser.line(l)).collect();
        finished.extend(parser.flush());
        assert_eq!(finished, [0]);
    }

    const START1: &str = r#"{ "type": "suite", "event": "started", "test_count": 1 }"#;

    #[test]
    fn printed_process_error_does_not_fail_a_passing_test() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "works" }}
child process didn't exit successfully: `helper` (exit status: 1)
{{ "type": "test", "name": "works", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
"#
        );
        let report = parse(&input);
        assert_eq!(report.targets[0].status(), Status::Pass);
        assert!(!report.stopped_early);
        assert_eq!(report.targets[0].output.len(), 1);
    }

    #[test]
    fn printed_duplicate_start_is_ignored() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "works" }}
{{ "type": "test", "event": "started", "name": "works" }}
{{ "type": "test", "name": "works", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
"#
        );
        let report = parse(&input);
        assert_eq!(report.targets[0].status(), Status::Pass);
        assert_eq!(report.targets[0].tests.len(), 1);
    }

    #[test]
    fn quiet_doc_targets_of_two_packages_stay_apart() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "name": "src/lib.rs - f (line 1)", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
error: doctest failed, to rerun pass `-p alpha --doc`
{START1}
{{ "type": "test", "name": "src/lib.rs - g (line 10)", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
error: doctest failed, to rerun pass `-p beta --doc`
error: 2 targets failed:
"#
        );
        let report = parse(&input);
        assert_eq!(
            labels(&report),
            [
                (Some("alpha"), "doc", Status::Fail),
                (Some("beta"), "doc", Status::Fail),
            ]
        );
    }

    #[test]
    fn printed_failed_summary_keeps_later_output() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "bad" }}
error: 1 target failed:
database unavailable
{{ "type": "test", "name": "bad", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
"#
        );
        let report = parse(&input);
        let output = report.targets[0].output.join("\n");
        assert!(output.contains("database unavailable"), "{output}");
    }

    #[test]
    fn printed_report_type_json_is_output() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "bad" }}
{{"type":"report","message":"database unavailable"}}
{{ "type": "test", "name": "bad", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
{{ "type": "report", "total_time": 0.5, "compilation_time": 0.2 }}
"#
        );
        let report = parse(&input);
        let output = report.targets[0].output.join("\n");
        assert!(output.contains("database unavailable"), "{output}");
        assert!(!output.contains("total_time"), "{output}");
    }

    #[test]
    fn printed_hint_in_doc_suite_keeps_one_doc_target() {
        let input = format!(
            r#"Doc-tests demo
{START1}
{{ "type": "test", "event": "started", "name": "src/lib.rs - f (line 1)" }}
error: test failed, to rerun pass `--doc`
{{ "type": "test", "name": "src/lib.rs - f (line 1)", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
{START1}
{{ "type": "test", "event": "started", "name": "src/lib.rs - g (line 10)" }}
{{ "type": "test", "name": "src/lib.rs - g (line 10)", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
{{ "type": "report", "total_time": 0.5, "compilation_time": 0.2 }}
error: doctest failed, to rerun pass `--doc`
error: 1 target failed:
"#
        );
        let report = parse(&input);
        assert_eq!(labels(&report), [(Some("demo"), "doc", Status::Fail)]);
        assert_eq!(report.targets[0].tests.len(), 2);
    }

    fn finished(input: &str) -> Vec<usize> {
        let mut parser = Parser::new();
        let mut finished: Vec<usize> = input.lines().filter_map(|l| parser.line(l)).collect();
        finished.extend(parser.flush());
        finished
    }

    #[test]
    fn quiet_doc_suites_finish_once() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "name": "src/lib.rs - f (line 1)", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
{START1}
{{ "type": "test", "name": "src/lib.rs - g (line 10)", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
error: doctest failed, to rerun pass `--doc`
"#
        );
        assert_eq!(finished(&input), [0]);
    }

    #[test]
    fn crashed_target_finishes_once() {
        let input = format!(
            r#"{LIB}
{START1}
{{ "type": "test", "event": "started", "name": "works" }}
error: test failed, to rerun pass `--lib`
  process didn't exit successfully: `demo-1` (signal: 11, SIGSEGV)
error: 1 target failed:
"#
        );
        assert_eq!(finished(&input), [0]);
    }

    #[test]
    fn printed_hint_and_summary_do_not_fail_a_passing_test() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "works" }}
error: test failed, to rerun pass `--lib`
error: 1 target failed:
{{ "type": "test", "name": "works", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
"#
        );
        let report = parse(&input);
        assert_eq!(report.targets[0].status(), Status::Pass);
        assert_eq!(report.targets[0].count(Status::Pass), 1);
        assert!(!report.failed());
    }

    #[test]
    fn printed_result_for_another_test_is_output() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "works" }}
{{"type":"test","event":"failed","name":"sample","stdout":"example failure"}}
{{ "type": "test", "name": "works", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
"#
        );
        let report = parse(&input);
        let target = &report.targets[0];
        assert_eq!(target.status(), Status::Pass);
        assert_eq!(target.tests.len(), 1);
        assert!(target.output[0].contains("example failure"));
    }

    #[test]
    fn quiet_doc_report_ends_a_doc_target() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "src/lib.rs - f (line 1)" }}
{{ "type": "test", "name": "src/lib.rs - f (line 1)", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
{{ "type": "report", "total_time": 0.5, "compilation_time": 0.2 }}
{START1}
{{ "type": "test", "event": "started", "name": "src/lib.rs - g (line 10)" }}
{{ "type": "test", "name": "src/lib.rs - g (line 10)", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
{{ "type": "report", "total_time": 0.5, "compilation_time": 0.2 }}
error: doctest failed, to rerun pass `-p beta --doc`
error: 1 target failed:
"#
        );
        let report = parse(&input);
        assert_eq!(
            labels(&report),
            [
                (None, "doc", Status::Pass),
                (Some("beta"), "doc", Status::Fail),
            ]
        );
        assert_eq!(finished(&input), [0, 1]);
    }

    #[test]
    fn printed_report_json_while_a_test_runs_is_output() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "bad" }}
{{"type":"report","total_time":0.1,"compilation_time":0.1,"message":"database unavailable"}}
{{ "type": "test", "name": "bad", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
"#
        );
        let report = parse(&input);
        let output = report.targets[0].output.join("\n");
        assert!(output.contains("database unavailable"), "{output}");
    }

    #[test]
    fn printed_hint_and_header_do_not_split_a_target() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "works" }}
error: test failed, to rerun pass `--lib`
Running tests/api.rs (target/debug/deps/api-0123456789abcdef)
{{ "type": "test", "name": "works", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
"#
        );
        let report = parse(&input);
        assert_eq!(labels(&report), [(None, "#1", Status::Pass)]);
        let output = report.targets[0].output.join("\n");
        assert!(output.contains("Running tests/api.rs"), "{output}");
    }

    #[test]
    fn printed_start_for_another_test_is_dropped_at_suite_end() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "works" }}
{{"type":"test","event":"started","name":"sample"}}
{{ "type": "test", "name": "works", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0, "ignored": 0, "measured": 0 }}
"#
        );
        let report = parse(&input);
        let target = &report.targets[0];
        assert_eq!(target.status(), Status::Pass);
        assert_eq!(target.tests.len(), 1);
    }

    #[test]
    fn quiet_doc_suites_of_two_packages_without_report() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "src/lib.rs - f (line 1)" }}
{{ "type": "test", "name": "src/lib.rs - f (line 1)", "event": "ok" }}
{{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }}
{START1}
{{ "type": "test", "event": "started", "name": "src/lib.rs - g (line 10)" }}
{{ "type": "test", "name": "src/lib.rs - g (line 10)", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
error: doctest failed, to rerun pass `-p beta --doc`
error: 1 target failed:
"#
        );
        let report = parse(&input);
        // The suites can come from two packages, so the package is unknown.
        // The rerun command still uses cargo's hint.
        assert_eq!(labels(&report), [(None, "doc", Status::Fail)]);
        assert_eq!(
            report.targets[0].rerun_args().as_deref(),
            Some("-p beta --doc")
        );
    }

    #[test]
    fn printed_hint_and_summary_keep_later_output() {
        let input = format!(
            r#"{START1}
{{ "type": "test", "event": "started", "name": "bad" }}
error: test failed, to rerun pass `--lib`
error: 1 target failed:
database unavailable
{{ "type": "test", "name": "bad", "event": "failed" }}
{{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }}
"#
        );
        let report = parse(&input);
        let output = report.targets[0].output.join("\n");
        assert!(output.contains("database unavailable"), "{output}");
        assert!(report.other_lines.is_empty());
    }

    #[test]
    fn hint_names_a_lib_with_a_custom_path() {
        let input = r#"Running unittests tests/lib.rs (target/debug/build/demo/1/out/custom-1)
{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "event": "started", "name": "bad" }
{ "type": "test", "name": "bad", "event": "failed", "stdout": "boom" }
{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }
error: test failed, to rerun pass `--lib`
"#;
        let report = parse(input);
        assert_eq!(labels(&report), [(Some("demo"), "lib", Status::Fail)]);
    }
}
