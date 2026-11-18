---
plan_format_version: 2
plan_id: PLAN-20261118-config-layout-command
title: Expose the resolved harness configuration-directory layout
status: implemented
created_at: 2026-11-18
updated_at: 2026-11-18
owner: jfb
source:
  type: manual
  url: null
  external_id: null
  imported_at: null
bug:
  summary: >
    Downstream installers cannot enumerate every harness configuration directory
    clanker resolves, because provider derivation and the multi-variable directory
    shape exist only inside clanker and no command exposes them.
  severity: medium
  affected_area: src/cli.rs, src/lib.rs, src/layout.rs, src/error.rs, README.md
  user_impact: >
    Tooling that installs skills, plugins, hooks, and MCP declarations into harness
    configuration roots must reimplement part of clanker's schema, and misses
    provider-derived roots and multi-variable roots.
execution:
  requires_operator_approval_before_implementation: true
  requires_plan_updates_during_execution: true
  task_graph_status: complete
project:
  slug: clanker
  remote: null
---

# ADR: Expose the resolved harness configuration-directory layout

## Problem Statement

Clanker resolves harness configuration directories at launch: it substitutes
`{context}` in each harness's `config_dir` template, expands `~`, and derives a
provider's directories from the harness's own template. Those rules live only in
`src/config.rs` (`ConfigDirectory`, `ConfigDirectory::for_provider`,
`provider_template`) and `src/launch.rs` (`resolve_config_directories`).

No command exposes the result. Tooling that needs to install persistent content into
every harness configuration root — skills, plugins, hooks, MCP declarations — therefore
reimplements a subset of the schema. Agent-content's `clanker_roots` reads only
`.harness.<name>.config_dir.path` and `[defaults].contexts`: it ignores the
multi-variable `directories` shape and has no notion of provider-derived roots. The
consequence observed on 2026-11-18 is that `~/.config/claude-zai/personal` and
`~/.config/claude-deepseek/personal` received no skills, plugins, or MCP servers, and
provider hooks were never registered.

`clanker exec` cannot serve as the enumeration interface: it omits every harness whose
declared directory is missing, exports only the resolved context, and replaces the
process. Enumeration must be read-only and must include directories that do not exist.

## Source Material

### Ticket

No ticket was provided. The requirement comes from an operator conversation on
2026-11-18 about making installation provider-aware.

### Discussion Summary

The operator identified that installation must read clanker's configuration and use it
to manipulate harness configurations. We evaluated putting installation in clanker
versus a separate installer, and concluded that clanker should own the layout while
content installation remains separate. The first deliverable is a read-only layout
command; agent-content will consume it in a separate plan.

## Context

Current behavior:

- `ConfigDirectory` has two shapes: `Single { environment, path }` and
  `Multiple { directories }`, exposed through `entries()`.
- A provider covers every harness with a `config_dir`; `ConfigDirectory::for_provider`
  and `provider_template` rewrite each template by suffixing one path component.
- A harness may restrict `contexts`; otherwise it uses `defaults.contexts`.
- `resolve_config_directories` in `src/launch.rs` performs `{context}` substitution,
  `~` expansion, and an existence check that fails the launch.
- `clanker exec` exports resolved directories, but only for existing directories and
  only for the selected context/provider.

Expected behavior after this plan:

- `clanker config dirs` prints one row per resolved directory for every harness with a
  `config_dir`, every supported context, and every provider, including the native
  (providerless) row. Each row carries harness, context, provider, variable, path, and
  existence.
- Text output is tab-separated with no header; `--json` emits the shared response
  envelope with an array of objects.
- Optional `--context` and `--provider` filters restrict the rows and fail loudly when
  the named context or provider is not configured.
- The command is read-only: it does not export environment variables, mint session
  markers, create directories, or execute anything.

## Constraints

- The command validates the merged configuration and fails loudly on an invalid one;
  it does not take a launch-time fallback.
- It must handle both directory shapes, restricted harness contexts, fixed
  single-context paths, and provider derivation.
- It must list missing directories with `exists = false` rather than failing.
- `~` expansion requires `HOME`; a configuration using only absolute or relative
  templates must work without `HOME`.
- Output order is deterministic: harness, context, provider (native first), variable.
- New code stays within RS-003 lint rules and RS-007 coverage.
- The command is not added to the agent surface; it is operator and tooling surface.

## Non-Goals

- Installing, copying, or deleting any file.
- Choosing a policy for creating missing provider directories.
- Changing launch, exec, prompt, project, or session behavior.
- Adding a content manifest to the runtime configuration.
- Exposing harness install surfaces (skills/commands/MCP paths); that is a separate
  contract.

## Decision

Add a `config` command group with a `dirs` operation:

```text
clanker config dirs [--context <name>] [--provider <name>] [--json]
```

Implementation:

- `src/cli.rs` adds `Command::Config { command: ConfigCommand }` and
  `ConfigCommand::Dirs { context: Option<String>, provider: Option<String> }`.
- A new `src/layout.rs` owns enumeration:
  - `DirectoryEntry { harness, context, provider, variable, path, exists }`;
  - `LayoutError { HomeNotSet, UnknownContext, UnknownProvider, Path }`;
  - `directories(config, home, filter)` iterates harnesses in `BTreeMap` order,
    supported contexts, each `entries()` pair, and native plus every configured
    provider.
  - `{context}` substitution and `~` expansion reuse `launch::expand_path`.
  - Filters match exact strings against configured contexts and provider names.
- `src/lib.rs` dispatches `Command::Config`, reuses `load_runtime_config`, and renders
  text or JSON through `render_response("config dirs", ...)`.
- `src/error.rs` adds `Layout(#[from] LayoutError)`.
- README documents the command, columns, JSON fields, filters, and the fact that
  missing directories are listed rather than rejected.

## Alternatives Considered

Extending `clanker exec` with a print-only mode. Rejected: exec's contract is process
replacement, its missing-directory policy omits exactly the roots enumeration must
show, and overloading it would make two unrelated behaviors share one command.

Extending `clanker env`. Rejected: it resolves one harness and requires every declared
directory to exist; it is a plan projection, not an inventory.

Extending `clanker list`. Rejected: list reports configured names for agents, and
adding a full row-per-directory payload would mix two audiences and change an existing
output contract.

Having agent-content derive provider paths in bash. Rejected: it duplicates
`provider_template` and `ConfigDirectory::for_provider`, which is the defect this plan
removes.

## Consequences

Agent-content and any other installer can enumerate every root from one command,
including provider roots and multi-variable harnesses, without parsing clanker's schema.
Missing provider roots become visible, which lets the installer choose whether to
report or provision them.

Risks: the command's output becomes a stable interface; additions should be additive.
It requires a clanker release before consumers can rely on it, so agent-content must
pin or document the minimum version.

## Operator Guidance Log

Append-only.

### 2026-11-18 — jfb (operator)

Question or decision needed: whether installation should move into clanker, into its
runtime configuration, or stay in a separate installer consuming a read-only layout
contract.

Decision or guidance received: keep installation separate; implement the read-only
layout command and the agent-content `clanker_roots` replacement.

Impact on plan: this plan covers the command only; the consumer change is planned
separately in agent-content.

## Decision Log

Append-only.

### 2026-11-18 — D001: the command is `clanker config dirs`

The command reports configuration-directory layout, not general configuration. A
`config` group can gain further read-only inspection operations without overloading an
existing command.

### 2026-11-18 — D002: filters are exact strings, not parsed names

`--context` and `--provider` match configured entries exactly and fail with the
unmatched value. This keeps the command diagnostic and avoids a second name-validation
path whose errors would differ from clanker's other commands.

### 2026-11-18 — D003: listing does not require the directories to exist

`exists` is data in each row. Failing on a missing directory would hide precisely the
provider roots the command exists to reveal, and would conflate enumeration with
launch-time validation.

# Task Graph

<!-- TASK_GRAPH:BEGIN -->
```yaml
tasks:
  - id: T001
    title: Implement the config dirs command and its layout enumeration
    status: done
    depends_on: []
    owner: null
    description: >
      Add the CLI surface, layout module, error variant, dispatch, and text/JSON
      rendering for `clanker config dirs`.
    work_items:
      - Add ConfigCommand::Dirs with optional --context and --provider string filters.
      - Add src/layout.rs with DirectoryEntry, LayoutError, and directories().
      - Reuse launch::expand_path for tilde expansion and config::provider_template for provider derivation.
      - Filter by exact context and provider strings, failing for unconfigured values.
      - Render TSV in text mode and the standard JSON envelope with harness, context, provider, variable, path, and exists.
      - Add ClankerError::Layout and dispatch Command::Config in src/lib.rs.
    invariants:
      - The command performs no filesystem writes and exports no environment variables.
      - Missing directories are reported with exists=false, not rejected.
      - Both directory shapes and restricted contexts are enumerated.
      - Native rows carry a null/empty provider.
    acceptance_checks:
      - Unit tests cover native and provider rows, both shapes, restricted contexts, filters, unknown filters, and HOME-less absolute templates.
      - An integration test drives the binary and asserts TSV columns and JSON fields, including an existing and a missing provider directory.
      - mise run check passes.
    completion_evidence: "Done 2026-11-18. src/cli.rs:98 adds ConfigCommand::Dirs; src/layout.rs:18 defines DirectoryEntry and :73 directories(); src/error.rs:50 adds ClankerError::Layout; src/lib.rs:517 dispatches run_config and :551 emits text/JSON. Unit tests in src/layout.rs cover native rows, provider derivation, both directory shapes, restricted contexts, filters, unknown filters, existence, HOME-less absolute templates, and the missing-HOME failure. tests/config_dirs.rs drives the binary for TSV, JSON, filters, unknown values, and creation-free behavior; it forwards LLVM_PROFILE_FILE so the subprocess is attributed. cargo nextest: 372 passed. mise run ci exited 0 with 97.22% line coverage."
  - id: T002
    title: Document the layout command and its output contract
    status: done
    depends_on:
      - T001
    owner: null
    description: >
      Add the command to README.md where harness configuration directories are
      described, including columns, JSON fields, filters, and missing-directory
      behavior.
    work_items:
      - Document `clanker config dirs` and the TSV column order.
      - Document the JSON object fields and that provider is null for native rows.
      - State that the command is read-only and lists missing directories.
    invariants:
      - No documentation claims the command installs or creates anything.
      - The documented column order matches the implementation.
    acceptance_checks:
      - mise run check passes, including spelling and documentation hooks.
    completion_evidence: "Done 2026-11-18. README.md:267 adds the Configuration layout section with the TSV column order, JSON fields, filters, read-only behavior, and exists=false for missing directories. mise run check passed every hook, including typos and taplo."
  - id: T003
    title: Run the full gate and record coverage
    status: done
    depends_on:
      - T001
      - T002
    owner: null
    description: >
      Run the repository CI gate, record the test count and measured line coverage,
      and update the stated coverage figure if it changed.
    work_items:
      - Run mise run ci.
      - Record the test count and line coverage in completion evidence.
      - Update README.md and REPO_INVARIANTS.md RS-007 if the measured figure changed.
    invariants:
      - No test is skipped, ignored, or deleted to pass the gate.
      - Coverage remains at or above 95% without new allow attributes.
    acceptance_checks:
      - mise run ci exits 0.
    completion_evidence: "Done 2026-11-18. mise run ci exited 0: all 18 prek hooks passed and check:coverage ran 372 tests, all passing, at 97.22% line coverage against the 95% floor. No test was skipped, ignored, or deleted and no allow attribute was added. README.md:58 and REPO_INVARIANTS.md RS-007 now state 97.22%."
```
<!-- TASK_GRAPH:END -->

# Task Details

## T001 — Implement the config dirs command and its layout enumeration

Status: `done`

Depends on: none

### Description

Add the CLI surface, layout module, error variant, dispatch, and text/JSON rendering
for `clanker config dirs`.

### Invariants

- The command performs no filesystem writes and exports no environment variables.
- Missing directories are reported with exists=false, not rejected.
- Both directory shapes and restricted contexts are enumerated.
- Native rows carry a null/empty provider.

### Acceptance Checks

- Unit tests cover native and provider rows, both shapes, restricted contexts, filters, unknown filters, and HOME-less absolute templates.
- An integration test drives the binary and asserts TSV columns and JSON fields, including an existing and a missing provider directory.
- mise run check passes.

### Completion Evidence

Done 2026-11-18. src/cli.rs:98 adds ConfigCommand::Dirs; src/layout.rs:18 defines
DirectoryEntry and :73 directories(); src/error.rs:50 adds ClankerError::Layout;
src/lib.rs:517 dispatches run_config and :551 emits text/JSON. Unit tests in
src/layout.rs cover native rows, provider derivation, both directory shapes,
restricted contexts, filters, unknown filters, existence, HOME-less absolute
templates, and the missing-HOME failure. tests/config_dirs.rs drives the binary
for TSV, JSON, filters, unknown values, and creation-free behavior. cargo
nextest: 372 passed. mise run ci exited 0 with 97.22% line coverage.

## T002 — Document the layout command and its output contract

Status: `done`

Depends on: T001

### Description

Add the command to README.md where harness configuration directories are described,
including columns, JSON fields, filters, and missing-directory behavior.

### Invariants

- No documentation claims the command installs or creates anything.
- The documented column order matches the implementation.

### Acceptance Checks

- mise run check passes, including spelling and documentation hooks.

### Completion Evidence

Done 2026-11-18. README.md:267 adds the Configuration layout section with the
TSV column order, JSON fields, filters, read-only behavior, and exists=false
for missing directories. mise run check passed every hook.

## T003 — Run the full gate and record coverage

Status: `done`

Depends on: T001, T002

### Description

Run the repository CI gate, record the test count and measured line coverage, and
update the stated coverage figure if it changed.

### Invariants

- No test is skipped, ignored, or deleted to pass the gate.
- Coverage remains at or above 95% without new allow attributes.

### Acceptance Checks

- mise run ci exits 0.

### Completion Evidence

Done 2026-11-18. mise run ci exited 0: all 18 prek hooks passed and
check:coverage ran 372 tests, all passing, at 97.22% line coverage against the
95% floor. README.md:58 and REPO_INVARIANTS.md RS-007 now state 97.22%.

# Execution Protocol

Agents executing this plan must:

- Read the full plan and verify a task's dependencies are `done` before starting it.
- Set a task to `in_progress` and update `updated_at` when starting.
- Update the plan when they find it wrong, discover missing context, change the
  approach, need operator guidance, or alter the task graph.
- When operator guidance is needed: add an Operator Guidance Log entry, set affected
  tasks to `blocked`, and not guess when guessing risks rework, data loss, security,
  or product-behavior changes.
- When completing a task: verify invariants, run or describe acceptance checks, record
  `completion_evidence`, set status `done`, update `updated_at`, and update ADR
  sections only when the task changed context, decision, alternatives, or consequences.
- Name, in the completion evidence, the files the task actually touched. A path and a
  line number beats a filename.
- Treat the Operator Guidance Log, Decision Log, and completion evidence as
  append-only; supersede prior entries with a new dated entry rather than deleting them.
