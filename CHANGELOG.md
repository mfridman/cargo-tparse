# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- Add `--by package` to show one row for each package, with the number of test targets in each (#2)
- Add `--sort elapsed` to show the slowest rows first (#2)
- Show package names when cargo puts test binaries in `target/debug/deps` (older cargo, for example 1.95). Before, the Package column showed `-`. The name is a guess from the crate name of the package's lib or `src/main.rs` bin, or comes from cargo's rerun hint (#2)
- Do not use a guessed package name in `-p` of a rerun command, because a crate name such as `foo_bar` does not match the package `foo-bar` (#2)
- Add `--slow N` to show the N slowest passed tests of each package. Test times need libtest's `--report-time` option (#3)
- Start each hint after the totals line with "Hint:", so it does not look like part of the results
- `--progress` now also prints cargo's build status lines (`Compiling`, `Finished`, and others), so a long build no longer looks stuck. Progress lines also show guessed package names

## [v0.1.0] - 2026-11-20

- Initial release

[Unreleased]: https://github.com/mfridman/cargo-tparse/compare/v0.1.0...HEAD
[v0.1.0]: https://github.com/mfridman/cargo-tparse/releases/tag/v0.1.0
