# Limits

- With `--nocapture`, libtest does not capture test output, so it shows under "More from <target>" instead of under each test.
- `cargo test -q` hides cargo's `Running` lines, so passing targets show as `#1`, `#2`, and doc tests from more than one package can share a row.

## Package names on older cargo

Older cargo (for example 1.95) puts test binaries in `target/debug/deps`, and their paths have no package name. Then `cargo tparse` guesses the name:

- It uses the crate name of the package's lib or `src/main.rs` bin, which has `_` where the package name has `-`.
- A package with neither (only `src/bin/*.rs`, or `[lib] test = false`) can show under the package before it.
- When cargo's rerun hint for a failed target names the package, all targets of that package use the name. A `--progress` line printed before the hint keeps the guessed name.
- A guessed name is never used in `-p` of a rerun command.
