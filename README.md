# clanker

Launch AI harnesses with runtime-configured context, domains, models, and prompts

## Installation

```sh
cargo install --locked --git https://github.com/tftio/clanker
```

This installs the `clanker` binary. Then install a base configuration (see
Configuration below).

## Getting started

Toolchain, task execution, and hook tools are managed by mise.
The Rust toolchain is declared in `mise.toml`; rustup is an implementation
detail and `rust-toolchain.toml` is intentionally absent.
Entering the directory does not prepare, install, or regenerate anything: setup
is explicit, so no lockfile ever changes because someone walked into the
repository.

```sh
mise trust --quiet
mise install
mise run setup:idea   # optional; regenerates the gitignored .idea/
```

Tools come from `mise activate <shell>` in an interactive shell, and from mise
shims for non-interactive processes such as editors and coding agents.

Dependencies move on one deliberate command, and never on their own:

```sh
mise run update       # mise tools, cargo crates, prek hooks
mise run check:locks  # read-only; fails if Cargo.lock is stale
```

## Tasks

```sh
mise run check  # check-only hooks, as CI runs them
mise run lint   # manual autofix hooks
mise run test   # test suite
mise run ci     # full CI gate
```

The generated Rust gate includes formatting, TOML formatting, shell linting,
spelling, clippy, nextest, docs, unused-dependency detection, advisory audit,
license/source policy, packaging, and at least 95% line coverage. The coverage
floor is 95% rather than the generated 100% because the binary entry in
`src/main.rs` (`process::exit` on a failed process environment), the
`execute_plan` process replacement that never returns on success, a few
structurally-unreachable command guards, and filesystem I/O error-map closures
that require OS-specific or permission errors cannot be exercised by a genuine,
network-free, in-process test. The axis resolution, prompt, config, context, and
session and exec-plan logic is otherwise covered by inline unit tests, and every
offline command is driven through the binary, keeping actual coverage at 97.03%. See
REPO_INVARIANTS.md RS-007.

## Selection

Context, domain, and model resolve through one rule: an explicit flag, then the
nearest `.clanker` at or above the working directory, then `CLANKER_CONTEXT` /
`CLANKER_DOMAIN` / `CLANKER_MODEL`, then a configured default. Context has no
configured default — a launch that cannot resolve one refuses to start rather
than guessing, because context selects the harness config directory and
therefore its credentials. Only `HOME`, `PATH`, and `CLANKER_`-prefixed
variables are examined.

## Project resolution

At every launch, clanker resolves which project the working directory belongs
to, independent of context/domain/model selection. A project is a slug (not a
repository or a remote); the derivation, the registry format, and the remote
normalization rules live in `tftio_lib::project` and are not restated here.
In order: a `project = "<slug>"` key in the `.clanker` at the working
directory's project root (the repository top level inside a repository, with
no ancestor walk -- a peer of `.git`, not found the way context/domain/model
are); then a `paths` entry in the project registry that is the working
directory or an ancestor of it; then the working directory's git origin
remote looked up among the registry's `remotes`; then a slug derived from an
unregistered remote's last path segment; otherwise no project. Environment and
the working directory are read once, in `main.rs`, exactly like every other
axis; a registry-load or directory-discovery failure never blocks a launch --
it is reported on stderr and the launch proceeds as though that tier resolved
nothing (`clanker doctor` reports the same failures as check failures).

The registry lives at `${XDG_CONFIG_HOME:-~/.config}/tftio/projects.toml`,
with an uncommitted `projects.local.toml` overlay beside it for machine-local
paths. It is a neutral file clanker reads but does not own or write.

`clanker project resolve [--dir <path>]` exposes the same derivation to a
script or hook, defaulting to the current directory. Its text output is one
tab-separated line, `<slug>\t<source>\t<remote>`, with empty fields when
nothing resolves; it exits `0` either way, so a shell hook can call it
unconditionally. `--json` reports `{"project", "source", "remote"}` with
`null` in place of an empty field. `clanker project list` reports every slug
the registry knows, its remotes, and its paths, in the same tab-separated or
`--json` shapes. `clanker doctor` validates the installed registry (slug
grammar, remotes already normalized, no remote or path claimed by two
projects) as failures, reports a registered path absent on this machine as a
(passing) warning, and fails when the working directory's `.clanker`
declaration and the registry's remote mapping name different projects.

## Configuration

`config/config.toml` in this repository is the base runtime configuration --
harnesses, models, domains, and the prompter bundle path -- and this
repository's own CI gates it with `mise run check:config` (`clanker doctor
--config config/config.toml`). Install (or symlink) it at
`${CLANKER_CONFIG:-~/.config/clanker/config.toml}`. It grants no credentials
to any domain; add the variables a domain may see in a local overlay.

A machine-specific overlay goes in a sibling `local.toml` next to wherever
`config.toml` is installed -- for example `~/.config/clanker/local.toml`.
`load_config` deep-merges it automatically when present (scalars and arrays
are replaced, tables merge recursively) and is silently absent otherwise; no
code change is needed to add or remove an overlay.

## Pi (single context)

<!-- AGENT_GENERATED: true; AGENT_MODEL: gpt-6-astra; AGENT_DATE: 2026-09-21 -->

Pi keeps all of its state in one directory, so the shipped config restricts it
to the `personal` context: `clanker pi --context personal` uses
`PI_CODING_AGENT_DIR=~/.pi/agent`, preserving Pi's existing provider
definitions, auth, settings, and sessions. Pi launches in any other context
fail, including `pi-launch` symlink mode. The shipped config uses the
`deepseek` prompt family, which falls back to neutral fragments when no variant
exists; it appends the composed prompt by file and does not force a model.
Use Pi's own model selector or pass native flags after `--`.

Harness `contexts` is an optional nonempty, unique subset of `defaults.contexts`.
Omitting it preserves all-context behavior. A fixed `config_dir.path` without
`{context}` is permitted only with an explicit single-context restriction.
`clanker exec` omits restricted harnesses in other contexts and removes their
inherited config-directory variables.

Multiple providers can be configured in Pi's own `auth.json` and `models.json`.
For environment-based keys, explicitly list the required variable names in the
selected domain's `credentials` via clanker's local overlay; arrays replace their
base values, so retain existing grants when adding keys. Do not put key values in
clanker config or pass them as command-line arguments. No provider key is granted
automatically. Named clanker model aliases can override the prompt family when
switching to Claude or GPT; native Pi model switching does not recompose a running
session's clanker prompt.

Clanker does not install Pi itself or migrate its credentials.

## Running context-sensitive tools

`clanker exec` runs an arbitrary command with the config-directory environment
for every configured harness. This is intended for context-sensitive tools that
manage several agents in one invocation. For example, this installs a global
skill into each agent's directories for the resolved context instead of its
shared defaults:

```sh
clanker exec -- skills add -g <package>
```

The command argv begins after the `--` boundary and is passed directly to
`execvp`; clanker does not insert a shell. Pipelines therefore belong to the
calling shell:

```sh
clanker exec -- command-a | clanker exec -- command-b
```

Context follows the normal precedence rules, with `clanker exec --context work
-- <argv>` as the explicit form. Clanker prepends `defaults.shim_path` to
`PATH`, exports `CONTEXT` and `CLANKER_CONTEXT`, and unions the declared
`config_dir` variables from every harness. A harness without a `config_dir`
contributes nothing. If any directory declared by one harness is absent,
clanker warns on stderr, omits all variables from that harness, and continues
with the others. Two harnesses resolving the same variable to different paths
are a configuration error.

Exec does not compose a prompt, add a sandbox, or export
`CLANKER_SESSION_*` markers; it also removes inherited session markers when
called from a harness. Consequently, `clanker exec -- clanker current` reports
`active=false`; `active` continues to mean that a harness session is running.

## Session markers

Every command-mode launch exports a set of `CLANKER_SESSION_*` markers
describing what the launch resolved, and `clanker current` reads them back
without recomputing anything. One of them, `CLANKER_SESSION_ID`, identifies the
launch itself: a UUIDv7 minted once per `clanker` process, exported to the
harness, and written nowhere. It exists so a tool running inside a session can
attribute its records to that session; two sessions started a minute apart in
the same context and directory are otherwise indistinguishable. Clanker neither
persists it nor correlates it with a harness's own internal session id, which
is minted after clanker has already handed the process over.

Every launch also exports `CLANKER_SESSION_PROJECT`,
`CLANKER_SESSION_PROJECT_SOURCE`, and `CLANKER_SESSION_REMOTE` from the same
resolution described under Project resolution above; each is the empty
string when nothing resolved, consistent with the other optional axes.
`clanker current` reports them back the same way it reports every other
marker, never recomputing them.

For `mnene`, every harness launch also exports `MNENE_AGENT` from the selected
harness name, `MNENE_CONTEXT` from the resolved clanker context, and
`MNENE_SESSION` from `CLANKER_SESSION_ID`. Clanker deliberately does not export
`MNENE_SCOPE`: mnene resolves the repository-derived scope for each invocation,
so changing repositories during one harness session cannot freeze the wrong
scope into the process environment. A launch outside clanker still relies on
mnene's own fallbacks and fails loudly when neither an agent nor a repository
scope can be resolved.

`clanker exec` mints no identifier and strips an inherited one, as it does with
every other session marker.

The marker set is versioned by `CLANKER_SESSION_VERSION`, and `clanker current`
accepts only the version it was built for. Adding the identifier moved that
contract from 3 to 4; adding the project markers moved it from 4 to 5, so a
harness session launched by an older clanker reports `unsupported clanker
session marker version` until it is relaunched. That is the version check
working; relaunching the session clears it.

## Harness config directories

A harness may declare the context-derived directories it needs, in either of two
shapes. The single-variable shape covers a harness that keeps everything behind
one variable:

```toml
[harness.claude.config_dir]
environment = "CLAUDE_CONFIG_DIR"
path = "~/.config/claude/{context}"
```

The multi-variable shape covers a harness that splits its state across several
roots. Each key is an exported variable and each value is its path template:

```toml
[harness.polytoken.config_dir.directories]
POLYTOKEN_CONFIG_PATH = "~/.config/polytoken/{context}"
POLYTOKEN_DATA_PATH = "~/.local/share/polytoken/{context}"
POLYTOKEN_CACHE_PATH = "~/.cache/polytoken/{context}"
```

The two shapes are exclusive; declaring both, or half of the single shape, is a
schema error. Every declared path template must contain `{context}`, every
declared variable is reserved against domain and model override, and every
declared directory must already exist. A harness launch missing any one of them
fails and exports none of them: the directory selects the harness credentials,
so creating an empty one would turn a misconfiguration into a session with no
credentials and no explanation. Under `clanker exec`, the same all-or-nothing
rule applies per harness; a missing directory warns and omits that harness's
whole set without blocking variables from other harnesses.

## Playwright MCP profiles

Every harness launch and `clanker exec` exports a Playwright MCP browser and
persistent user-data directory derived from the resolved context:

```text
PLAYWRIGHT_MCP_BROWSER=chromium
PLAYWRIGHT_MCP_USER_DATA_DIR=~/.local/share/clanker/playwright/<context>/chromium
```

The browser is Playwright's managed Chromium, installed once and shared across
contexts. Browser authentication is not shared: `personal` and `work` receive
different persistent profiles beneath clanker's data directory. The path is
fixed rather than configurable, so clanker cannot be directed to an ordinary
Chrome, Chromium, Edge, or Safari profile.

The Playwright MCP declaration itself belongs in each harness's context-specific
configuration, such as the Codex configuration selected by `CODEX_HOME` or the
Claude configuration selected by `CLAUDE_CONFIG_DIR`. Use the existing
`clanker exec --context <context> -- <client-specific installer>` boundary when
an MCP client provides an installation command. Pin `@playwright/mcp` in those
declarations; clanker neither selects the package version nor installs npm
packages or browser binaries.

Do not add `--user-data-dir`, `--executable-path`, `--extension`, or
`--cdp-endpoint` to the managed MCP declaration. The first two would bypass
clanker's fixed profile/browser choices, while extension and CDP modes can attach
to an existing human browser. A persistent profile also supports only one active
browser process, so concurrent agents in the same context must serialize their
Playwright use.

## Polytoken

Polytoken splits its state across three roots, so it uses the multi-variable
shape shown above. Two details are worth stating because they are easy to get
wrong:

- `POLYTOKEN_CONFIG_PATH` names the configuration directory itself. No
  `polytoken` component is appended, so the template ends in `{context}`, not in
  `polytoken/{context}` below an XDG root.
- Credentials straddle two roots. Static provider keys live in `config.yaml`
  under the config root; device-flow auth and MCP OAuth live under `auth/` in the
  data root. Isolating the config root alone leaves those credentials shared
  between contexts.

Polytoken's `--config-dir` flag is not this mechanism and must not be used for
it: for runtime commands it sets the *project* config layer, not the user one.

Polytoken has no per-launch system-prompt flag, so prompt injection goes through
the `env-file` contract and a per-context hook. Clanker renders the composed
prompt and exports its path:

```toml
[harness.polytoken]
bin = "polytoken"
family = "gpt"
default_args = ["new"]

[harness.polytoken.injection]
kind = "env-file"
environment = "CLANKER_PROMPT_FILE"
```

Each context's `hooks.json`, at `<config dir>/hooks.json`, reads that file on
`session_start` and returns it as additional context:

```json
[
  {
    "name": "clanker-prompt",
    "event": "session_start",
    "handler": {
      "bash": "jq -Rs '{outcome:\"allow\",additional_context:.}' \"$CLANKER_PROMPT_FILE\""
    }
  }
]
```

The prompt reaches the model as a system-reminder rather than as the system
prompt proper, because polytoken builds its system prompt entirely from the
active facet.

One divergence from the harnesses clanker replaces itself with: `polytoken new`
spawns a daemon and attaches a TUI, so the daemon inherits the launch
environment at spawn time. A session later resumed with `polytoken attach` or
`polytoken continue` carries the session markers of its daemon's original spawn,
not those of the resuming invocation.

### Per-context setup

A polytoken context needs its three directories to exist before a launch
succeeds, plus a configuration file and the hook above:

```sh
context=personal
mkdir -p ~/.config/polytoken/$context \
         ~/.local/share/polytoken/$context \
         ~/.cache/polytoken/$context
POLYTOKEN_CONFIG_PATH=~/.config/polytoken/$context polytoken config edit --user
```
