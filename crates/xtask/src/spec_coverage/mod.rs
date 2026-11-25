//! `cargo xtask spec-coverage`: how many OpenSpec acceptance scenarios are
//! linked to tests, and (with `--run`) how many of those tests pass.
//!
//! A test is linked to a scenario by a doc line naming the scenario exactly
//! as in the spec, after `#### Scenario: `:
//!
//! ```text
//! /// Verifies: FR-CO02-011 acceptance 1
//! #[tokio::test]
//! async fn changed_host_key_always_aborts() { … }
//! ```
//!
//! Tags that point at an unknown requirement or scenario, or that are not
//! on a `#[test]` function, are "dangling" and fail the run.

mod runner;
mod specs;
mod tags;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use runner::{Outcome, Outcomes};
use specs::Requirement;
use tags::TaggedTest;

pub struct Options {
    /// Report only requirements of this phase (`P1`, `P2`, `P3`).
    pub phase: Option<String>,
    /// Write the report as JSON to this file.
    pub json: Option<PathBuf>,
    /// Run the tagged tests; a scenario is covered when a linked test passes.
    pub run: bool,
}

/// The workspace root (two levels above this crate's manifest).
pub fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .unwrap_or(manifest)
        .to_path_buf()
}

pub fn run(opts: &Options) -> Result<bool> {
    let root = workspace_root();
    let requirements = specs::load(&root.join("openspec/specs"))?;
    let scan = tags::scan(&root.join("crates"))?;
    let outcomes = if opts.run {
        Some(runner::run(&root, &scan.tests)?)
    } else {
        None
    };
    let report = build_report(
        &root,
        &requirements,
        &scan,
        outcomes.as_ref(),
        opts.phase.as_deref(),
    );
    print_report(&report);
    if let Some(path) = &opts.json {
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(report.dangling.is_empty())
}

#[derive(Serialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub scenarios: usize,
    /// Scenarios with at least one tagged test.
    pub linked: usize,
    /// Scenarios with at least one tagged test that passed; absent unless
    /// the tests were run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered: Option<usize>,
}

impl Counts {
    fn add(&mut self, linked: bool, covered: Option<bool>) {
        self.scenarios += 1;
        self.linked += usize::from(linked);
        if let Some(c) = covered {
            *self.covered.get_or_insert(0) += usize::from(c);
        }
    }
}

#[derive(Serialize, Debug)]
pub struct Group {
    pub name: String,
    #[serde(flatten)]
    pub counts: Counts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<Group>,
}

#[derive(Serialize, Debug)]
pub struct TestRef {
    pub name: String,
    pub file: String,
    pub line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

#[derive(Serialize, Debug)]
pub struct ScenarioEntry {
    pub scenario: String,
    pub requirement: String,
    pub capability: String,
    pub module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub tests: Vec<TestRef>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Dangling {
    pub file: String,
    pub line: usize,
    pub tag: String,
    pub reason: String,
}

#[derive(Serialize, Debug)]
pub struct Report {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub total: Counts,
    pub capabilities: Vec<Group>,
    pub scenarios: Vec<ScenarioEntry>,
    pub dangling: Vec<Dangling>,
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

pub fn build_report(
    root: &Path,
    requirements: &[Requirement],
    scan: &tags::Scan,
    outcomes: Option<&Outcomes>,
    phase: Option<&str>,
) -> Report {
    // Scenario name → requirement, over all phases: a tag on an
    // out-of-phase scenario is valid, just not counted.
    let mut by_scenario: HashMap<&str, &Requirement> = HashMap::new();
    let mut by_id: HashMap<&str, &Requirement> = HashMap::new();
    for r in requirements {
        by_id.insert(&r.id, r);
        for s in &r.scenarios {
            by_scenario.insert(s, r);
        }
    }

    let mut links: HashMap<&str, Vec<&TaggedTest>> = HashMap::new();
    let mut dangling: Vec<Dangling> = scan
        .stray
        .iter()
        .map(|(t, reason)| Dangling {
            file: relative(root, &t.file),
            line: t.line,
            tag: t.scenario.clone(),
            reason: (*reason).to_owned(),
        })
        .collect();
    for test in &scan.tests {
        for tag in &test.tags {
            if let Some((name, _)) = by_scenario.get_key_value(tag.scenario.as_str()) {
                let tests = links.entry(name).or_default();
                if !tests.iter().any(|t| std::ptr::eq(*t, test)) {
                    tests.push(test);
                }
                continue;
            }
            let id = tag.scenario.split_whitespace().next().unwrap_or_default();
            let reason = match by_id.get(id) {
                Some(r) => format!(
                    "{id} has no such scenario (it has: {})",
                    r.scenarios.join(", ")
                ),
                None => format!("unknown requirement `{id}`"),
            };
            dangling.push(Dangling {
                file: relative(root, &tag.file),
                line: tag.line,
                tag: tag.scenario.clone(),
                reason,
            });
        }
    }
    dangling.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    let outcome_of = |t: &TaggedTest| {
        outcomes.map(|o| {
            o.get(&(t.target_root.clone(), t.name.clone()))
                .copied()
                .unwrap_or(Outcome::NotFound)
        })
    };

    let mut total = Counts::default();
    let mut caps: BTreeMap<&str, (Counts, BTreeMap<&str, Counts>)> = BTreeMap::new();
    let mut scenarios = Vec::new();
    for r in requirements {
        if phase.is_some() && r.phase.as_deref() != phase {
            continue;
        }
        for s in &r.scenarios {
            let tests = links.get(s.as_str()).map(Vec::as_slice).unwrap_or(&[]);
            let refs: Vec<TestRef> = tests
                .iter()
                .map(|t| TestRef {
                    name: t.name.clone(),
                    file: relative(root, &t.file),
                    line: t.line,
                    outcome: outcome_of(t),
                })
                .collect();
            let linked = !refs.is_empty();
            let covered = outcomes.map(|_| refs.iter().any(|t| t.outcome == Some(Outcome::Passed)));
            total.add(linked, covered);
            let (cap, modules) = caps.entry(&r.capability).or_default();
            cap.add(linked, covered);
            modules.entry(&r.module).or_default().add(linked, covered);
            scenarios.push(ScenarioEntry {
                scenario: s.clone(),
                requirement: r.id.clone(),
                capability: r.capability.clone(),
                module: r.module.clone(),
                phase: r.phase.clone(),
                tests: refs,
            });
        }
    }

    let capabilities = caps
        .into_iter()
        .map(|(name, (counts, modules))| Group {
            name: name.to_owned(),
            counts,
            modules: modules
                .into_iter()
                .map(|(name, counts)| Group {
                    name: name.to_owned(),
                    counts,
                    modules: Vec::new(),
                })
                .collect(),
        })
        .collect();
    Report {
        phase: phase.map(str::to_owned),
        total,
        capabilities,
        scenarios,
        dangling,
    }
}

fn pct(n: usize, of: usize) -> f64 {
    if of == 0 {
        0.0
    } else {
        n as f64 * 100.0 / of as f64
    }
}

fn row(name: &str, c: &Counts) {
    let covered = c.covered.map_or_else(|| "-".to_owned(), |n| n.to_string());
    println!(
        "{name:<32} {:>9} {:>7} {:>7} {:>8.1}%",
        c.scenarios,
        c.linked,
        covered,
        pct(c.covered.unwrap_or(c.linked), c.scenarios)
    );
}

fn print_report(report: &Report) {
    let scope = report
        .phase
        .as_deref()
        .map_or_else(|| "all phases".to_owned(), |p| format!("phase {p}"));
    let measure = if report.total.covered.is_some() {
        "covered"
    } else {
        "linked"
    };
    println!("spec coverage, {scope} (percentage = {measure} / scenarios)\n");
    println!(
        "{:<32} {:>9} {:>7} {:>7} {:>9}",
        "capability / module", "scenarios", "linked", "covered", "%"
    );
    for cap in &report.capabilities {
        row(&cap.name, &cap.counts);
        for m in cap.modules.iter().filter(|m| m.counts.linked > 0) {
            row(&format!("  {}", m.name), &m.counts);
        }
    }
    row("total", &report.total);

    let problems: Vec<_> = report
        .scenarios
        .iter()
        .flat_map(|s| s.tests.iter().map(move |t| (s, t)))
        .filter(|(_, t)| !matches!(t.outcome, None | Some(Outcome::Passed)))
        .collect();
    if !problems.is_empty() {
        println!("\ntagged tests that did not pass:");
        for (s, t) in problems {
            println!(
                "  {}:{} {} ({:?}) for {}",
                t.file,
                t.line,
                t.name,
                t.outcome.unwrap_or(Outcome::NotFound),
                s.scenario
            );
        }
    }
    if !report.dangling.is_empty() {
        println!("\ndangling tags:");
        for d in &report.dangling {
            println!(
                "  {}:{} `Verifies: {}`: {}",
                d.file, d.line, d.tag, d.reason
            );
        }
    }
    let c = report.total;
    println!(
        "\n{scope}: {}/{} scenarios {measure} ({:.1}%), {} dangling tag(s)",
        c.covered.unwrap_or(c.linked),
        c.scenarios,
        pct(c.covered.unwrap_or(c.linked), c.scenarios),
        report.dangling.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::{Scan, Tag};

    fn req(id: &str, capability: &str, module: &str, phase: &str, n: usize) -> Requirement {
        Requirement {
            id: id.into(),
            capability: capability.into(),
            module: module.into(),
            phase: Some(phase.into()),
            scenarios: (1..=n).map(|i| format!("{id} acceptance {i}")).collect(),
        }
    }

    fn test(name: &str, tags: &[&str]) -> TaggedTest {
        TaggedTest {
            target_root: PathBuf::from("/w/crates/p/src/lib.rs"),
            name: name.into(),
            file: PathBuf::from("/w/crates/p/src/x.rs"),
            line: 1,
            tags: tags
                .iter()
                .map(|s| Tag {
                    scenario: (*s).into(),
                    file: PathBuf::from("/w/crates/p/src/x.rs"),
                    line: 1,
                })
                .collect(),
        }
    }

    fn fixture() -> (Vec<Requirement>, Scan) {
        let reqs = vec![
            req("FR-AG03-001", "agentic", "AG-03", "P1", 2),
            req("FR-AG03-002", "agentic", "AG-03", "P2", 1),
            req("FR-CO01-001", "connectivity", "CO-01", "P1", 1),
        ];
        let scan = Scan {
            tests: vec![
                test(
                    "x::a",
                    &["FR-AG03-001 acceptance 1", "FR-AG03-002 acceptance 1"],
                ),
                test(
                    "x::b",
                    &[
                        "FR-AG03-001 acceptance 1",
                        "FR-AG03-001 acceptance 7",
                        "FR-ZZ99-001 acceptance 1",
                    ],
                ),
            ],
            stray: vec![(
                Tag {
                    scenario: "FR-CO01-001 acceptance 1".into(),
                    file: PathBuf::from("/w/crates/p/src/y.rs"),
                    line: 4,
                },
                "not on a #[test] function",
            )],
        };
        (reqs, scan)
    }

    #[test]
    fn counts_linked_scenarios_per_capability_and_module() {
        let (reqs, scan) = fixture();
        let report = build_report(Path::new("/w"), &reqs, &scan, None, None);
        assert_eq!(
            report.total,
            Counts {
                scenarios: 4,
                linked: 2,
                covered: None
            }
        );
        let agentic = &report.capabilities[0];
        assert_eq!(
            (agentic.name.as_str(), agentic.counts.linked),
            ("agentic", 2)
        );
        assert_eq!(agentic.modules[0].name, "AG-03");
        let first = &report.scenarios[0];
        assert_eq!(first.scenario, "FR-AG03-001 acceptance 1");
        assert_eq!(first.tests.len(), 2);
    }

    #[test]
    fn phase_filter_keeps_only_that_phase() {
        let (reqs, scan) = fixture();
        let report = build_report(Path::new("/w"), &reqs, &scan, None, Some("P1"));
        assert_eq!(report.total.scenarios, 3);
        assert_eq!(report.total.linked, 1);
    }

    #[test]
    fn reports_dangling_tags_with_reasons() {
        let (reqs, scan) = fixture();
        let report = build_report(Path::new("/w"), &reqs, &scan, None, None);
        let reasons: Vec<_> = report
            .dangling
            .iter()
            .map(|d| (d.file.as_str(), d.tag.as_str(), d.reason.as_str()))
            .collect();
        assert_eq!(reasons.len(), 3);
        assert!(reasons.contains(&(
            "crates/p/src/y.rs",
            "FR-CO01-001 acceptance 1",
            "not on a #[test] function"
        )));
        assert!(
            reasons
                .iter()
                .any(|(_, tag, reason)| *tag == "FR-AG03-001 acceptance 7"
                    && reason.starts_with("FR-AG03-001 has no such scenario"))
        );
        assert!(
            reasons
                .iter()
                .any(|(_, _, reason)| *reason == "unknown requirement `FR-ZZ99-001`")
        );
    }

    #[test]
    fn covered_needs_a_passing_test() {
        let (reqs, scan) = fixture();
        let lib = PathBuf::from("/w/crates/p/src/lib.rs");
        let outcomes: Outcomes = [
            ((lib.clone(), "x::a".to_owned()), Outcome::Failed),
            ((lib, "x::b".to_owned()), Outcome::Passed),
        ]
        .into();
        let report = build_report(Path::new("/w"), &reqs, &scan, Some(&outcomes), None);
        // acceptance 1 of FR-AG03-001 has a passing test (b); FR-AG03-002
        // acceptance 1 is linked only to the failing test (a).
        assert_eq!(report.total.linked, 2);
        assert_eq!(report.total.covered, Some(1));
    }
}
