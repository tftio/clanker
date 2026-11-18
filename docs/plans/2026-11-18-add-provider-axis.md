---
plan_format_version: 2
plan_id: PLAN-20261118-add-provider-axis
title: Add a provider selection axis so one harness can run against several endpoints with separate state
status: approved
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
    clanker cannot launch one harness against a second inference endpoint (for
    example Claude Code against Z.ai) with its own config directory while still
    selecting the context, so the second endpoint either shares the context's
    history and credentials or needs a separate context that cannot also be
    personal or work.
  severity: medium
  affected_area: src/axis.rs, src/context.rs, src/cli.rs, src/config.rs, src/launch.rs, src/session.rs, src/lib.rs, src/main.rs
  user_impact: >
    The operator cannot run Claude Code against Z.ai GLM in both the personal and
    work contexts without either mixing GLM and Claude sessions in one history or
    giving up the context distinction.
execution:
  requires_operator_approval_before_implementation: true
  requires_plan_updates_during_execution: true
  task_graph_status: ready
project:
  slug: clanker
  remote: null
---

<!-- AGENT_GENERATED: true; AGENT_MODEL: claude-opus-5-5; AGENT_DATE: 2026-11-18 -->

# ADR: Add a provider selection axis to clanker

## Problem Statement

clanker launches a harness (`claude`, `codex`, `opencode`, `pi`) after resolving three
selection axes: context, domain, and model. Context selects the harness config
directory, for Claude Code `CLAUDE_CONFIG_DIR=~/.config/claude/{context}`, and that
directory holds the harness's credentials, settings, plugins, and session history.

The operator wants to run Claude Code against a third-party Anthropic-compatible
endpoint, Z.ai's GLM Coding Plan (`https://api.z.ai/api/anthropic`), and later DeepSeek
(`https://api.deepseek.com/anthropic`), in either the personal or the work context. Two
requirements follow from operator guidance on 2026-11-18:

1. The endpoint choice is orthogonal to context. `personal` with Z.ai and `work` with
   Z.ai are both valid launches.
2. Each endpoint keeps its own state. A GLM session and a Claude session never share a
   config directory, so history, `--resume`, settings, and credentials stay separate per
   endpoint and per context.

The current configuration surface cannot express this:

- A model alias (`[model.<name>]`) applies literal `env` and `harness_args` in every
  context, but it cannot change the config directory. `CLAUDE_CONFIG_DIR` is reserved
  against model and domain `env` (`Config::reserved_environment`, `src/config.rs`), and
  a config directory template accepts only the `{context}` placeholder
  (`resolve_config_directories`, `src/launch.rs`).
- A second harness entry (for example `claude-zai` with `bin = "claude"` and its own
  `config_dir`) separates the directories, but `clanker exec` then fails: two harnesses
  resolving `CLAUDE_CONFIG_DIR` to different paths is a hard
  `ConfigDirectoryCollisionError` (`build_exec_plan`, `src/launch.rs`).
- A separate context (for example `zai`) separates the directories but cannot be
  combined with `personal` or `work`.

## Source Material

### Ticket

No ticket was provided. The requirement comes from an operator conversation on
2026-11-18, summarised below.

### Discussion Summary

- The operator asked for a clanker-supported way to run Claude Code against Z.ai with a
  separate config directory.
- A first proposal added a `zai` context. The operator rejected it: the endpoint should
  be a different dimension, because Z.ai is wanted in both personal and work contexts.
- A second attempt used model aliases (`[model.glm]` with `ANTHROPIC_BASE_URL` in `env`
  and an `apiKeyHelper` passed through `--settings`). It was applied to
  `~/.config/clanker/local.toml` and then reverted. The operator rejected it because GLM
  and Claude sessions would share the context's config directory and history.
- The operator then authorised clanker changes and asked for a plan before any code.

### External facts this plan relies on

- Claude Code's authentication precedence ranks `apiKeyHelper` above subscription OAuth
  from `/login` (code.claude.com/docs/en/authentication, "Authentication precedence").
  A provider directory without a `/login` credential therefore has no subscription token
  to leak, and one with an `apiKeyHelper` uses that helper.
- `apiKeyHelper` output is sent as both `X-Api-Key` and `Authorization: Bearer`
  (code.claude.com/docs/en/settings-reference, `apiKeyHelper`). Z.ai and DeepSeek both
  accept bearer authentication.
- A Claude Code session that moves between GLM and Claude in one history can fail with
  HTTP 400, because Z.ai emits `server_tool_use` blocks Anthropic rejects
  (github.com/betmoar/cc-proxy-plugin issue 67). Separate directories avoid this.
- The operator's sops document `~/.config/sops/secrets.sops.yaml` already holds
  `env.ZAI_API_KEY` and `env.DEEPSEEK_API_KEY`, decryptable with the age key at
  `~/.config/sops/age/keys.txt`, which clanker's default passthrough (`HOME`,
  `XDG_CONFIG_HOME`) reaches.

## Context

Existing behaviour:

- `src/axis.rs` defines `CONTEXT`, `DOMAIN`, and `MODEL` axes with one precedence rule:
  flag, nearest `.clanker`, `CLANKER_<AXIS>`, configured default.
- `src/context.rs` parses `.clanker` into `DirectoryConfig { context, domain, model,
  project }` with `deny_unknown_fields`, merging ancestors per field.
- `src/cli.rs` parses `--context`, `--domain`, `--model`, and their `=` forms for
  launches; `parse_exec` takes only a context.
- `src/main.rs` reads `CLANKER_CONTEXT`, `CLANKER_DOMAIN`, `CLANKER_MODEL` once.
- `src/launch.rs` builds launch plans (`build_command_plan`, `build_symlink_plan`), exec
  plans (`build_exec_plan`), resolves config directories, applies model `env` and
  `harness_args`, and exports session markers with `SESSION_VERSION = "5"`.
- `src/session.rs` parses the markers back for `clanker current`, accepting only the
  version it was built for.
- `src/config.rs` validates harnesses, models, and domains, and reserves config-directory
  variables and clanker-owned variables against `env` overrides.

Expected behaviour after this plan:

- `clanker claude --provider zai --context work` launches Claude Code with the `zai`
  provider's config directory for `work` (for example `~/.config/claude-zai/work`), the
  provider's literal `env`, and the provider's `harness_args`.
- With no provider selected, every launch, exec, and session marker except the version
  is identical to today.
- `clanker exec --provider zai -- <argv>` exports the provider's directories for
  harnesses the provider covers and the native directories for the rest.

Affected systems: the clanker binary; the operator's `~/.config/clanker/local.toml` and
fish aliases (T009); agent-content's eval goldens, which snapshot dry-run JSON and
`CLANKER_SESSION_VERSION` (T008).

## Constraints

- Separate state is the point. For every harness a provider covers, every resolved
  provider config directory differs from the harness's native directory for the same
  context, and validation enforces it.
- Fail closed. A provider directory that does not exist stops the launch, exactly as a
  missing context directory does today. clanker never creates it.
- No secret in clanker config (ENG-002). Provider `env` and `harness_args` are literal,
  non-secret values. A credential reaches the harness only through a mechanism that
  reads it at use time, such as an `apiKeyHelper` in the provider directory's own
  `settings.json`.
- No behaviour change without a provider. The existing tests and agent-content's goldens
  differ only by the session version and the new fields, which carry empty values.
- Repository invariants hold: RS-003 lint set, RS-007 coverage at or above 95%, CLI-001
  parsing in `src/main.rs` and `src/cli.rs` with behaviour in the library, ENG-009 closed
  sum types without catch-all arms.

## Non-Goals

- Recreating, migrating, or copying any existing harness config directory. Creating the
  provider directories and their `settings.json` is operator setup in T009.
- Provider support in the base `config/config.toml`. The base config ships with no
  providers; providers are defined in the operator's overlay.
- A configured default provider (`defaults.provider`). The provider axis has no
  configured-default tier; absence of a provider means the native endpoint.
- Changing domain credential grants, the passthrough list, or the sandbox wrapper.
- Fixing the unrelated problems noticed during discovery: the personal Claude
  `SessionEnd` hook path `/Users/jfb/...` on Linux, and the stale clanker `main`
  checkout that predates the public history.

## Decision

Add a fourth selection axis, `provider`, resolved by the shared rule in `src/axis.rs`:
`--provider` flag, then `provider =` in the nearest `.clanker`, then `CLANKER_PROVIDER`,
with no configured-default tier. An unresolved provider means the harness's native
endpoint and directories.

A provider is a top-level table keyed by name, with one sub-table per harness it covers:

```toml
[provider.zai.harness.claude]
env = { ANTHROPIC_BASE_URL = "https://api.z.ai/api/anthropic", ANTHROPIC_DEFAULT_OPUS_MODEL = "glm-5.3", ANTHROPIC_DEFAULT_SONNET_MODEL = "glm-5.3", ANTHROPIC_DEFAULT_HAIKU_MODEL = "glm-5.3-flash", API_TIMEOUT_MS = "3000000" }
harness_args = []

[provider.zai.harness.claude.config_dir]
environment = "CLAUDE_CONFIG_DIR"
path = "~/.config/claude-zai/{context}"
```

- `config_dir` is required, uses the existing `ConfigDirectory` type in either shape,
  must declare exactly the variables the harness's own `config_dir` declares, follows
  the same placeholder rule, and must not equal the native template for any variable.
- `env` and `harness_args` are optional literal values. `env` is subject to the same
  name and reservation checks as model `env`.
- Selecting a provider that does not cover the launched harness is an error
  (`ProviderHarnessUnsupported`), not a silent fallback to the native endpoint.
- Launch order: harness `default_args`, injected prompt, provider `harness_args`, model
  `harness_args`, passthrough. Environment: domain, then provider (new
  `Provenance::Provider`), then model. A model and a provider for the same harness may
  not set the same `env` key; validation reports it.
- `clanker exec` resolves at most one provider, so the collision rule is unchanged.
- Session markers gain `CLANKER_SESSION_PROVIDER` and `CLANKER_SESSION_PROVIDER_SOURCE`.
  `SESSION_VERSION` moves from 5 to 6.
- Symlink mode (`<harness>-launch`) resolves provider from `.clanker` and
  `CLANKER_PROVIDER`, as it already does for context.

## Alternatives Considered

A `zai` context. Rejected by the operator: it cannot be combined with personal or work.

Model aliases carrying the endpoint. Rejected by the operator: the session shares the
context's config directory and history. It also cannot be fixed inside the model axis
without letting a model rewrite reserved config-directory variables, which would make
every model alias a possible credential switch.

A second harness entry per endpoint (`claude-zai`). Separates directories with no code
change, but `clanker exec` fails on the `CLAUDE_CONFIG_DIR` collision in every context
where both directories exist. Relaxing the collision rule would let exec export one
arbitrary directory for a variable two harnesses claim, which the rule exists to prevent.

A `{provider}` placeholder in the harness's own `config_dir` template. Fewer config
keys, but it forces one naming scheme on every provider, needs a reserved spelling for
"no provider", and hides the directory a provider uses from the provider's own table.

Per-provider default for `defaults.provider`. Deferred: a configured default would make
a provider the silent default endpoint, and the operator has asked for nothing that
needs it. `.clanker` already covers per-directory defaults.

## Consequences

Expected: `clanker claude --provider zai` and `--provider deepseek` launch Claude Code
against those endpoints in any context with separate state. The fish aliases `glm` and
`deepseek` become one-line wrappers (T009).

Risks:

- Session marker version 6 makes sessions launched by an older clanker report
  `unsupported clanker session marker version` until relaunched. This is the existing
  version contract working.
- agent-content's eval goldens snapshot `CLANKER_SESSION_VERSION` and dry-run JSON, so
  they must be regenerated against the new binary (T008).
- Each provider directory is a full Claude Code config directory. Plugins, hooks, and
  skills are installed into it separately (`clanker exec --provider zai -- claude plugin
  ...`), so the operator maintains one more directory per provider per context.

Follow-up: if a provider is wanted for `opencode` or `pi`, it is a config change only.

## Operator Guidance Log

Append-only. Agents add a dated entry whenever they need operator guidance or operator
guidance changes the plan.

### 2026-11-18 — jfb (operator)

Question or decision needed: how to run Claude Code against Z.ai under clanker.

Decision or guidance received: not a new context; a different dimension, usable in both
personal and work contexts. The models must not share history. clanker changes are
allowed; the checkout is `~/Projects/clanker`. Write a plan before changing code.

Impact on plan: the provider axis with per-provider config directories, and the
separate-directory constraint.

### 2026-11-18 — open question for the operator

Question or decision needed: the public `origin/main` history carries no `docs/plans/`
directory; the plans in the old private history were dropped at the public release.
Should this plan be committed with the feature branch, or kept outside the repository?

Decision or guidance received: pending.

Impact on plan: none on the task graph; it decides where this file lives.

### 2026-11-18 — jfb (operator)

Question or decision needed: approval of this plan, and where it lives (the open question above).

Decision or guidance received: the plan is approved as written. It is committed on the `feat/provider-axis` branch under `docs/plans/`.

Impact on plan: implementation may begin with T001. The pending question in the earlier entry is answered.

## Decision Log

Append-only. Decisions affecting architecture, behavior, sequencing, or tradeoffs.

### 2026-11-18 — D001: the axis is named `provider`

It names the inference endpoint a harness talks to, which is what changes. `backend`
was the alternative and is less specific.

### 2026-11-18 — D002: per-provider, per-harness tables carry the directory override

The provider's directory is declared in the provider's own table rather than computed
from a placeholder in the harness template. See Alternatives Considered.

### 2026-11-18 — D003: no configured-default tier for provider

Absence of a provider means the native endpoint. A default provider is out of scope.

### 2026-11-18 — D004: work happens on `feat/provider-axis` from `origin/main`

The local `main` is the pre-public private history, 86 commits ahead of and 11 behind
`origin/main`, with an uncommitted `mise.lock` change. It is left untouched. The
worktree is `~/Projects/clanker/.worktrees/provider-axis`.

### 2026-11-18 — D005: supersedes D004's description of the local checkout

The operator reset the local `main` to `origin/main`. `main`, `origin/main`, and
`feat/provider-axis` all point at `45a016f`, and the uncommitted `mise.lock` change is
gone. Work still happens on `feat/provider-axis` in the same worktree; the stale private
history no longer exists locally.

# Task Graph

<!-- TASK_GRAPH:BEGIN -->
```yaml
tasks:
  - id: T001
    title: Add the provider schema and its validation to the runtime config
    status: not_started
    depends_on: []
    owner: null
    description: >
      Add ProviderName, ProviderConfig, and ProviderHarnessConfig to src/config.rs
      and a top-level `provider` map to Config, defaulting to empty so existing
      configs parse unchanged. Validate every provider in Config::issues.
    work_items:
      - Add ProviderName with the same name grammar as the other axis names.
      - Add a `provider` field of type `BTreeMap<ProviderName, ProviderConfig>` with serde default; ProviderConfig holds a `harness` field of type `BTreeMap<HarnessName, ProviderHarnessConfig>`; ProviderHarnessConfig holds `config_dir` (required ConfigDirectory), `env` (default empty), `harness_args` (default empty), all with deny_unknown_fields.
      - Validate that each covered harness exists and declares a config_dir.
      - Validate that the provider config_dir declares exactly the harness config_dir's variables and follows the same placeholder rule as the harness.
      - Validate that no provider template equals the native template for the same variable.
      - Validate provider env names with check_environment_name and check_not_reserved, and report a key set by both a provider and a model for the same harness.
      - Validate that each provider covers at least one harness.
    invariants:
      - A config with no provider table produces the same issues as before this task.
      - Every new ConfigIssue key is a dotted path under `provider.<name>.harness.<harness>`.
    acceptance_checks:
      - Unit tests in src/config.rs cover each new issue, including the equal-to-native template, a variable-set mismatch, an unknown harness, a harness with no config_dir, a reserved env key, and a provider/model env overlap.
      - A test asserts a valid provider table produces no issues.
      - mise run check passes.
    completion_evidence: null
  - id: T002
    title: Resolve the provider axis from flag, .clanker, and CLANKER_PROVIDER
    status: not_started
    depends_on:
      - T001
    owner: null
    description: >
      Add the PROVIDER axis and thread its three tiers to the launch and exec
      entry points, with resolution and registry checks in the library.
    work_items:
      - Add `axis::PROVIDER` (name provider, flag --provider, environment CLANKER_PROVIDER, configured_default label "none").
      - Add `provider` to DirectoryConfig and to DirectoryConfig::inherit_from in src/context.rs.
      - Parse `--provider` and `--provider=` in the launch parser in src/cli.rs; add a provider override to ExecRequest and parse_exec, and to the exec subcommand definition.
      - Read CLANKER_PROVIDER in src/main.rs and carry it on LaunchInputs and the lib.rs runtime context.
      - Add a resolve_provider function returning an optional Resolved<ProviderName>, with LaunchError variants UnknownProvider and ProviderHarnessUnsupported that name the supplying tier.
    invariants:
      - With no provider in any tier, resolution returns None and no error.
      - The precedence order is flag, .clanker, environment, matching the other axes.
    acceptance_checks:
      - Unit tests cover each tier winning, nearest-ancestor .clanker inheritance of provider, an empty CLANKER_PROVIDER falling through, an invalid name, an unknown provider, and a provider that does not cover the harness.
      - CLI parser tests cover --provider, --provider=, a missing value, and the exec form.
      - mise run check passes.
    completion_evidence: null
  - id: T003
    title: Apply the provider to launch plans
    status: not_started
    depends_on:
      - T002
    owner: null
    description: >
      Use the provider's config_dir, env, and harness_args in command-mode and
      symlink-mode launch plans, and report the provider in dry-run output.
    work_items:
      - Resolve config directories from the provider's ConfigDirectory when a provider is selected, with the existing existence check and MissingConfigDirectory error.
      - Add Provenance::Provider and apply provider env after domain env and before model env.
      - Insert provider harness_args after the injected prompt and before model harness_args.
      - Resolve provider in build_symlink_plan from .clanker and CLANKER_PROVIDER.
      - Add provider and provider_source to LaunchPlan, LaunchPlanOutput, and the text dry-run output.
    invariants:
      - With no provider, the plan's args and environment are identical to the plan built before this task, apart from session markers added in T005.
      - A selected provider's directory that does not exist fails the launch and exports no directory.
      - The provider's directory is never the native directory for the same context.
    acceptance_checks:
      - A test builds the same launch with and without a provider and asserts the config directory, env provenance, and argument order differ only as specified.
      - A test asserts a missing provider directory returns MissingConfigDirectory naming the provider path.
      - A test asserts symlink mode honours CLANKER_PROVIDER.
      - An integration test in tests/ drives `clanker claude --provider <p> --dry-run --json` and asserts the provider fields and CLAUDE_CONFIG_DIR.
      - mise run check passes.
    completion_evidence: null
  - id: T004
    title: Apply the provider to clanker exec
    status: not_started
    depends_on:
      - T003
    owner: null
    description: >
      Make `clanker exec --provider <p>` export the provider's directories for the
      harnesses it covers and the native directories for every other harness.
    work_items:
      - Resolve provider in build_exec_plan from the request, .clanker, and CLANKER_PROVIDER.
      - Substitute the provider ConfigDirectory per covered harness before the collision check.
      - Keep the skip-and-warn behaviour for missing directories per harness.
    invariants:
      - The collision rule is unchanged and still fires for two native harnesses that disagree.
      - With no provider, the exec plan is identical to the plan built before this task.
    acceptance_checks:
      - Tests in tests/clanker_exec.rs assert the provider directory for a covered harness, the native directory for an uncovered one, and a skip warning when the provider directory is missing.
      - mise run check passes.
    completion_evidence: null
  - id: T005
    title: Export provider session markers and report them
    status: not_started
    depends_on:
      - T003
    owner: null
    description: >
      Add CLANKER_SESSION_PROVIDER and CLANKER_SESSION_PROVIDER_SOURCE, move the
      marker contract to version 6, and surface the provider in `clanker current`,
      `clanker env`, and `clanker list`.
    work_items:
      - Add the two markers to SESSION_ENVIRONMENTS and apply_session_environment, with empty values when no provider resolved.
      - Change SESSION_VERSION to "6" and update CurrentSession parsing and its text and JSON output.
      - Accept --provider in `clanker env` launch-selection flags.
      - List configured providers and the harnesses each covers in `clanker list`, text and JSON.
    invariants:
      - Exec continues to strip every session marker, including the two new ones.
      - A version-5 marker set reports the unsupported-version error.
    acceptance_checks:
      - Session parsing tests cover a provider marker, an empty provider marker, and version 5 rejection.
      - Integration tests assert `clanker current`, `clanker env`, and `clanker list` output with and without providers.
      - mise run check passes.
    completion_evidence: null
  - id: T006
    title: Document the provider axis
    status: not_started
    depends_on:
      - T004
      - T005
    owner: null
    description: >
      Update README.md so the provider axis, its config shape, the separate-state
      constraint, exec behaviour, and the marker version change are documented
      where the other axes are.
    work_items:
      - Extend the Selection section to four axes and state that provider has no configured-default tier.
      - Add a Providers section with the TOML shape, the validation rules, and a Claude Code example that keeps the credential in the provider directory's settings.json apiKeyHelper.
      - Update the Session markers section for version 6 and the two new markers.
      - Update the clanker exec section for --provider.
      - Update RS-007 in REPO_INVARIANTS.md only if the measured coverage figure it states changes.
    invariants:
      - No example contains a credential value.
      - CHANGELOG.md is not edited by hand; release-plz owns it.
    acceptance_checks:
      - mise run check passes, including the spelling and docs hooks.
    completion_evidence: null
  - id: T007
    title: Run the full gate
    status: not_started
    depends_on:
      - T006
    owner: null
    description: >
      Run the repository's CI gate locally and record the results.
    work_items:
      - Run `mise run ci`.
      - Record test count, measured line coverage, and any hook output in completion evidence.
    invariants:
      - No test is skipped, ignored, or deleted to pass the gate.
      - Coverage stays at or above the 95% floor without new allow attributes.
    acceptance_checks:
      - mise run ci exits 0.
    completion_evidence: null
  - id: T008
    title: Regenerate agent-content eval goldens against the new binary
    status: not_started
    depends_on:
      - T007
    owner: null
    description: >
      agent-content's evals/goldens/plans/*.json snapshot dry-run JSON including
      CLANKER_SESSION_VERSION. Regenerate them with the new clanker and confirm the
      only differences are the version and the empty provider fields.
    work_items:
      - Build or install the T007 binary for the agent-content eval runner, with operator approval for any install.
      - Regenerate the goldens with agent-content's own task and inspect the diff.
    invariants:
      - Every golden diff is limited to CLANKER_SESSION_VERSION, the two provider markers, and the provider and provider_source fields.
    acceptance_checks:
      - agent-content's eval task passes against the regenerated goldens.
    completion_evidence: null
  - id: T009
    title: Configure Z.ai and DeepSeek providers and the fish aliases
    status: not_started
    depends_on:
      - T007
    owner: null
    description: >
      Operator setup outside the repository, each step behind operator approval:
      install the new clanker, define the providers in the overlay, create the
      provider directories with their credential helpers, and point the aliases at
      the provider axis.
    work_items:
      - Install the clanker build from T007 (approval required).
      - Add `[provider.zai.harness.claude]` and `[provider.deepseek.harness.claude]` to ~/.config/clanker/local.toml with literal env only, using Z.ai's and DeepSeek's documented base URLs and model names.
      - Create ~/.config/claude-zai/{personal,work} and ~/.config/claude-deepseek/{personal,work} for the contexts in use; write each settings.json with an apiKeyHelper that runs `sops -d --extract '["env"]["ZAI_API_KEY"]'` (or DEEPSEEK_API_KEY) against ~/.config/sops/secrets.sops.yaml, plus the hooks and plugins the operator wants carried over.
      - Change the fish functions in ~/.config/fish/conf.d/ai-tools.fish so `glm` runs `clanker claude --provider zai` and `deepseek` runs `clanker claude --provider deepseek`.
      - Run `clanker doctor`, a dry run per provider and context, and one live `glm -p` smoke test.
    invariants:
      - No credential value is written to any clanker config, fish file, or argv.
      - The native ~/.config/claude/{context} directories are not modified.
    acceptance_checks:
      - clanker doctor reports healthy.
      - Dry runs show CLAUDE_CONFIG_DIR under ~/.config/claude-zai/<context> for `glm` and under ~/.config/claude/<context> for `claude`.
      - A live `glm -p 'say hi'` returns a reply and its session appears only under ~/.config/claude-zai/<context>.
    completion_evidence: null
```
<!-- TASK_GRAPH:END -->

# Task Details

## T001 — Add the provider schema and its validation to the runtime config

Status: `not_started`

Depends on: none

### Description

Add ProviderName, ProviderConfig, and ProviderHarnessConfig to src/config.rs and a
top-level `provider` map to Config, defaulting to empty so existing configs parse
unchanged. Validate every provider in Config::issues.

### Invariants

- A config with no provider table produces the same issues as before this task.
- Every new ConfigIssue key is a dotted path under `provider.<name>.harness.<harness>`.

### Acceptance Checks

- Unit tests in src/config.rs cover each new issue, including the equal-to-native
  template, a variable-set mismatch, an unknown harness, a harness with no config_dir, a
  reserved env key, and a provider/model env overlap.
- A test asserts a valid provider table produces no issues.
- mise run check passes.

### Completion Evidence

Agents update this when the task is complete.

## T002 — Resolve the provider axis from flag, .clanker, and CLANKER_PROVIDER

Status: `not_started`

Depends on: T001

### Description

Add the PROVIDER axis and thread its three tiers to the launch and exec entry points,
with resolution and registry checks in the library.

### Invariants

- With no provider in any tier, resolution returns None and no error.
- The precedence order is flag, .clanker, environment, matching the other axes.

### Acceptance Checks

- Unit tests cover each tier winning, nearest-ancestor .clanker inheritance of provider,
  an empty CLANKER_PROVIDER falling through, an invalid name, an unknown provider, and a
  provider that does not cover the harness.
- CLI parser tests cover --provider, --provider=, a missing value, and the exec form.
- mise run check passes.

### Completion Evidence

Agents update this when the task is complete.

## T003 — Apply the provider to launch plans

Status: `not_started`

Depends on: T002

### Description

Use the provider's config_dir, env, and harness_args in command-mode and symlink-mode
launch plans, and report the provider in dry-run output.

### Invariants

- With no provider, the plan's args and environment are identical to the plan built
  before this task, apart from session markers added in T005.
- A selected provider's directory that does not exist fails the launch and exports no
  directory.
- The provider's directory is never the native directory for the same context.

### Acceptance Checks

- A test builds the same launch with and without a provider and asserts the config
  directory, env provenance, and argument order differ only as specified.
- A test asserts a missing provider directory returns MissingConfigDirectory naming the
  provider path.
- A test asserts symlink mode honours CLANKER_PROVIDER.
- An integration test in tests/ drives `clanker claude --provider <p> --dry-run --json`
  and asserts the provider fields and CLAUDE_CONFIG_DIR.
- mise run check passes.

### Completion Evidence

Agents update this when the task is complete.

## T004 — Apply the provider to clanker exec

Status: `not_started`

Depends on: T003

### Description

Make `clanker exec --provider <p>` export the provider's directories for the harnesses it
covers and the native directories for every other harness.

### Invariants

- The collision rule is unchanged and still fires for two native harnesses that disagree.
- With no provider, the exec plan is identical to the plan built before this task.

### Acceptance Checks

- Tests in tests/clanker_exec.rs assert the provider directory for a covered harness, the
  native directory for an uncovered one, and a skip warning when the provider directory
  is missing.
- mise run check passes.

### Completion Evidence

Agents update this when the task is complete.

## T005 — Export provider session markers and report them

Status: `not_started`

Depends on: T003

### Description

Add CLANKER_SESSION_PROVIDER and CLANKER_SESSION_PROVIDER_SOURCE, move the marker
contract to version 6, and surface the provider in `clanker current`, `clanker env`, and
`clanker list`.

### Invariants

- Exec continues to strip every session marker, including the two new ones.
- A version-5 marker set reports the unsupported-version error.

### Acceptance Checks

- Session parsing tests cover a provider marker, an empty provider marker, and version 5
  rejection.
- Integration tests assert `clanker current`, `clanker env`, and `clanker list` output
  with and without providers.
- mise run check passes.

### Completion Evidence

Agents update this when the task is complete.

## T006 — Document the provider axis

Status: `not_started`

Depends on: T004, T005

### Description

Update README.md so the provider axis, its config shape, the separate-state constraint,
exec behaviour, and the marker version change are documented where the other axes are.

### Invariants

- No example contains a credential value.
- CHANGELOG.md is not edited by hand; release-plz owns it.

### Acceptance Checks

- mise run check passes, including the spelling and docs hooks.

### Completion Evidence

Agents update this when the task is complete.

## T007 — Run the full gate

Status: `not_started`

Depends on: T006

### Description

Run the repository's CI gate locally and record the results.

### Invariants

- No test is skipped, ignored, or deleted to pass the gate.
- Coverage stays at or above the 95% floor without new allow attributes.

### Acceptance Checks

- mise run ci exits 0.

### Completion Evidence

Agents update this when the task is complete.

## T008 — Regenerate agent-content eval goldens against the new binary

Status: `not_started`

Depends on: T007

### Description

agent-content's evals/goldens/plans/*.json snapshot dry-run JSON including
CLANKER_SESSION_VERSION. Regenerate them with the new clanker and confirm the only
differences are the version and the empty provider fields.

### Invariants

- Every golden diff is limited to CLANKER_SESSION_VERSION, the two provider markers, and
  the provider and provider_source fields.

### Acceptance Checks

- agent-content's eval task passes against the regenerated goldens.

### Completion Evidence

Agents update this when the task is complete.

## T009 — Configure Z.ai and DeepSeek providers and the fish aliases

Status: `not_started`

Depends on: T007

### Description

Operator setup outside the repository, each step behind operator approval: install the
new clanker, define the providers in the overlay, create the provider directories with
their credential helpers, and point the aliases at the provider axis.

### Invariants

- No credential value is written to any clanker config, fish file, or argv.
- The native ~/.config/claude/{context} directories are not modified.

### Acceptance Checks

- clanker doctor reports healthy.
- Dry runs show CLAUDE_CONFIG_DIR under ~/.config/claude-zai/<context> for `glm` and
  under ~/.config/claude/<context> for `claude`.
- A live `glm -p 'say hi'` returns a reply and its session appears only under
  ~/.config/claude-zai/<context>.

### Completion Evidence

Agents update this when the task is complete.

# Execution Protocol

Agents executing this plan must:

- Read the full plan and verify a task's dependencies are `done` before starting it.
- Set a task to `in_progress` and update `updated_at` when starting.
- Update the plan when they find it wrong, discover missing context, change the approach,
  need operator guidance, or alter the task graph.
- When operator guidance is needed: add an Operator Guidance Log entry, set affected tasks
  to `blocked`, and not guess when guessing risks rework, data loss, security, or
  product-behavior changes.
- When completing a task: verify invariants, run or describe acceptance checks, record
  `completion_evidence`, set status `done`, update `updated_at`, and update ADR sections
  only when the task changed context, decision, alternatives, or consequences.
- Name, in the completion evidence, the files the task actually touched. A path and a
  line number beats a filename.
- Treat the Operator Guidance Log, Decision Log, and completion evidence as append-only;
  supersede prior entries with a new dated entry rather than deleting them.
