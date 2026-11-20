//! Checks for changes in the libtest JSON format. They run the test crate in
//! canary/ and compare the result with files in tests/testdata/. They do not
//! run by default:
//!
//!   cargo +nightly test --test canary -- --ignored nightly_format
//!   cargo +stable test --test canary -- --ignored stable_has_no_json
//!
//! A weekly workflow runs them and opens an issue when one fails. If a change
//! is expected, set CANARY_UPDATE=1 to write the files from the new output.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Run "cargo test" in canary/ with stdout and stderr in one stream, the same
/// as "2>&1" in the README command.
fn cargo_test(test_args: &[&str], bootstrap: bool) -> String {
    let canary = Path::new(env!("CARGO_MANIFEST_DIR")).join("canary");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("canary");
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("\"$CARGO\" test --no-fail-fast -- \"$@\" 2>&1")
        .arg("sh")
        .args(test_args)
        .current_dir(canary)
        .env("CARGO", env!("CARGO"))
        .env("CARGO_TARGET_DIR", target);
    if !bootstrap {
        command.env_remove("RUSTC_BOOTSTRAP");
    }
    let output = command.output().unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn tparse(input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cargo-tparse"))
        .args(["--format", "markdown"])
        .env_remove("CLICOLOR_FORCE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stdout).unwrap()
}

/// The keys of each kind of JSON event, for example
/// "test failed: event message name type".
fn shapes(output: &str) -> String {
    let mut shapes = BTreeSet::new();
    for line in output.lines() {
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str(line.trim()) else {
            continue;
        };
        let field = |key: &str| map.get(key).and_then(|v| v.as_str()).unwrap_or("-");
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        shapes.insert(format!(
            "{} {}: {}",
            field("type"),
            field("event"),
            keys.join(" ")
        ));
    }
    shapes.into_iter().map(|s| s + "\n").collect()
}

/// The parts of the report that do not change from run to run: status
/// lines, rerun commands, table rows without the time, and the totals.
fn summary(report: &str) -> String {
    let mut lines = Vec::new();
    for line in report.lines() {
        if line.starts_with("| ") {
            let mut cells: Vec<&str> = line.split(" | ").collect();
            cells.remove(1);
            lines.push(cells.join(" | "));
        } else if line.starts_with("**") || line.starts_with("Rerun:") {
            lines.push(line.to_string());
        } else if line.contains(" passed, ") {
            let counts = line.rsplit_once(" in ").map_or(line, |(counts, _)| counts);
            lines.push(counts.to_string());
        }
    }
    lines.into_iter().map(|l| l + "\n").collect()
}

fn check(name: &str, actual: &str) {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "testdata", name]
        .iter()
        .collect();
    if std::env::var_os("CANARY_UPDATE").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    assert!(
        expected == actual,
        "the libtest output changed; compare with {}\n\nexpected:\n{expected}\nactual:\n{actual}",
        path.display()
    );
}

#[test]
#[ignore = "needs nightly; the canary workflow runs it"]
fn nightly_format() {
    let output = cargo_test(&["-Z", "unstable-options", "--format", "json"], true);
    check("canary-shapes.txt", &shapes(&output));
    check("canary-summary.txt", &summary(&tparse(&output)));
}

#[test]
#[ignore = "needs stable; the canary workflow runs it"]
fn stable_has_no_json() {
    let output = cargo_test(&["--format", "json"], false);
    let events = output
        .lines()
        .filter(|l| l.contains("{ \"type\": "))
        .count();
    assert_eq!(
        events, 0,
        "libtest JSON works on stable now: update the README and drop +nightly\n\n{output}"
    );
    assert!(
        output.contains("nightly"),
        "stable cargo test failed for another reason:\n\n{output}"
    );
}
