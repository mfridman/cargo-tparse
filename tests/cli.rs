use std::io::Write;
use std::process::{Command, Output, Stdio};

/// Run cargo-tparse with input on stdin.
fn run(input: &str, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cargo-tparse"))
        .args(args)
        .env_remove("CLICOLOR_FORCE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn failed_tests_exit_one_with_rerun_command() {
    let output = run(&fixture("workspace.txt"), &[]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("rerun: cargo test -p alpha-core --lib -- t::fails --exact"));
    assert!(text.contains("3 passed, 1 failed, 0 skipped"));
}

#[test]
fn cargo_subcommand_argument_is_ignored() {
    let output = run(
        &fixture("workspace.txt"),
        &["tparse", "--format", "markdown"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("| Status |"));
}

#[test]
fn piped_output_has_no_color() {
    let output = run(&fixture("single.txt"), &[]);
    let text = stdout(&output);
    assert!(text.contains("tests::bad"));
    assert!(!text.contains('\x1b'));
}

#[test]
fn markdown_summary() {
    let output = run(&fixture("single.txt"), &["--format", "markdown"]);
    let text = stdout(&output);
    assert!(text.contains("| Status | Elapsed | Package | Target | Pass | Fail | Skip |"));
    assert!(text.contains("Rerun: `cargo test --lib -- tests::bad --exact`"));
}

#[test]
fn progress_prints_a_line_for_each_target() {
    let output = run(&fixture("workspace.txt"), &["--progress"]);
    let text = stdout(&output);
    assert!(
        text.starts_with("FAIL   alpha-core lib  1 failed"),
        "{text}"
    );
    assert!(text.contains("PASS   beta bin"));
}

#[test]
fn build_error_prints_cargo_output() {
    let output = run(&fixture("build-error.txt"), &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("error[E0308]"));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no test events found"));
}

#[test]
fn passing_run_exits_zero() {
    let input = concat!(
        "     Running tests/api.rs (target/debug/deps/api-0123abcd)\n",
        r#"{ "type": "suite", "event": "started", "test_count": 1 }"#,
        "\n",
        r#"{ "type": "test", "event": "started", "name": "works" }"#,
        "\n",
        r#"{ "type": "test", "name": "works", "event": "ok" }"#,
        "\n",
        r#"{ "type": "suite", "event": "ok", "passed": 1, "failed": 0, "exec_time": 0.002 }"#,
        "\n",
    );
    let output = run(input, &[]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("1 passed, 0 failed, 0 skipped"));
}

#[test]
fn empty_input_exits_one() {
    let output = run("", &[]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn rerun_commands_match_cargo_hints() {
    let text = stdout(&run(&fixture("edge.txt"), &[]));
    assert!(text.contains("rerun: cargo test --bin mt -- t::bin_fails --exact"));
    assert!(text.contains("rerun: cargo test --lib -- 'tests::ünïcödé_名前' --exact"));
    // No --exact for doc tests: rustdoc then matches nothing.
    assert!(text.contains("rerun: cargo test --doc -- 'src/lib.rs - two (line 1)'\n"));
}

#[test]
fn compact_json_and_no_finished_targets() {
    // Compact JSON (no space after the brace), and a binary that never ends.
    let input = concat!(
        "     Running tests/a.rs (target/debug/deps/a-0001)\n",
        r#"{"type":"suite","event":"started","test_count":1}"#,
        "\n",
        r#"{"type":"test","event":"started","name":"hangs"}"#,
        "\n",
    );
    let output = run(input, &[]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("ABORT  hangs  test a"), "{text}");
    assert!(
        text.contains("0 passed, 1 failed, 0 skipped in 0.00s"),
        "{text}"
    );
}

#[test]
fn markdown_output_cannot_break_out_of_code() {
    let input = concat!(
        "Doc-tests demo\n",
        r#"{ "type": "suite", "event": "started", "test_count": 1 }"#,
        "\n",
        r#"{ "type": "test", "event": "failed", "name": "src/a`b.rs - f (line 1)", "stdout": "before\n````\n# forged heading\n````\nafter" }"#,
        "\n",
        r#"{ "type": "suite", "event": "failed", "passed": 0, "failed": 1 }"#,
        "\n",
    );
    let text = stdout(&run(input, &["--format", "markdown"]));
    assert!(text.contains("``src/a`b.rs - f (line 1)``"), "{text}");
    assert!(
        text.contains("`````text\nbefore\n````\n# forged heading"),
        "{text}"
    );
}

#[test]
fn wide_characters_keep_the_table_aligned() {
    let input = concat!(
        "Running tests/界.rs (target/debug/build/demo/1/out/界-1)\n",
        r#"{ "type": "suite", "event": "started", "test_count": 1 }"#,
        "\n",
        r#"{ "type": "test", "event": "ok", "name": "works" }"#,
        "\n",
        r#"{ "type": "suite", "event": "ok", "passed": 1, "failed": 0 }"#,
        "\n",
    );
    let text = stdout(&run(input, &[]));
    let widths: Vec<usize> = text
        .lines()
        .filter(|l| l.starts_with('│') || l.starts_with('┌'))
        .map(unicode_width::UnicodeWidthStr::width)
        .collect();
    assert!(widths.windows(2).all(|w| w[0] == w[1]), "{text}");
}

#[test]
fn zero_tests_show_filtered_count() {
    let input = concat!(
        r#"{ "type": "suite", "event": "started", "test_count": 0 }"#,
        "\n",
        r#"{ "type": "suite", "event": "ok", "passed": 0, "failed": 0, "filtered_out": 12 }"#,
        "\n",
    );
    let output = run(input, &[]);
    // Same as cargo test: a filter that matches no test is not a failure.
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("0 passed, 0 failed, 0 skipped, 12 filtered out"));
}

#[test]
fn crashed_target_without_tests_is_reported() {
    let input = concat!(
        "Running tests/crash.rs (target/debug/build/demo/2/out/crash-2)\n",
        "error: test failed, to rerun pass `--test crash`\n",
        "  process didn't exit successfully: `crash-2` (signal: 11, SIGSEGV: invalid memory reference)\n",
    );
    let output = run(input, &[]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("ABORT  demo test crash"), "{text}");
    assert!(text.contains("Exit: signal: 11"), "{text}");
    assert!(text.contains("rerun: cargo test --test crash\n"), "{text}");
}

#[test]
fn stable_toolchain_hint() {
    let input = concat!(
        "     Running unittests src/lib.rs (target/debug/build/demo/1/out/demo-1)\n",
        "error: the option `Z` is only accepted on the nightly compiler\n",
        "error: test failed, to rerun pass `--lib`\n",
    );
    let output = run(input, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("needs a nightly toolchain"));
}

#[test]
fn markdown_color_codes_cannot_join_backticks() {
    let input = concat!(
        r#"{"type":"suite","event":"started","test_count":1}"#,
        "\n",
        r#"{"type":"test","event":"failed","name":"bad","stdout":"before\n`\u001b[0m``\n# forged heading\n`\u001b[0m``\nafter"}"#,
        "\n",
        r#"{"type":"suite","event":"failed","passed":0,"failed":1}"#,
        "\n",
    );
    let text = stdout(&run(input, &["--format", "markdown"]));
    assert!(
        text.contains("````text\nbefore\n```\n# forged heading"),
        "{text}"
    );
}

#[test]
fn markdown_escapes_target_names() {
    let input = concat!(
        "Running tests/__api__.rs (target/debug/build/demo/1/out/__api__-1)\n",
        r#"{"type":"suite","event":"started","test_count":1}"#,
        "\n",
        r#"{"type":"test","event":"failed","name":"works"}"#,
        "\n",
        r#"{"type":"suite","event":"failed","passed":0,"failed":1}"#,
        "\n",
    );
    let text = stdout(&run(input, &["--format", "markdown"]));
    assert!(text.contains(r"| test \_\_api\_\_ |"), "{text}");
    assert!(text.contains(r"(demo test \_\_api\_\_)"), "{text}");
}

#[test]
fn crash_after_failed_test_shows_exit() {
    let input = concat!(
        "     Running unittests src/lib.rs (target/debug/build/demo/1/out/demo-1)\n",
        r#"{"type":"suite","event":"started","test_count":1}"#,
        "\n",
        r#"{"type":"test","event":"failed","name":"bad"}"#,
        "\n",
        r#"{"type":"suite","event":"failed","passed":0,"failed":1}"#,
        "\n",
        "error: test failed, to rerun pass `--lib`\n",
        "\n",
        "Caused by:\n",
        "  process didn't exit successfully: `demo-1` (signal: 11, SIGSEGV)\n",
    );
    let text = stdout(&run(input, &[]));
    assert!(
        text.contains("More from demo lib:\n\n    Exit: signal: 11, SIGSEGV"),
        "{text}"
    );
}

#[test]
fn progress_waits_for_cargo_failure_hint() {
    let input = concat!(
        "     Running unittests src/lib.rs (target/debug/build/demo/1/out/demo-1)\n",
        r#"{"type":"suite","event":"started","test_count":1}"#,
        "\n",
        r#"{"type":"test","event":"ok","name":"works"}"#,
        "\n",
        r#"{"type":"suite","event":"ok","passed":1,"failed":0}"#,
        "\n",
        "error: test failed, to rerun pass `--lib`\n",
    );
    let text = stdout(&run(input, &["--progress"]));
    assert!(text.starts_with("FAIL   demo lib"), "{text}");
    assert!(!text.contains("PASS   demo lib"), "{text}");
}

#[test]
fn markdown_control_characters_cannot_join_backticks() {
    let input = concat!(
        r#"{"type":"suite","event":"started","test_count":1}"#,
        "\n",
        r#"{"type":"test","event":"failed","name":"bad","stdout":"before\n`\u0000``\n# forged heading\n`\u0008``\nafter"}"#,
        "\n",
        r#"{"type":"suite","event":"failed","passed":0,"failed":1}"#,
        "\n",
    );
    let text = stdout(&run(input, &["--format", "markdown"]));
    assert!(
        text.contains("````text\nbefore\n```\n# forged heading"),
        "{text}"
    );
}

#[test]
fn package_rows() {
    let text = stdout(&run(&fixture("workspace-deps.txt"), &["--by", "package"]));
    assert!(
        text.contains("│ Status │ Elapsed │ Package    │ Targets │ Pass │ Fail │ Skip │"),
        "{text}"
    );
    assert!(
        text.contains("│ FAIL   │   0.53s │ alpha-core │       4 │    4 │    1 │    0 │"),
        "{text}"
    );
    assert!(text.contains("9 passed, 1 failed, 0 skipped in 1.20s"));
}

#[test]
fn sort_by_elapsed_puts_the_slowest_first() {
    let order = |args: &[&str]| {
        let text = stdout(&run(&fixture("workspace-deps.txt"), args));
        let packages = ["alpha-core", "beta", "gamma", "delta"];
        let mut found: Vec<(usize, &str)> = packages
            .iter()
            .filter_map(|p| text.find(&format!("│ {p} ")).map(|i| (i, *p)))
            .collect();
        found.sort();
        found.into_iter().map(|(_, p)| p).collect::<Vec<_>>()
    };
    let by_package = ["--by", "package"];
    assert_eq!(order(&by_package), ["alpha-core", "beta", "delta", "gamma"]);
    let slowest = ["--by", "package", "--sort", "elapsed"];
    assert_eq!(order(&slowest), ["alpha-core", "beta", "gamma", "delta"]);
    // With target rows, the slowest target is "beta test cli".
    let text = stdout(&run(&fixture("workspace-deps.txt"), &["--sort", "elapsed"]));
    let first_row = text
        .lines()
        .find(|l| l.starts_with("│ PASS") || l.starts_with("│ FAIL"));
    assert_eq!(
        first_row,
        Some("│ PASS   │   0.50s │ beta       │ test cli  │    1 │    0 │    0 │")
    );
}

#[test]
fn package_rows_in_markdown() {
    let text = stdout(&run(
        &fixture("workspace.txt"),
        &["--by", "package", "--format", "markdown"],
    ));
    assert!(text.contains("| Status | Elapsed | Package | Targets | Pass | Fail | Skip |"));
    assert!(
        text.contains("| --- | ---: | --- | ---: | ---: | ---: | ---: |"),
        "{text}"
    );
    // The doc target of alpha-core ran no tests, so it does not count.
    assert!(
        text.contains("| FAIL | 0.00s | alpha-core | 1 | 2 | 1 | 0 |"),
        "{text}"
    );
}
