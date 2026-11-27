# Examples

All examples use the same `cargo test` command. Add `--workspace` to test all packages of a workspace.

## Find the slowest packages

Show one row for each package, with the slowest package first:

```
cargo +nightly test --workspace --no-fail-fast -- -Z unstable-options --format json 2>&1 \
  | cargo tparse --by package --sort elapsed
```

`--sort elapsed` also works with the default rows, one for each test target.

## Find the slowest tests

Show the 5 slowest passed tests of each package. libtest gives the time of each test only with `--report-time`:

```
cargo +nightly test --workspace --no-fail-fast -- -Z unstable-options --format json --report-time 2>&1 \
  | cargo tparse --slow 5
```

Without `--report-time`, `cargo tparse` shows a hint and no table of slow tests.

## See progress while cargo builds

Without `--progress`, `cargo tparse` prints nothing until cargo exits, which can take a long time while cargo builds. With `--progress`, you see cargo's `Compiling` and `Finished` lines as they come, then one line for each test target when it finishes:

```
cargo +nightly test --no-fail-fast -- -Z unstable-options --format json 2>&1 \
  | cargo tparse --progress
```

## Write a GitHub Actions job summary

`cargo tparse` exits with 1 if any test fails. `set -o pipefail` makes the step fail too:

```
set -o pipefail
cargo +nightly test --no-fail-fast -- -Z unstable-options --format json 2>&1 \
  | cargo tparse --format markdown >> "$GITHUB_STEP_SUMMARY"
```

## Read saved output

Save the output one time, then read it again with other options:

```
cargo +nightly test --no-fail-fast -- -Z unstable-options --format json > test.out 2>&1
cargo tparse --by package < test.out
```
