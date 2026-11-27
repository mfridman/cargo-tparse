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

Read saved output: `cargo tparse < test.out`. Exits with 1 if any test fails.

Options:

```
--format <table|markdown>   Output format (default: table)
--by <target|package>       One row for each test target or each package (default: target)
--sort <name|elapsed>       Sort rows by name, or by elapsed time with the slowest first (default: name)
--slow <N>                  Show the N slowest passed tests of each package (needs --report-time)
--progress                  Print cargo's build status, and a line for each test target as it finishes
```

## More

- [Examples](docs/examples.md): find slow packages and tests, see progress, write a GitHub Actions summary, read saved output.
- [Times](docs/times.md): what `Elapsed`, `build`, and `total` count.
- [Limits](docs/limits.md): `--nocapture`, `cargo test -q`, and package names on older cargo.
