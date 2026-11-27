//! Package names for targets that cargo does not name, and package rows.
//!
//! Cargo's "Running" line names the source file and the test binary. Newer
//! cargo puts the package name in the binary path. Older cargo, for
//! example 1.95, does not. Then we guess the package name.
//!
//! `cargo test` runs the unit and integration test binaries sorted by
//! package. In a package, it sorts them by kind: lib, bins, tests, benches,
//! examples. A package has at most one lib, and it runs first. So a lib, or a
//! kind that sorts before the kind of the last target, starts a new package.
//! The crate name of the lib, or of the bin from src/main.rs, is the guess.
//! Doc tests run after all other binaries. Their "Doc-tests" line names the
//! lib crate.
//!
//! A package with no lib tests and no src/main.rs can join the package before
//! it. For example a package with only src/bin/*.rs, or with
//! `[lib] test = false`.

use crate::parse::{Status, Target};

/// Target kinds, in the order that cargo runs them in a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Lib,
    Bin,
    Test,
    Bench,
    Example,
}

impl Kind {
    fn of(target: &Target) -> Option<Self> {
        match target.label.split(' ').next()? {
            "lib" => Some(Self::Lib),
            "bin" => Some(Self::Bin),
            "test" => Some(Self::Test),
            "bench" => Some(Self::Bench),
            "example" => Some(Self::Example),
            _ => None,
        }
    }
}

/// The targets of one package, in the order that cargo ran them.
#[derive(Debug, Default)]
struct Run {
    targets: Vec<usize>,
    last: Option<Kind>,
    lib: Option<String>,
    main_bin: bool,
    /// Crate name of the lib or of the bin from src/main.rs.
    guess: Option<String>,
}

impl Run {
    /// The target is the first target of the next package.
    fn ends_before(&self, target: &Target) -> bool {
        match Kind::of(target) {
            Some(Kind::Lib) => true,
            Some(kind) if self.last.is_some_and(|last| kind < last) => true,
            // A package has one src/main.rs bin. Its crate name is the
            // package name, so it is the same as the crate name of the lib
            // unless Cargo.toml gives the lib another name.
            Some(Kind::Bin) if target.main_bin => {
                let other_lib = match (&self.lib, &target.krate) {
                    (Some(lib), Some(krate)) => !same_name(lib, krate),
                    _ => false,
                };
                self.main_bin || other_lib
            }
            _ => false,
        }
    }

    fn push(&mut self, index: usize, target: &Target) {
        self.targets.push(index);
        let kind = Kind::of(target);
        if kind.is_some() {
            self.last = kind;
        }
        if kind == Some(Kind::Lib) {
            self.lib.clone_from(&target.krate);
            self.guess.clone_from(&target.krate);
        } else if kind == Some(Kind::Bin) && target.main_bin {
            self.main_bin = true;
            if self.guess.is_none() {
                self.guess.clone_from(&target.krate);
            }
        }
    }

    /// A package name from cargo, if it agrees with the guess. Cargo gives
    /// it in the rerun hint of a failed target. Otherwise the guess.
    fn name(&self, targets: &[Target]) -> Option<String> {
        let mut known = self
            .targets
            .iter()
            .map(|&i| &targets[i])
            .filter(|t| t.package_known)
            .filter_map(|t| t.package.as_deref());
        match &self.guess {
            Some(guess) => Some(
                known
                    .find(|p| same_name(p, guess))
                    .unwrap_or(guess)
                    .to_string(),
            ),
            None => {
                let first = known.next()?;
                known.all(|p| p == first).then(|| first.to_string())
            }
        }
    }
}

/// Package and crate names are the same, but crate names have '_' for '-'.
fn same_name(a: &str, b: &str) -> bool {
    a.replace('-', "_") == b.replace('-', "_")
}

/// Give a package name to each target that cargo did not name.
pub fn name_unknown(targets: &mut [Target]) {
    let mut runs: Vec<Run> = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        if target.is_doc() || !target.has_header() {
            continue;
        }
        if runs.last().is_none_or(|run| run.ends_before(target)) {
            runs.push(Run::default());
        }
        if let Some(run) = runs.last_mut() {
            run.push(index, target);
        }
    }
    for run in &runs {
        let name = run.name(targets);
        for &index in &run.targets {
            let target = &mut targets[index];
            if !target.package_known {
                target.package.clone_from(&name);
            }
        }
    }
    // A doc target has the package of the lib with the same crate name.
    for index in 0..targets.len() {
        let doc = &targets[index];
        if !doc.is_doc() || doc.package_known || doc.krate.is_none() {
            continue;
        }
        let lib = targets
            .iter()
            .find(|t| Kind::of(t) == Some(Kind::Lib) && t.krate == doc.krate);
        if let Some(package) = lib.and_then(|t| t.package.clone()) {
            targets[index].package = Some(package);
        }
    }
}

/// The targets of one package, for a package row.
#[derive(Debug)]
pub struct Package<'a> {
    /// None when cargo did not name the package and there is no guess.
    pub name: Option<&'a str>,
    pub targets: Vec<&'a Target>,
}

impl Package<'_> {
    /// The worst status of the targets.
    pub fn status(&self) -> Status {
        let statuses = || self.targets.iter().map(|t| t.status());
        if statuses().any(|s| s == Status::Abort) {
            Status::Abort
        } else if statuses().any(|s| s == Status::Fail) {
            Status::Fail
        } else {
            Status::Pass
        }
    }

    /// Cargo runs one test binary at a time, so the time of a package is the
    /// sum of the times of its targets.
    pub fn elapsed(&self) -> Option<f64> {
        let times = self.targets.iter().filter_map(|t| t.elapsed());
        times.reduce(|total, time| total + time)
    }

    pub fn count(&self, status: Status) -> usize {
        self.targets.iter().map(|t| t.count(status)).sum()
    }

    pub fn filtered_out(&self) -> u64 {
        self.targets.iter().map(|t| t.filtered_out()).sum()
    }
}

/// Group targets by package, in the order of the first target of each.
pub fn packages<'a>(targets: &[&'a Target]) -> Vec<Package<'a>> {
    let mut packages: Vec<Package<'a>> = Vec::new();
    for &target in targets {
        let name = target.package.as_deref();
        match packages.iter_mut().find(|p| p.name == name) {
            Some(package) => package.targets.push(target),
            None => packages.push(Package {
                name,
                targets: vec![target],
            }),
        }
    }
    packages
}
