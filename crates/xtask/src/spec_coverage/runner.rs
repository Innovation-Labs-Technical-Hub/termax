//! Runs the tagged tests and records whether each one exists and passes.
//!
//! Stable libtest has no machine-readable output, so each cargo target with
//! tagged tests runs once as `cargo test -p P --lib|--bin B|--test T --
//! --exact NAME…` and the human `test NAME ... ok` lines are parsed. Only
//! tagged tests run, so this is much cheaper than the whole suite.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::tags::TaggedTest;

/// What happened to one tagged test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Passed,
    Failed,
    Ignored,
    /// The target ran but no test by that name exists (wrong module path,
    /// or compiled out by `cfg` on this platform).
    NotFound,
    /// The crate is not built on this platform.
    Skipped,
}

/// Crates excluded from the build on this platform, as in the justfile's
/// and CI's crate selection.
const EXCLUDED: &[&str] = if cfg!(target_os = "linux") {
    &[]
} else {
    &["termax-term-ghostty", "termax-term-ghostty-sys"]
};

/// Outcome per (target root, test name).
pub type Outcomes = HashMap<(PathBuf, String), Outcome>;

pub fn run(root: &Path, tests: &[TaggedTest]) -> Result<Outcomes> {
    let targets = cargo_targets(root)?;
    let mut by_target: BTreeMap<&Path, Vec<&str>> = BTreeMap::new();
    for t in tests {
        by_target.entry(&t.target_root).or_default().push(&t.name);
    }

    let mut outcomes = Outcomes::new();
    for (target_root, names) in by_target {
        let canonical = target_root
            .canonicalize()
            .with_context(|| format!("resolving {}", target_root.display()))?;
        let Some(target) = targets.get(&canonical) else {
            bail!(
                "no cargo target has {} as its root file",
                target_root.display()
            );
        };
        let results = if EXCLUDED.contains(&target.package.as_str()) {
            HashMap::new()
        } else {
            run_target(root, target, &names)?
        };
        for name in names {
            let outcome = if EXCLUDED.contains(&target.package.as_str()) {
                Outcome::Skipped
            } else {
                results.get(name).copied().unwrap_or(Outcome::NotFound)
            };
            outcomes.insert((target_root.to_path_buf(), name.to_owned()), outcome);
        }
    }
    Ok(outcomes)
}

/// A cargo target that can hold tests.
#[derive(Debug)]
struct Target {
    package: String,
    /// `--lib`, or `--bin <name>` / `--test <name>`.
    selector: Vec<String>,
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<MetaPackage>,
}

#[derive(Deserialize)]
struct MetaPackage {
    name: String,
    targets: Vec<MetaTarget>,
}

#[derive(Deserialize)]
struct MetaTarget {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

/// Workspace targets by canonical root file, from `cargo metadata`.
fn cargo_targets(root: &Path) -> Result<HashMap<PathBuf, Target>> {
    let output = Command::new(cargo())
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .context("running cargo metadata")?;
    if !output.status.success() {
        bail!("cargo metadata failed ({})", output.status);
    }
    let meta: Metadata =
        serde_json::from_slice(&output.stdout).context("parsing cargo metadata output")?;
    let mut targets = HashMap::new();
    for package in meta.packages {
        for t in package.targets {
            let selector = if t.kind.iter().any(|k| k == "lib" || k == "rlib") {
                vec!["--lib".to_owned()]
            } else if t.kind.iter().any(|k| k == "bin") {
                vec!["--bin".to_owned(), t.name]
            } else if t.kind.iter().any(|k| k == "test") {
                vec!["--test".to_owned(), t.name]
            } else {
                continue;
            };
            let path = t
                .src_path
                .canonicalize()
                .with_context(|| format!("resolving {}", t.src_path.display()))?;
            targets.insert(
                path,
                Target {
                    package: package.name.clone(),
                    selector,
                },
            );
        }
    }
    Ok(targets)
}

fn run_target(root: &Path, target: &Target, names: &[&str]) -> Result<HashMap<String, Outcome>> {
    let output = Command::new(cargo())
        .args(["test", "-p", &target.package])
        .args(&target.selector)
        .args(["--", "--exact"])
        .args(names)
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running the tests of {}", target.package))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let results = parse_libtest(&stdout);
    if !output.status.success() && results.is_empty() {
        bail!(
            "cargo test -p {} {} failed before running any test ({})",
            target.package,
            target.selector.join(" "),
            output.status
        );
    }
    Ok(results)
}

/// Parse libtest's human output: `test NAME ... ok|FAILED|ignored`.
fn parse_libtest(stdout: &str) -> HashMap<String, Outcome> {
    stdout
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("test ")?;
            let (name, status) = rest.split_once(" ... ")?;
            let name = name.strip_suffix(" - should panic").unwrap_or(name);
            let outcome = if status.starts_with("ok") {
                Outcome::Passed
            } else if status.starts_with("FAILED") {
                Outcome::Failed
            } else if status.starts_with("ignored") {
                Outcome::Ignored
            } else {
                return None;
            };
            Some((name.to_owned(), outcome))
        })
        .collect()
}

/// The cargo that runs this task, so toolchain overrides carry over.
fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_libtest_result_lines() {
        let out = "\
running 4 tests
test tests::a ... ok
test tests::b - should panic ... ok
test tests::c ... FAILED
test tests::d ... ignored, needs sshd
test tests::e has been running for over 60 seconds

test result: FAILED. 2 passed; 1 failed; 1 ignored
";
        let r = parse_libtest(out);
        assert_eq!(r.len(), 4);
        assert_eq!(r["tests::a"], Outcome::Passed);
        assert_eq!(r["tests::b"], Outcome::Passed);
        assert_eq!(r["tests::c"], Outcome::Failed);
        assert_eq!(r["tests::d"], Outcome::Ignored);
    }
}
