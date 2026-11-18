---
plan_format_version: 2
plan_id: PLAN-20261118-add-provider-axis
title: Add a provider selection axis so one harness can run against several endpoints with separate state
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
  task_graph_status: complete
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

### 2026-11-18 — open question for the operator: cargo deny fails before this change

Question or decision needed: `mise run check` fails in `cargo deny` on RUSTSEC-2026-0543
(chrono is unmaintained, no safe upgrade). The failure reproduces on untouched `main` at
`45a016f`, so it is not caused by this plan. Every task's acceptance check says
`mise run check passes`, which cannot hold until the advisory is ignored in `deny.toml`
or chrono is replaced. Both are dependency-policy changes outside this plan's scope.

Decision or guidance received: pending.

Impact on plan: tasks proceed and record every other hook passing; T007 cannot be
completed until this is decided.

### 2026-11-18 — T007 blocked on the cargo deny question

Question or decision needed: the same as the entry above. T001 to T006 are done.

Decision or guidance received: pending.

Impact on plan: T007 is `blocked`. Its results so far: `mise run check` passes every
hook except `cargo deny` (RUSTSEC-2026-0543, present on `main` at `45a016f`);
`mise run check:coverage` passes with 353 tests and 97.19% line coverage (floor 95%),
and the stated figure in README.md and REPO_INVARIANTS.md RS-007 moved from 97.03% to
97.19%. T008 and T009 depend on T007 and wait with it.

### 2026-11-18 — jfb (operator)

Question or decision needed: whether a provider implies a new base directory for
Claude Code, Codex, and OpenCode without a table per harness; the cargo deny advisory;
committing; T008 and T009.

Decision or guidance received: a provider derives its directories automatically for
every harness that has a config_dir (the "option 1" design). Replace chrono; jiff or
time are both acceptable. Commit the work so far on the feature branch (done:
`1eac504` and `ebac713`). T008 and T009 approvals are acknowledged for later.

Impact on plan: D007 supersedes D002. New tasks T010 (replace chrono) and T011
(derive provider directories); T007 now depends on both and returns to `not_started`.

### 2026-11-18 — follow-up for the operator: DeepSeek plugins and other harnesses

Question or decision needed: none blocking; recorded so it is not lost.

Decision or guidance received: n/a.

Impact on plan: the DeepSeek Claude Code directory has no plugins installed. Starting
one session there (for example `deepseek -p hi`) should register the marketplaces from
its settings.json, after which `clanker exec --provider deepseek -- claude plugin
install <plugin>` should work as it did for zai; this was not tried, under the
two-attempt rule. Codex and OpenCode provider directories (for example
`~/.config/codex-zai/personal`) do not exist yet; each needs its own endpoint
configuration before a provider launch of that harness will start.

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

### 2026-11-18 — D006: launches and exec export CLANKER_PROVIDER; symlink mode applies provider arguments

Recorded while executing T003 and T004.

A launch or exec that resolved a provider exports `CLANKER_PROVIDER`, as it already
exports `CLANKER_CONTEXT`, so a `clanker` launched from inside the session resolves the
same provider. `CLANKER_PROVIDER` joins the clanker-owned variables that domain, model,
and provider `env` may not set. Symlink mode prepends the provider's `harness_args` to
the passthrough as well as applying its `env` and directories: a provider applied
without its arguments would be half-applied, and symlink mode has no other argument
composition to conflict with.

### 2026-11-18 — D007: provider directories are derived, not declared (supersedes D002)

A provider covers every harness that declares a `config_dir`, in every context the
harness supports, and derives each directory from the harness's own template: when
`{context}` is a whole path component, the component before it gains `-<provider>`
(`~/.config/claude/{context}` becomes `~/.config/claude-zai/{context}`); when `{context}`
sits inside a component, that component gains `-<provider>` at its end; a fixed
single-context path gains `-<provider>` on its last component (`~/.pi/agent` becomes
`~/.pi/agent-zai`). The provider name grammar is already safe as part of a path
component. `[provider.<name>]` may be empty; `[provider.<name>.harness.<harness>]`
carries only optional `env` and `harness_args`, and naming a harness without a
`config_dir` there is a configuration error. Selecting a provider for a harness with no
`config_dir` fails with `ProviderHarnessUnsupported`, since there is no directory to
separate. Derivation always changes the path, so the "never the native directory" rule
holds by construction rather than by validation.

# Task Graph

<!-- TASK_GRAPH:BEGIN -->
```yaml
tasks:
  - id: T001
    title: Add the provider schema and its validation to the runtime config
    status: done
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
    completion_evidence: >
      Done 2026-11-18. Files touched: src/config.rs only. ProviderName at
      src/config.rs:103; Config.provider (serde default) at :146; ProviderConfig at
      :406 and ProviderHarnessConfig at :414 (config_dir required, env and
      harness_args default empty, deny_unknown_fields); Config::issues calls
      validate_providers at :547; validate_providers at :704 and
      validate_provider_harness at :719 report an empty harness table, an unknown
      harness, a harness with no config_dir, a variable-set mismatch, a template that
      renders to the native directory for any context the harness supports
      (render_directory at :802 trims trailing slashes), the placeholder rule
      (check_directory_template at :812, now shared with harness validation), invalid
      or reserved env names, and an env key also set by a model for the same harness.
      Tests: 13 new unit tests from :1257 to :1437; cargo nextest config:: runs 44
      tests, all pass. mise run check: every hook passes except cargo deny, which
      fails on RUSTSEC-2026-0543 (chrono unmaintained) in Cargo.lock; the same failure
      occurs on untouched main at 45a016f, so it predates this change (see Operator
      Guidance Log).
  - id: T002
    title: Resolve the provider axis from flag, .clanker, and CLANKER_PROVIDER
    status: done
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
    completion_evidence: >
      Done 2026-11-18. Files touched: src/axis.rs (PROVIDER axis at :52 with no
      configured default; test at :265), src/context.rs (DirectoryConfig.provider at
      :25, inherited per field at :48; test at :331 plus provider assertions in the
      three-level merge test), src/cli.rs (LaunchRequest.provider at :186,
      ExecRequest.provider at :207, Exec subcommand --provider at :63, parse_exec
      gains a provider argument at :244, --provider at :329 and --provider= at :352;
      tests at :578 and :602), src/main.rs:88 (reads CLANKER_PROVIDER), src/lib.rs:141
      (RuntimeContext.provider_environment, passed into LaunchInputs), src/launch.rs
      (LaunchInputs.provider_environment; resolve_provider at :1123 and bind_provider
      at :1154; LaunchError::UnknownProvider at :438 and ProviderHarnessUnsupported at
      :448, both naming the supplying tier). Precedence tests for flag, .clanker,
      environment, blank environment, and invalid names are in src/launch.rs under the
      provider-axis test block from :2205. cargo nextest: 348 tests pass. mise run
      check: every hook passes except the pre-existing cargo deny advisory.
  - id: T003
    title: Apply the provider to launch plans
    status: done
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
    completion_evidence: >
      Done 2026-11-18. Files touched: src/launch.rs and src/lib.rs, plus new
      tests/provider_axis.rs. build_harness_plan_for_context takes the bound provider
      and resolves the provider's ConfigDirectory in place of the harness's own, with
      the existing existence check; Provenance::Provider at src/launch.rs:86;
      apply_provider_environment at :1174 runs after the domain env and before the
      model env; compose_arguments inserts provider harness_args after the prompt and
      before model args at :1056; build_symlink_plan resolves provider from .clanker
      and CLANKER_PROVIDER and prepends provider args to the passthrough;
      LaunchPlan.provider and provider_source at :175, also in LaunchPlanOutput and
      the text dry-run at src/lib.rs:751 and :754. Launches export CLANKER_PROVIDER
      when a provider resolved (D006). Unit tests in src/launch.rs from :2205 cover
      directory, env provenance, argument order, the unchanged no-provider plan, a
      missing provider directory, an unknown provider, an uncovered harness, and
      symlink mode. tests/provider_axis.rs drives the binary with the shipped config
      plus a local.toml provider overlay: JSON and text dry runs, the native plan
      without a provider, fail-closed on a missing work directory, unknown and
      uncovered providers, and exec. 348 tests pass.
  - id: T004
    title: Apply the provider to clanker exec
    status: done
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
    completion_evidence: >
      Done 2026-11-18. Files touched: src/launch.rs. build_exec_plan resolves the
      provider through resolve_provider; for each harness the provider covers it uses
      the provider ConfigDirectory (src/launch.rs:733) before the unchanged collision
      check; the base environment moved into exec_base_environment at :796 (to keep
      build_exec_plan under the clippy line limit) and exports CLANKER_PROVIDER when a
      provider resolved; ExecPlan.provider at :212. Tests: three unit tests ending at
      src/launch.rs:2539 (provider directory for the covered harness and native for
      the rest, skip with the provider path when the provider directory is missing, no
      change without a provider, unknown provider refused) and
      exec_with_a_provider_exports_the_provider_directory in tests/provider_axis.rs.
      The plan named tests/clanker_exec.rs for these; they live in src/launch.rs and
      tests/provider_axis.rs instead, beside the other provider tests. All existing
      exec tests pass unchanged.
  - id: T005
    title: Export provider session markers and report them
    status: done
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
    completion_evidence: >
      Done 2026-11-18. Files touched: src/launch.rs (SESSION_VERSION is "6" at :36;
      CLANKER_SESSION_PROVIDER and CLANKER_SESSION_PROVIDER_SOURCE at :46 and :47,
      added to SESSION_ENVIRONMENTS at :52 so exec strips them, and exported by
      apply_session_environment with empty values when no provider resolved),
      src/session.rs (CurrentSession.provider and provider_source from :45; tests at
      :346 and :372, the second rejecting version 5), src/lib.rs (current text output
      at :732; list gains ProviderListing at :638 and a Providers section at :692 in
      text and a providers array in JSON), tests/symlink_mode.rs (the stub prints the
      provider markers and CLANKER_PROVIDER; marker fixtures and expectations moved to
      version 6 and assert the new markers, including that an inherited stale
      CLANKER_SESSION_PROVIDER is overwritten with empty), tests/provider_axis.rs (env
      with --provider, list in text and JSON, current with provider markers in JSON).
      `clanker env` needed no change: it parses launch flags through parse_launch,
      which accepts --provider since T002. Two unit tests in src/launch.rs that pin
      the marker count and version were updated from 18 to 20 removed variables and
      from "5" to "6". A stale target directory once let tests pass against an old
      binary; every result here is from a build after cargo clean -p tftio-clanker.
      cargo nextest: 353 tests pass. mise run check: every hook passes except the pre-
      existing cargo deny advisory.
  - id: T006
    title: Document the provider axis
    status: done
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
    completion_evidence: >
      Done 2026-11-18. Files touched: README.md only. Selection now names four axes
      and says provider has no configured default and that an unresolved provider
      means the native endpoint; the clanker exec section documents --provider,
      CLANKER_PROVIDER, and per-harness directory substitution; Session markers
      documents CLANKER_SESSION_PROVIDER and CLANKER_SESSION_PROVIDER_SOURCE and the
      move from version 5 to 6; a new Providers section at README.md:266 (marked
      agent-generated) gives the TOML shape, the validation rules, argument and
      environment order, the uncovered-harness error, CLANKER_PROVIDER export, symlink
      mode, and a Claude Code apiKeyHelper example that keeps the credential in the
      provider directory's settings.json (the JSON example parses with jq). No example
      contains a credential value. CHANGELOG.md is untouched. REPO_INVARIANTS.md
      RS-007 is left for T007, which measures coverage. mise run check: every hook
      passes, including typos and taplo, except the pre-existing cargo deny advisory.
  - id: T007
    title: Run the full gate
    status: done
    depends_on:
      - T006
      - T010
      - T011
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
    completion_evidence: >
      Done 2026-11-18. Ran after cargo clean -p tftio-clanker, at commit b4bcfa9 plus
      this coverage update. mise run ci exited 0: all 18 check hooks passed (fmt,
      taplo, shellcheck, typos, gitleaks history and staged, clippy, nextest, doc,
      machete, audit, deny, package, lockfiles, clanker doctor on the base config, and
      the file checks), and check:coverage ran 358 tests, all passing, with 97.12%
      line coverage against the 95% floor. No test was skipped, ignored, or deleted,
      and no allow attribute was added. The stated coverage in README.md:58 and
      REPO_INVARIANTS.md RS-007 changed from 97.19% to 97.12%. mise rewrites one
      checksum_source line in mise.lock on every run; that change is reverted and not
      committed.
  - id: T008
    title: Regenerate agent-content eval goldens against the new binary
    status: done
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
    completion_evidence: >
      Done 2026-11-18 in agent-content on branch chore/clanker-provider-goldens
      (worktree .worktrees/clanker-provider-goldens from origin/main at cd143bd),
      using CLANKER_BIN pointed at the release build of this branch rather than an
      installed binary. Discovery: on Linux, all 23 launch-plan goldens already failed
      with the installed clanker, because they were recorded on macOS and macOS adds
      __CF_USER_TEXT_ENCODING to every process, which appears in removed_environment.
      Commit bbddc7f makes evals/run-evals drop OS-injected names
      (PLATFORM_INJECTED_ENVIRONMENT, drop_platform_environment) and regenerates the
      goldens with the old clanker; that diff removes only the __CF_USER_TEXT_ENCODING
      entry from 23 plans. Commit 561f74e regenerates them with the new clanker;
      across the 23 plans the diff is exactly CLANKER_SESSION_VERSION 5 to 6, the two
      empty CLANKER_SESSION_PROVIDER and CLANKER_SESSION_PROVIDER_SOURCE entries, and
      provider and provider_source as null. evals/run-evals --check: 48 checked, 0
      failed against the new binary. agent-content's mise run check passed every hook
      before 561f74e; its composition-evals hook uses the clanker on PATH, so it
      passes after 561f74e only once the new clanker is installed (T009).
  - id: T009
    title: Configure Z.ai and DeepSeek providers and the fish aliases
    status: done
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
    completion_evidence: >
      Done 2026-11-18, with one follow-up. Installed this branch with cargo install
      --locked --path . (clanker 0.11.3 from the worktree; clanker doctor healthy).
      ~/.config/clanker/local.toml gains [provider.zai.harness.claude] and
      [provider.deepseek.harness.claude] with literal env only (endpoint, model
      mapping, timeout, nonessential traffic and telemetry off;
      ENABLE_TOOL_SEARCH=false for zai); clanker list reports both covering claude,
      codex, opencode, and pi. Created ~/.config/claude-zai/personal and
      ~/.config/claude-deepseek/personal (personal is the only enabled context on this
      machine) with a mode-600 settings.json built from the personal one: an
      apiKeyHelper that runs sops -d --extract on env.ZAI_API_KEY or
      env.DEEPSEEK_API_KEY, the same hooks with the kb SessionEnd path changed from
      /Users/jfb to ~/.config/claude/hooks, the same permissions and display settings,
      and the tftio marketplace path pointed at the personal directory's tftio-
      marketplace-src on this machine; the remote-control and push-notification keys
      were left out. ~/.config/fish/conf.d/ai-tools.fish: glm runs clanker claude
      --provider zai and deepseek runs clanker claude --provider deepseek. No
      credential value is in any clanker config, fish file, or argv, and
      ~/.config/claude/personal was not modified. Checks: clanker doctor healthy; dry
      runs show CLAUDE_CONFIG_DIR at ~/.config/claude/personal without a provider,
      ~/.config/claude-zai/personal with zai, and ~/.config/claude-deepseek/personal
      with deepseek; a live clanker claude --provider zai -p call returned "hello glm"
      from model glm-5.3[1m], and its session file exists only under ~/.config/claude-
      zai/personal/projects. Plugins: all four personal plugins installed and enabled
      in the zai directory through clanker exec --provider zai -- claude plugin
      install. In the deepseek directory both install and marketplace update failed
      because the copied extraKnownMarketplaces are not registered until a session
      starts there; this was stopped after two attempts and is left for the operator
      (see Operator Guidance Log). DeepSeek was not called live; its acceptance check
      in this task covers glm only. Codex and OpenCode provider directories were not
      created, so their provider launches fail closed until they are.
  - id: T010
    title: Replace chrono with jiff for the local date
    status: done
    depends_on: []
    owner: null
    description: >
      chrono is unmaintained (RUSTSEC-2026-0543) and fails cargo deny. clanker
      uses it only to format today's local date as YYYY-MM-DD in three places
      in src/promptlib. Replace it with jiff and drop the chrono dependency.
    work_items:
      - Replace chrono::Local::now() date formatting in src/promptlib/mod.rs and src/promptlib/render.rs with jiff's local date.
      - Remove chrono from Cargo.toml and update Cargo.lock through cargo.
      - Confirm cargo deny, cargo audit, and cargo machete pass.
    invariants:
      - The rendered date string keeps the YYYY-MM-DD format in the local time zone.
      - No advisory is ignored in deny.toml to make the gate pass.
    acceptance_checks:
      - The existing prompt rendering tests pass unchanged.
      - mise run check passes, including cargo deny.
    completion_evidence: >
      Done 2026-11-18. Files touched: Cargo.toml (chrono removed; jiff = "0.2.39"),
      Cargo.lock (jiff pinned at 0.2.39 with cargo update --precise; jiff 0.2.39 was
      published 2026-11-02, while every later release, including 1.0.0 and 1.0.1, was
      under two weeks old on 2026-11-18), src/promptlib/mod.rs (local_date at :259,
      jiff::Zoned::now().date() rendered as YYYY-MM-DD; used by format_system_prefix;
      new test local_date_is_an_iso_calendar_date), src/promptlib/render.rs (both call
      sites, including the test fixture, use crate::promptlib::local_date). A live
      clanker prompt run printed "Today is 2026-11-18", matching date +%F. No advisory
      was ignored. cargo nextest: 354 tests pass. mise run check: every hook passes,
      including cargo deny, cargo audit, and cargo machete.
  - id: T011
    title: Derive provider directories from the harness templates
    status: done
    depends_on: []
    owner: null
    description: >
      Implement D007. A provider covers every harness with a config_dir and
      derives each directory from the harness template, so one empty
      [provider.<name>] table gives Claude Code, Codex, and OpenCode separate
      directories. Per-harness tables keep only env and harness_args.
    work_items:
      - Remove config_dir from ProviderHarnessConfig and make ProviderConfig.harness optional.
      - Add the derivation function for single and multiple directory shapes, covering a whole-component {context}, an embedded {context}, and a fixed single-context path.
      - Replace the declared-directory validation with a check that each named harness exists and declares a config_dir; drop the at-least-one-harness rule.
      - Make launch, symlink mode, and exec use derived directories for every harness with a config_dir, and refuse a harness without one.
      - Report covered harnesses in clanker list as those with a config_dir.
      - Update the tests and the README Providers section to the derived shape.
    invariants:
      - A derived directory never equals the harness's own directory for any context.
      - With no provider, plans are identical to those before this task.
      - A derived directory that does not exist fails a launch and is skipped with a warning under exec.
    acceptance_checks:
      - Unit tests cover the three derivation cases and both directory shapes.
      - An integration test launches claude and codex with an empty provider table and asserts each derived directory.
      - mise run check passes apart from any failure T010 has not yet removed.
    completion_evidence: >
      Done 2026-11-18. Files touched: src/config.rs (ProviderConfig at :434 now
      Default with an optional harness map; ProviderHarnessConfig at :443 holds only
      env and harness_args; ConfigDirectory::for_provider at :388 derives both shapes
      entry by entry; provider_template at :851 implements the D007 rule; validation
      keeps env name, reservation, and model-overlap checks, reports a named harness
      that is unknown or has no config_dir, and validate_provider_directories at :769
      with native_directories at :742 reports a derived directory equal to any
      harness's own directory in any supported context; the declared-directory,
      variable-set, placeholder, and at-least-one-harness checks are gone),
      src/launch.rs (BoundProvider at :1120 carries the derived directory and an
      optional entry; bind_provider at :1159 refuses a harness without a config_dir;
      exec derives for every harness with a config_dir at :740), src/lib.rs:669 (list
      reports every harness with a config_dir as covered), README.md:266 (Providers
      section rewritten for derivation, with a table of examples), and the provider
      tests in src/config.rs, src/launch.rs, and tests/provider_axis.rs. New tests
      cover the three derivation cases plus trailing slashes and a component after
      {context}, both directory shapes, cross-harness and cross-context coincidences,
      an empty provider launching Claude Code, and an empty provider table deriving
      claude-bare and codex-bare directories through the binary. A stale build once
      reported 356 passing tests against the old binary; the results here follow cargo
      clean -p tftio-clanker. cargo nextest: 358 tests pass. mise run check passes
      every hook. Line coverage 97.12%.
```
<!-- TASK_GRAPH:END -->

# Task Details

## T001 — Add the provider schema and its validation to the runtime config

Status: `done`

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

Done 2026-11-18. Files touched: src/config.rs only. ProviderName at src/config.rs:103;
Config.provider (serde default) at :146; ProviderConfig at :406 and
ProviderHarnessConfig at :414 (config_dir required, env and harness_args default empty,
deny_unknown_fields); Config::issues calls validate_providers at :547;
validate_providers at :704 and validate_provider_harness at :719 report an empty harness
table, an unknown harness, a harness with no config_dir, a variable-set mismatch, a
template that renders to the native directory for any context the harness supports
(render_directory at :802 trims trailing slashes), the placeholder rule
(check_directory_template at :812, now shared with harness validation), invalid or
reserved env names, and an env key also set by a model for the same harness. Tests: 13
new unit tests from :1257 to :1437; cargo nextest config:: runs 44 tests, all pass. mise
run check: every hook passes except cargo deny, which fails on RUSTSEC-2026-0543 (chrono
unmaintained) in Cargo.lock; the same failure occurs on untouched main at 45a016f, so it
predates this change (see Operator Guidance Log).

## T002 — Resolve the provider axis from flag, .clanker, and CLANKER_PROVIDER

Status: `done`

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

Done 2026-11-18. Files touched: src/axis.rs (PROVIDER axis at :52 with no configured
default; test at :265), src/context.rs (DirectoryConfig.provider at :25, inherited per
field at :48; test at :331 plus provider assertions in the three-level merge test),
src/cli.rs (LaunchRequest.provider at :186, ExecRequest.provider at :207, Exec
subcommand --provider at :63, parse_exec gains a provider argument at :244, --provider
at :329 and --provider= at :352; tests at :578 and :602), src/main.rs:88 (reads
CLANKER_PROVIDER), src/lib.rs:141 (RuntimeContext.provider_environment, passed into
LaunchInputs), src/launch.rs (LaunchInputs.provider_environment; resolve_provider at
:1123 and bind_provider at :1154; LaunchError::UnknownProvider at :438 and
ProviderHarnessUnsupported at :448, both naming the supplying tier). Precedence tests
for flag, .clanker, environment, blank environment, and invalid names are in
src/launch.rs under the provider-axis test block from :2205. cargo nextest: 348 tests
pass. mise run check: every hook passes except the pre-existing cargo deny advisory.

## T003 — Apply the provider to launch plans

Status: `done`

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

Done 2026-11-18. Files touched: src/launch.rs and src/lib.rs, plus new
tests/provider_axis.rs. build_harness_plan_for_context takes the bound provider and
resolves the provider's ConfigDirectory in place of the harness's own, with the existing
existence check; Provenance::Provider at src/launch.rs:86; apply_provider_environment at
:1174 runs after the domain env and before the model env; compose_arguments inserts
provider harness_args after the prompt and before model args at :1056;
build_symlink_plan resolves provider from .clanker and CLANKER_PROVIDER and prepends
provider args to the passthrough; LaunchPlan.provider and provider_source at :175, also
in LaunchPlanOutput and the text dry-run at src/lib.rs:751 and :754. Launches export
CLANKER_PROVIDER when a provider resolved (D006). Unit tests in src/launch.rs from :2205
cover directory, env provenance, argument order, the unchanged no-provider plan, a
missing provider directory, an unknown provider, an uncovered harness, and symlink mode.
tests/provider_axis.rs drives the binary with the shipped config plus a local.toml
provider overlay: JSON and text dry runs, the native plan without a provider, fail-
closed on a missing work directory, unknown and uncovered providers, and exec. 348 tests
pass.

## T004 — Apply the provider to clanker exec

Status: `done`

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

Done 2026-11-18. Files touched: src/launch.rs. build_exec_plan resolves the provider
through resolve_provider; for each harness the provider covers it uses the provider
ConfigDirectory (src/launch.rs:733) before the unchanged collision check; the base
environment moved into exec_base_environment at :796 (to keep build_exec_plan under the
clippy line limit) and exports CLANKER_PROVIDER when a provider resolved;
ExecPlan.provider at :212. Tests: three unit tests ending at src/launch.rs:2539
(provider directory for the covered harness and native for the rest, skip with the
provider path when the provider directory is missing, no change without a provider,
unknown provider refused) and exec_with_a_provider_exports_the_provider_directory in
tests/provider_axis.rs. The plan named tests/clanker_exec.rs for these; they live in
src/launch.rs and tests/provider_axis.rs instead, beside the other provider tests. All
existing exec tests pass unchanged.

## T005 — Export provider session markers and report them

Status: `done`

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

Done 2026-11-18. Files touched: src/launch.rs (SESSION_VERSION is "6" at :36;
CLANKER_SESSION_PROVIDER and CLANKER_SESSION_PROVIDER_SOURCE at :46 and :47, added to
SESSION_ENVIRONMENTS at :52 so exec strips them, and exported by
apply_session_environment with empty values when no provider resolved), src/session.rs
(CurrentSession.provider and provider_source from :45; tests at :346 and :372, the
second rejecting version 5), src/lib.rs (current text output at :732; list gains
ProviderListing at :638 and a Providers section at :692 in text and a providers array in
JSON), tests/symlink_mode.rs (the stub prints the provider markers and CLANKER_PROVIDER;
marker fixtures and expectations moved to version 6 and assert the new markers,
including that an inherited stale CLANKER_SESSION_PROVIDER is overwritten with empty),
tests/provider_axis.rs (env with --provider, list in text and JSON, current with
provider markers in JSON). `clanker env` needed no change: it parses launch flags
through parse_launch, which accepts --provider since T002. Two unit tests in
src/launch.rs that pin the marker count and version were updated from 18 to 20 removed
variables and from "5" to "6". A stale target directory once let tests pass against an
old binary; every result here is from a build after cargo clean -p tftio-clanker. cargo
nextest: 353 tests pass. mise run check: every hook passes except the pre-existing cargo
deny advisory.

## T006 — Document the provider axis

Status: `done`

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

Done 2026-11-18. Files touched: README.md only. Selection now names four axes and says
provider has no configured default and that an unresolved provider means the native
endpoint; the clanker exec section documents --provider, CLANKER_PROVIDER, and per-
harness directory substitution; Session markers documents CLANKER_SESSION_PROVIDER and
CLANKER_SESSION_PROVIDER_SOURCE and the move from version 5 to 6; a new Providers
section at README.md:266 (marked agent-generated) gives the TOML shape, the validation
rules, argument and environment order, the uncovered-harness error, CLANKER_PROVIDER
export, symlink mode, and a Claude Code apiKeyHelper example that keeps the credential
in the provider directory's settings.json (the JSON example parses with jq). No example
contains a credential value. CHANGELOG.md is untouched. REPO_INVARIANTS.md RS-007 is
left for T007, which measures coverage. mise run check: every hook passes, including
typos and taplo, except the pre-existing cargo deny advisory.

## T007 — Run the full gate

Status: `done`

Depends on: T006, T010, T011

### Description

Run the repository's CI gate locally and record the results.

### Invariants

- No test is skipped, ignored, or deleted to pass the gate.
- Coverage stays at or above the 95% floor without new allow attributes.

### Acceptance Checks

- mise run ci exits 0.

### Completion Evidence

Done 2026-11-18. Ran after cargo clean -p tftio-clanker, at commit b4bcfa9 plus this
coverage update. mise run ci exited 0: all 18 check hooks passed (fmt, taplo,
shellcheck, typos, gitleaks history and staged, clippy, nextest, doc, machete, audit,
deny, package, lockfiles, clanker doctor on the base config, and the file checks), and
check:coverage ran 358 tests, all passing, with 97.12% line coverage against the 95%
floor. No test was skipped, ignored, or deleted, and no allow attribute was added. The
stated coverage in README.md:58 and REPO_INVARIANTS.md RS-007 changed from 97.19% to
97.12%. mise rewrites one checksum_source line in mise.lock on every run; that change is
reverted and not committed.

## T008 — Regenerate agent-content eval goldens against the new binary

Status: `done`

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

Done 2026-11-18 in agent-content on branch chore/clanker-provider-goldens (worktree
.worktrees/clanker-provider-goldens from origin/main at cd143bd), using CLANKER_BIN
pointed at the release build of this branch rather than an installed binary. Discovery:
on Linux, all 23 launch-plan goldens already failed with the installed clanker, because
they were recorded on macOS and macOS adds __CF_USER_TEXT_ENCODING to every process,
which appears in removed_environment. Commit bbddc7f makes evals/run-evals drop OS-
injected names (PLATFORM_INJECTED_ENVIRONMENT, drop_platform_environment) and
regenerates the goldens with the old clanker; that diff removes only the
__CF_USER_TEXT_ENCODING entry from 23 plans. Commit 561f74e regenerates them with the
new clanker; across the 23 plans the diff is exactly CLANKER_SESSION_VERSION 5 to 6, the
two empty CLANKER_SESSION_PROVIDER and CLANKER_SESSION_PROVIDER_SOURCE entries, and
provider and provider_source as null. evals/run-evals --check: 48 checked, 0 failed
against the new binary. agent-content's mise run check passed every hook before 561f74e;
its composition-evals hook uses the clanker on PATH, so it passes after 561f74e only
once the new clanker is installed (T009).

## T009 — Configure Z.ai and DeepSeek providers and the fish aliases

Status: `done`

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

Done 2026-11-18, with one follow-up. Installed this branch with cargo install --locked
--path . (clanker 0.11.3 from the worktree; clanker doctor healthy).
~/.config/clanker/local.toml gains [provider.zai.harness.claude] and
[provider.deepseek.harness.claude] with literal env only (endpoint, model mapping,
timeout, nonessential traffic and telemetry off; ENABLE_TOOL_SEARCH=false for zai);
clanker list reports both covering claude, codex, opencode, and pi. Created
~/.config/claude-zai/personal and ~/.config/claude-deepseek/personal (personal is the
only enabled context on this machine) with a mode-600 settings.json built from the
personal one: an apiKeyHelper that runs sops -d --extract on env.ZAI_API_KEY or
env.DEEPSEEK_API_KEY, the same hooks with the kb SessionEnd path changed from /Users/jfb
to ~/.config/claude/hooks, the same permissions and display settings, and the tftio
marketplace path pointed at the personal directory's tftio-marketplace-src on this
machine; the remote-control and push-notification keys were left out.
~/.config/fish/conf.d/ai-tools.fish: glm runs clanker claude --provider zai and deepseek
runs clanker claude --provider deepseek. No credential value is in any clanker config,
fish file, or argv, and ~/.config/claude/personal was not modified. Checks: clanker
doctor healthy; dry runs show CLAUDE_CONFIG_DIR at ~/.config/claude/personal without a
provider, ~/.config/claude-zai/personal with zai, and ~/.config/claude-deepseek/personal
with deepseek; a live clanker claude --provider zai -p call returned "hello glm" from
model glm-5.3[1m], and its session file exists only under ~/.config/claude-
zai/personal/projects. Plugins: all four personal plugins installed and enabled in the
zai directory through clanker exec --provider zai -- claude plugin install. In the
deepseek directory both install and marketplace update failed because the copied
extraKnownMarketplaces are not registered until a session starts there; this was stopped
after two attempts and is left for the operator (see Operator Guidance Log). DeepSeek
was not called live; its acceptance check in this task covers glm only. Codex and
OpenCode provider directories were not created, so their provider launches fail closed
until they are.

## T010 — Replace chrono with jiff for the local date

Status: `done`

Depends on: none

### Description

chrono is unmaintained (RUSTSEC-2026-0543) and fails cargo deny. clanker uses it only to
format today's local date as YYYY-MM-DD in three places in src/promptlib. Replace it
with jiff and drop the chrono dependency.

### Invariants

- The rendered date string keeps the YYYY-MM-DD format in the local time zone.
- No advisory is ignored in deny.toml to make the gate pass.

### Acceptance Checks

- The existing prompt rendering tests pass unchanged.
- mise run check passes, including cargo deny.

### Completion Evidence

Done 2026-11-18. Files touched: Cargo.toml (chrono removed; jiff = "0.2.39"), Cargo.lock
(jiff pinned at 0.2.39 with cargo update --precise; jiff 0.2.39 was published
2026-11-02, while every later release, including 1.0.0 and 1.0.1, was under two weeks
old on 2026-11-18), src/promptlib/mod.rs (local_date at :259, jiff::Zoned::now().date()
rendered as YYYY-MM-DD; used by format_system_prefix; new test
local_date_is_an_iso_calendar_date), src/promptlib/render.rs (both call sites, including
the test fixture, use crate::promptlib::local_date). A live clanker prompt run printed
"Today is 2026-11-18", matching date +%F. No advisory was ignored. cargo nextest: 354
tests pass. mise run check: every hook passes, including cargo deny, cargo audit, and
cargo machete.

## T011 — Derive provider directories from the harness templates

Status: `done`

Depends on: none

### Description

Implement D007. A provider covers every harness with a config_dir and derives each
directory from the harness template, so one empty `[provider.<name>]` table gives Claude
Code, Codex, and OpenCode separate directories. Per-harness tables keep only env and
harness_args.

### Invariants

- A derived directory never equals the harness's own directory for any context.
- With no provider, plans are identical to those before this task.
- A derived directory that does not exist fails a launch and is skipped with a warning
  under exec.

### Acceptance Checks

- Unit tests cover the three derivation cases and both directory shapes.
- An integration test launches claude and codex with an empty provider table and
  asserts each derived directory.
- mise run check passes apart from any failure T010 has not yet removed.

### Completion Evidence

Done 2026-11-18. Files touched: src/config.rs (ProviderConfig at :434 now Default with
an optional harness map; ProviderHarnessConfig at :443 holds only env and harness_args;
ConfigDirectory::for_provider at :388 derives both shapes entry by entry;
provider_template at :851 implements the D007 rule; validation keeps env name,
reservation, and model-overlap checks, reports a named harness that is unknown or has no
config_dir, and validate_provider_directories at :769 with native_directories at :742
reports a derived directory equal to any harness's own directory in any supported
context; the declared-directory, variable-set, placeholder, and at-least-one-harness
checks are gone), src/launch.rs (BoundProvider at :1120 carries the derived directory
and an optional entry; bind_provider at :1159 refuses a harness without a config_dir;
exec derives for every harness with a config_dir at :740), src/lib.rs:669 (list reports
every harness with a config_dir as covered), README.md:266 (Providers section rewritten
for derivation, with a table of examples), and the provider tests in src/config.rs,
src/launch.rs, and tests/provider_axis.rs. New tests cover the three derivation cases
plus trailing slashes and a component after {context}, both directory shapes, cross-
harness and cross-context coincidences, an empty provider launching Claude Code, and an
empty provider table deriving claude-bare and codex-bare directories through the binary.
A stale build once reported 356 passing tests against the old binary; the results here
follow cargo clean -p tftio-clanker. cargo nextest: 358 tests pass. mise run check
passes every hook. Line coverage 97.12%.

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
