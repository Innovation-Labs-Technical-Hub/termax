#!/usr/bin/env python3
"""Generate Termax OpenSpec capability specs from the BRD YAML sources.

Usage:
    python3 tools/openspec_from_brd.py <design-set>/02-BRD/yaml openspec/specs

One capability spec is written per BRD. Nothing is invented: every
requirement, scenario and section is a mechanical mapping of a BRD record,
and every requirement cites its BRD id and source evidence.

Mapping:
  functional_requirements[]  -> Requirement (id + EN title), SHALL statement,
                                acceptance[] -> Scenario (GIVEN/WHEN/THEN)
  non_functional_requirements[] -> Requirement with a measurement scenario
  business_rules, journeys, data_entities, interfaces, screens, scope_out,
  constraints, assumptions, release_plan -> descriptive sections
  open_questions             -> "Open questions" marked [TO CONFIRM · CHỜ XÁC NHẬN]

The source design set uses the codename OxTerm; names are rewritten to
Termax (product), termax-* (crates) and tmx (CLI) to match the repository.
Requires Python 3.9+ and PyYAML.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

import yaml

CAPABILITIES = {
    "BRD-00": "product-foundation",
    "BRD-01": "app-shell-design-system",
    "BRD-02": "hosts-and-import",
    "BRD-03": "terminal-workspace",
    "BRD-04": "connectivity",
    "BRD-05": "file-transfer",
    "BRD-06": "identity-keys-trust",
    "BRD-07": "automation-logging-cli",
    "BRD-08": "agentic-ai-mcp",
    "BRD-09": "plugins",
    "BRD-10": "cloud-accounts-sync",
    "BRD-11": "team-admin-billing",
    "BRD-12": "mobile-apps",
    "BRD-13": "platform-engineering",
}

CONFIRM = "[TO CONFIRM · CHỜ XÁC NHẬN]"
KEYWORDS = ("Given", "When", "Then", "And", "But")


def rename(text: str) -> str:
    """Apply the OxTerm -> Termax rename used throughout the repository."""
    text = re.sub(r"\boxt-", "termax-", text)
    text = re.sub(r"\boxt_", "termax_", text)
    text = re.sub(r"\bOXT_", "TMX_", text)
    text = re.sub(r"\bOXT-", "TMX-", text)
    text = re.sub(r"\boxt\b(?![-_.])", "tmx", text)
    return text.replace("OxTerm", "Termax").replace("oxterm", "termax")


def clean(value) -> str:
    return rename(" ".join(str(value).split())) if value is not None else ""


def scenario_steps(text: str) -> list[tuple[str, str]]:
    """Split 'Given A, When B, Then C, And D' into keyword/clause pairs."""
    text = clean(text)
    parts = re.split(r",?\s+(?=(?:When|Then|And|But)\s)", text)
    steps = []
    for part in parts:
        word, _, rest = part.partition(" ")
        if word in KEYWORDS and rest:
            steps.append((word.upper(), rest.rstrip(".")))
        elif steps:
            key, prev = steps[-1]
            steps[-1] = (key, f"{prev}, {part.rstrip('.')}")
        else:
            steps.append(("THEN", part.rstrip(".")))
    if not any(k == "THEN" for k, _ in steps) or not any(k in ("GIVEN", "WHEN") for k, _ in steps):
        # Unstructured acceptance text: keep it whole as the expected outcome.
        steps = [("WHEN", "the acceptance check is performed"), ("THEN", text.rstrip("."))]
    return steps


def render_scenario(name: str, steps: list[tuple[str, str]]) -> list[str]:
    out = [f"#### Scenario: {name}"]
    out += [f"- **{k}** {v}" for k, v in steps]
    return out + [""]


def fr_block(brd_id: str, fr: dict) -> list[str]:
    title = clean(fr["title_en"])
    out = [f"### Requirement: {fr['id']} {title}"]
    out.append(f"The system SHALL provide: {title}.")
    out.append("")
    meta = [
        f"Source: {brd_id} {fr['id']}",
        f"Module: {fr.get('module', '-')}",
        f"Priority: {fr.get('priority', '-')}",
        f"Phase: {fr.get('phase', '-')}",
    ]
    out.append(" · ".join(meta) + "  ")
    if fr.get("title_vi"):
        out.append(f"VI: {clean(fr['title_vi'])}")
    out.append("")
    # The validator caps requirement bodies at 500 characters and asks for
    # detail in scenarios, so the BRD description becomes the first scenario.
    if fr.get("desc_en"):
        out += render_scenario(
            f"{fr['id']} specified behaviour",
            [("GIVEN", f"the system implements {fr['id']}"), ("THEN", clean(fr["desc_en"]).rstrip("."))],
        )
    for i, ac in enumerate(fr.get("acceptance") or [], 1):
        text = ac.get("en") if isinstance(ac, dict) else ac
        out += render_scenario(f"{fr['id']} acceptance {i}", scenario_steps(text))
    return out


def nfr_block(brd_id: str, nfr: dict) -> list[str]:
    what = clean(nfr.get("en")).rstrip(".")
    target = clean(nfr.get("target"))
    category = nfr.get("category", "Quality")
    out = [f"### Requirement: {nfr['id']} {category}: {what}"]
    out.append(f"The system MUST meet this {category.lower()} target.")
    out.append("")
    out.append(f"Source: {brd_id} {nfr['id']} · Category: {category}  ")
    if nfr.get("vi"):
        out.append(f"VI: {clean(nfr['vi'])}")
    out.append("")
    steps = []
    measurement = clean(nfr.get("measurement")) or "the method agreed for this target"
    steps.append(("WHEN", f"measured by: {measurement}"))
    steps.append(("THEN", f"the result meets: {target}"))
    out += render_scenario(f"{nfr['id']} target is met", steps)
    return out


def bullets(items, fmt) -> list[str]:
    return [fmt(i) for i in (items or [])] + ([""] if items else [])


def section(title: str, lines: list[str]) -> list[str]:
    return [f"## {title}", "", *lines] if lines else []


def render(brd: dict) -> str:
    brd_id = brd["id"]
    cap = CAPABILITIES[brd_id]
    out = [f"# {cap} Specification", "", "## Purpose", ""]
    out.append(clean(brd.get("purpose_en")))
    out.append("")
    modules = ", ".join(brd.get("modules") or []) or "cross-cutting"
    out.append(
        f"Source: {brd_id} “{clean(brd.get('title_en'))}” v{brd.get('version', '?')} "
        f"({brd.get('date', '?')}). Modules: {modules}. "
        "Generated by `tools/openspec_from_brd.py`; edit through OpenSpec changes."
    )
    out.append("")

    out += ["## Requirements", ""]
    for fr in brd.get("functional_requirements") or []:
        out += fr_block(brd_id, fr)
    for nfr in brd.get("non_functional_requirements") or []:
        out += nfr_block(brd_id, nfr)

    out += section(
        "Business rules",
        bullets(brd.get("business_rules"), lambda r: f"- **{r['id']}**: {clean(r.get('en'))}"),
    )
    out += section(
        "Business goals",
        bullets(
            brd.get("business_goals"),
            lambda g: f"- **{g.get('id', '-')}**: {clean(g.get('en') or g.get('goal_en'))}"
            + (f" (metric: {clean(g['metric'])})" if g.get("metric") else ""),
        ),
    )
    out += section(
        "Constraints",
        bullets(brd.get("constraints"), lambda c: f"- {clean(c.get('en') if isinstance(c, dict) else c)}"),
    )
    out += section(
        "Assumptions",
        bullets(brd.get("assumptions"), lambda a: f"- {clean(a.get('en') if isinstance(a, dict) else a)}"),
    )
    out += section(
        "Release plan",
        bullets(
            brd.get("release_plan"),
            lambda r: "- " + "; ".join(f"{k}: {clean(v)}" for k, v in r.items() if not k.endswith("_vi")),
        ),
    )

    journeys = []
    for j in brd.get("journeys") or []:
        journeys.append(f"### {j['id']} {clean(j.get('title_en'))}")
        journeys += [f"{n}. {clean(s)}" for n, s in enumerate(j.get("steps_en") or [], 1)]
        journeys.append("")
    out += section("User journeys", journeys)

    out += section(
        "Data entities",
        bullets(
            brd.get("data_entities"),
            lambda e: f"- **{clean(e['name'])}**: {clean(e.get('fields'))}"
            + (f" — {clean(e['notes_en'])}" if e.get("notes_en") else ""),
        ),
    )
    out += section(
        "Interfaces",
        bullets(
            brd.get("interfaces"),
            lambda i: f"- **{i['id']} {clean(i.get('name'))}** ({clean(i.get('kind'))}): {clean(i.get('desc_en'))}",
        ),
    )
    out += section(
        "Screens",
        bullets(
            brd.get("screens"),
            lambda s: f"- **{s['id']} {clean(s.get('name_en'))}** ({clean(s.get('ref'))}): {clean(s.get('notes_en'))}",
        ),
    )
    out += section(
        "Source evidence",
        bullets(
            [fr for fr in brd.get("functional_requirements") or [] if fr.get("source")],
            lambda fr: f"- **{fr['id']}**: {clean(fr['source'])}",
        ),
    )
    out += section(
        "Out of scope",
        bullets(brd.get("scope_out"), lambda s: f"- {clean(s.get('en'))}"),
    )
    out += section(
        "Open questions",
        bullets(
            brd.get("open_questions"),
            lambda q: f"- {CONFIRM} **{q['id']}** (owner: {clean(q.get('owner'))}): {clean(q.get('en'))} "
            f"— VI: {clean(q.get('vi'))}",
        ),
    )
    return "\n".join(out).rstrip() + "\n"


def render_glossary(entries: list[tuple[str, dict]]) -> str:
    """Bilingual glossary, first definition of each term wins."""
    seen: dict[str, tuple[str, dict]] = {}
    for brd_id, g in entries:
        seen.setdefault(clean(g["term"]).lower(), (brd_id, g))
    cell = lambda t: clean(t).replace("|", "\\|")
    rows = [
        f"| {cell(g['term'])} | {cell(g.get('en'))} | {cell(g.get('vi'))} | {brd_id} |"
        for _, (brd_id, g) in sorted(seen.items())
    ]
    return "\n".join(
        [
            "# Termax glossary",
            "",
            "Bilingual terms from the BRDs (business language Vietnamese, code language English).",
            "Generated by `tools/openspec_from_brd.py`.",
            "",
            "| Term | English | Tiếng Việt | Source |",
            "|---|---|---|---|",
            *rows,
        ]
    ) + "\n"


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    src, dst = Path(sys.argv[1]), Path(sys.argv[2])
    totals = {"specs": 0, "fr": 0, "nfr": 0, "scenarios": 0, "questions": 0, "glossary": 0}
    glossary: list[tuple[str, dict]] = []
    for brd_id, cap in CAPABILITIES.items():
        brd = yaml.safe_load((src / f"{brd_id}.yaml").read_text(encoding="utf-8"))
        text = render(brd)
        target = dst / cap / "spec.md"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")
        totals["specs"] += 1
        totals["fr"] += len(brd.get("functional_requirements") or [])
        totals["nfr"] += len(brd.get("non_functional_requirements") or [])
        totals["scenarios"] += text.count("#### Scenario:")
        totals["questions"] += len(brd.get("open_questions") or [])
        glossary += [(brd_id, g) for g in brd.get("glossary") or []]
    glossary_text = render_glossary(glossary)
    (dst.parent / "glossary.md").write_text(glossary_text, encoding="utf-8")
    totals["glossary"] = glossary_text.count("\n| ") - 1
    print(" ".join(f"{k}={v}" for k, v in totals.items()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
