# cargo-tparse

A command line tool for summarizing `cargo test` output. The Rust version of [tparse](https://github.com/mfridman/tparse).

> [!TIP]
> Run `cargo test` with JSON output and pipe stderr too (`2>&1`).

By default, `cargo tparse` prints each failed test with its output and a command to rerun only that test, followed by a summary table (one row per test target).

```
FAIL  t::fails  alpha-core lib

    thread 't::fails' panicked at crates/alpha-core/src/lib.rs:16:30:
    boom

    rerun: cargo test -p alpha-core --lib -- t::fails --exact

┌────────┬─────────┬────────────┬────────┬──────┬──────┬──────┐
│ Status │ Elapsed │ Package    │ Target │ Pass │ Fail │ Skip │
├────────┼─────────┼────────────┼────────┼──────┼──────┼──────┤
│ FAIL   │   0.00s │ alpha-core │ lib    │    2 │    1 │    0 │
│ PASS   │   0.00s │ beta       │ bin    │    1 │    0 │    0 │
└────────┴─────────┴────────────┴────────┴──────┴──────┴──────┘

3 passed, 1 failed, 0 skipped in 0.00s (build 0.42s, total 0.61s)
```

## Install

```
cargo install cargo-tparse
```

## Usage

Pipe `cargo test` JSON output to `cargo tparse`:

```
cargo +nightly test --no-fail-fast -- -Z unstable-options --format json 2>&1 | cargo tparse
```

- libtest JSON output is unstable and requires nightly; the same pipe will work on stable once [stabilized](https://rust-lang.github.io/rfcs/3558-libtest-json.html).
- `2>&1` lets `cargo tparse` read package and target names from cargo's `Running` lines.
- `--no-fail-fast` runs every test target instead of stopping at the first failure.

Read saved output: `cargo tparse < test.out`

Options:

```
--format <table|markdown>   Output format (default: table)
--by <target|package>       One row for each test target or each package (default: target)
--sort <name|elapsed>       Sort rows by name, or by elapsed time with the slowest first (default: name)
--slow <N>                  Show the N slowest passed tests of each package (needs --report-time)
--progress                  Print cargo's build status, and a line for each test target as it finishes
```

Without `--progress`, `cargo tparse` prints nothing until cargo exits, which can take a long time while cargo builds. With `--progress`, you see cargo's `Compiling` and `Finished` lines as they come, then one line for each test target.

Find the slowest packages in a workspace:

```
cargo +nightly test --workspace --no-fail-fast -- -Z unstable-options --format json 2>&1 \
  | cargo tparse --by package --sort elapsed
```

Show the 5 slowest tests of each package. libtest gives the time of each test only with `--report-time`:

```
cargo +nightly test --workspace --no-fail-fast -- -Z unstable-options --format json --report-time 2>&1 \
  | cargo tparse --slow 5
```

Exits with 1 if any test fails. Write a GitHub Actions summary:

```
set -o pipefail
cargo +nightly test --no-fail-fast -- -Z unstable-options --format json 2>&1 \
  | cargo tparse --format markdown >> "$GITHUB_STEP_SUMMARY"
```

## Times

Cargo builds the code, then runs each test binary one at a time. In a binary, libtest runs the tests in parallel.

- `Elapsed` in the tables, and the time after `in` on the totals line, is the time that libtest measured in each test binary. It does not count the build, the start of each binary, or the time that rustdoc needs before it runs doc tests.
- `build` is the time from cargo's `Finished` line.
- `total` is the time from the start of `cargo tparse` to the end of cargo's output, so it counts everything. It shows only when `cargo tparse` reads cargo's output live from a pipe, not for saved output.
- With `--slow`, the time of each test can overlap with other tests, because tests run in parallel.

## Limits

- With `--nocapture`, libtest does not capture test output, so it shows under "More from <target>" instead of under each test.
- Older cargo (for example 1.95) puts test binaries in `target/debug/deps`, and their paths have no package name. Then `cargo tparse` uses the crate name of the package's lib or `src/main.rs` bin, which has `_` where the package name has `-`. A package with neither (only `src/bin/*.rs`, or `[lib] test = false`) can show under the package before it. When cargo's rerun hint for a failed target names the package, all targets of that package use the name.
- `cargo test -q` hides cargo's `Running` lines, so passing targets show as `#1`, `#2`, and doc tests from more than one package can share a row.
