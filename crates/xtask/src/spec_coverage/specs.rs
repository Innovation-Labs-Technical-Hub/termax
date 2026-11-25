//! Reads requirements and their scenarios from `openspec/specs/*/spec.md`.

use std::path::Path;

use anyhow::{Context, Result};

/// One requirement and the acceptance scenarios that belong to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// `FR-UI09-001`, `NFR-00-003`, …
    pub id: String,
    /// Directory name under `openspec/specs/`.
    pub capability: String,
    /// `Module:` from the metadata line; NFRs have none and are grouped as
    /// `NFR-<BRD>` instead.
    pub module: String,
    /// `Phase:` from the metadata line (`P1`…`P3`); NFRs carry no phase.
    pub phase: Option<String>,
    /// Scenario names exactly as in the spec after `#### Scenario: `, with
    /// runs of whitespace collapsed (`FR-UI09-001 acceptance 1`).
    pub scenarios: Vec<String>,
}

/// Parse every `spec.md` under `specs_dir`, sorted by capability.
pub fn load(specs_dir: &Path) -> Result<Vec<Requirement>> {
    let mut dirs: Vec<_> = std::fs::read_dir(specs_dir)
        .with_context(|| format!("reading {}", specs_dir.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("listing {}", specs_dir.display()))?;
    dirs.sort_by_key(|e| e.file_name());
    let mut requirements = Vec::new();
    for entry in dirs {
        let spec = entry.path().join("spec.md");
        if !spec.is_file() {
            continue;
        }
        let capability = entry.file_name().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&spec)
            .with_context(|| format!("reading {}", spec.display()))?;
        requirements.extend(parse(&capability, &text));
    }
    Ok(requirements)
}

/// Collapse runs of whitespace so tags and headings compare reliably.
pub fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse one capability's spec. Only `### Requirement:` sections count;
/// scenarios under any other heading are ignored.
pub fn parse(capability: &str, text: &str) -> Vec<Requirement> {
    let mut out: Vec<Requirement> = Vec::new();
    let mut in_requirement = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("### Requirement:") {
            let Some(id) = rest.split_whitespace().next() else {
                in_requirement = false;
                continue;
            };
            out.push(Requirement {
                id: id.to_owned(),
                capability: capability.to_owned(),
                module: nfr_group(id),
                phase: None,
                scenarios: Vec::new(),
            });
            in_requirement = true;
        } else if line.starts_with("## ") || line.starts_with("### ") {
            in_requirement = false;
        } else if !in_requirement {
            continue;
        } else if let Some(name) = line.strip_prefix("#### Scenario:") {
            if let Some(req) = out.last_mut() {
                req.scenarios.push(normalize(name));
            }
        } else if line.starts_with("Source:")
            && let Some(req) = out.last_mut()
        {
            apply_metadata(req, line);
        }
    }
    out
}

/// `Source: BRD-08 FR-UI09-001 · Module: UI-09 · Priority: Must · Phase: P1`
fn apply_metadata(req: &mut Requirement, line: &str) {
    for field in line.split('·') {
        if let Some((key, value)) = field.split_once(':') {
            let value = value.trim();
            match key.trim() {
                "Module" if !value.is_empty() => req.module = value.to_owned(),
                "Phase" if !value.is_empty() => req.phase = Some(value.to_owned()),
                _ => {}
            }
        }
    }
}

/// Default group for a requirement without a `Module:` field: the id
/// without its sequence number (`NFR-00-003` → `NFR-00`).
fn nfr_group(id: &str) -> String {
    match id.rsplit_once('-') {
        Some((prefix, _)) => prefix.to_owned(),
        None => id.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = "\
# demo Specification

## Purpose

Source: BRD-08 “Agentic” v1.0. Modules: UI-09.

## Requirements

### Requirement: FR-UI09-001 Right-hand agent panel (Mod+J)
The system SHALL provide: Right-hand agent panel (Mod+J).

Source: BRD-08 FR-UI09-001 · Module: UI-09 · Priority: Must · Phase: P1
VI: Agent panel

#### Scenario: FR-UI09-001 specified behaviour
- **GIVEN** the system implements FR-UI09-001

#### Scenario: FR-UI09-001   acceptance 1
- **WHEN** Mod+J is pressed

### Requirement: NFR-08-002 Security
The system MUST meet this security target.

Source: BRD-08 NFR-08-002 · Category: Security

#### Scenario: NFR-08-002 target is met
- **WHEN** measured

## Interfaces

### IF-08-01 MCP server
#### Scenario: not a requirement scenario
";

    #[test]
    fn parses_requirements_metadata_and_scenarios() {
        let reqs = parse("agentic-ai-mcp", SPEC);
        assert_eq!(
            reqs,
            vec![
                Requirement {
                    id: "FR-UI09-001".into(),
                    capability: "agentic-ai-mcp".into(),
                    module: "UI-09".into(),
                    phase: Some("P1".into()),
                    scenarios: vec![
                        "FR-UI09-001 specified behaviour".into(),
                        "FR-UI09-001 acceptance 1".into(),
                    ],
                },
                Requirement {
                    id: "NFR-08-002".into(),
                    capability: "agentic-ai-mcp".into(),
                    module: "NFR-08".into(),
                    phase: None,
                    scenarios: vec!["NFR-08-002 target is met".into()],
                },
            ]
        );
    }

    #[test]
    fn scenarios_outside_requirements_are_ignored() {
        let reqs = parse("c", "#### Scenario: X acceptance 1\n## Interfaces\n");
        assert!(reqs.is_empty());
    }

    #[test]
    fn loads_the_real_specs() {
        let root = crate::spec_coverage::workspace_root();
        let reqs = load(&root.join("openspec/specs")).unwrap();
        assert!(reqs.len() > 100, "found only {} requirements", reqs.len());
        assert!(reqs.iter().all(|r| !r.scenarios.is_empty()));
        assert!(
            reqs.iter()
                .filter(|r| r.id.starts_with("FR-"))
                .all(|r| r.phase.is_some()),
            "every FR carries a phase"
        );
    }
}
