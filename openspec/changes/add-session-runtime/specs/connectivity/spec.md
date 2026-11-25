# Spec Delta

## ADDED Requirements

### Requirement: FR-CO01-011 Idle compaction of terminal history
The system SHALL compact a session's terminal history memory after the session's output has been quiet for a configurable period, without changing what the user sees, selects or finds by search.

Source: product decision 2026-11-24 (idle scrollback compression approved to meet NFR-00-003); evidence plans/reports/perf-261124-1028-terminal-core-baseline.md · Module: CO-01 · Priority: Must · Phase: P1  
Quiet period default: [TO CONFIRM · CHỜ XÁC NHẬN] (owner: Tech lead); provisional 60 s

#### Scenario: FR-CO01-011 quiet session is compacted
- **GIVEN** an SSH session on the Linux default terminal backend whose 80×24 terminal holds 10,000 lines of history
- **WHEN** no output arrives for the configured quiet period
- **THEN** the session's terminal memory is compacted once and the extra memory for that tab is at most 3 MiB (NFR-00-003)

#### Scenario: FR-CO01-011 compaction is invisible to the user
- **GIVEN** a compacted session with an active selection and a search query
- **WHEN** the user scrolls back, copies the selection and repeats the search
- **THEN** the visible rows, copied text and search matches are identical to those before compaction

#### Scenario: FR-CO01-011 busy sessions are not compacted
- **GIVEN** a session that receives output more often than the quiet period
- **WHEN** the runtime runs for longer than the quiet period
- **THEN** no compaction happens for that session
