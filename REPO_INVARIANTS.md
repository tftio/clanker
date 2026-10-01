# Repository invariants

These invariants are binding for humans and coding agents working in this repository.
Bypasses must be explicit: use `INVARIANT-BYPASS(<ID>): <reason>` in the smallest
possible change and explain why the invariant cannot hold.

## Engineering invariants

| ID | Invariant | Enforcement |
|---|---|---|
| ENG-001 | Keep changes single-purpose and use Conventional Commits for committed work. | Review + `cog verify` |
| ENG-002 | Never commit secrets, credentials, or tokens; inject configuration from the environment rather than hardcoding it. | Review + secret scanning |
| ENG-003 | Every behavior change ships with tests that exercise real systems, assert outcomes rather than implementation, and cover error paths. Do not skip, xfail, or delete tests to make checks pass. | Tests + review |
| ENG-004 | Fail loudly with actionable context at the source; do not add silent `except` blocks or swallowed `Result` values. | Review |
| ENG-005 | Distinguish recoverable domain errors (return typed error values) from invariant violations (assert and crash with diagnostics). | Review |
| ENG-006 | Parse external data into typed containers at the system boundary; validate once on the way in and trust the types inside. | Review |
| ENG-007 | Keep retained state immutable; store snapshots in fields, module- and class-level bindings, closures, and caches, never aliased mutable objects. | Review |
| ENG-008 | Keep business logic in pure, deterministic functions; confine I/O, logging, and state mutation to a thin imperative shell. | Review |
| ENG-009 | Model data so illegal states are unrepresentable; dispatch over closed sum types exhaustively with no silent catch-all. | Review |
| ENG-010 | Wrap third-party dependencies behind domain-specific interfaces rather than threading their APIs through the codebase. | Review |
| ENG-011 | Fit the repository's established conventions; do not rewrite working code solely to change its library, tooling, or style. | Review |
| ENG-012 | Keep formatting, linting, typing, and tests clean before handing off work. | `mise run check` |
| ENG-013 | `mise.toml` plus `mise.lock` are the repository-owned tool declarations; hooks and CI must invoke `mise run` tasks rather than reimplementing checks. | Review + CI |

## Rust invariants

| ID | Invariant | Enforcement |
|---|---|---|
| RS-001 | Use Rust 2024 with the exact Rust toolchain declared in `mise.toml`; do not add `rust-toolchain.toml` as a second source of truth. | Review + `mise run check` |
| RS-002 | Commit `Cargo.lock` for every Rust repository, including libraries, so local and CI dependency resolution use the same artifact. | Review + CI |
| RS-003 | Deny unsafe code, missing docs, clippy warnings, unwrap/expect/panic/todo/unimplemented/dbg, wildcard imports, enum glob imports, and unchecked indexing. | `mise run check:clippy` |
| RS-004 | Keep command entry points thin; put behavior in the library crate and cover it with tests. | Tests + review |
| RS-005 | Model recoverable failures as typed error enums with `thiserror`; use `anyhow` only at binary or integration boundaries. | Review |
| RS-006 | Validate dependency advisories, licenses, duplicate dependency shape, unused dependencies, documentation, packaging, and spelling before handoff. | `mise run check` |
| RS-007 | Maintain at least 95% line coverage. This repository-local floor (the generated default is 100%) accommodates residual paths that no genuine, network-free, in-process test can reach: in `src/main.rs`, the `runtime_context` failure arm (`eprintln!` plus `process::exit(1)`), which depends on a failing process environment; in `src/lib.rs`, the `execute_plan` process replacement (`execvp` never returns on success), the `Command::Meta` label and `UnroutedMetadataCommand` arms that `metadata_command` routes away before `run_command`, and the `NonUtf8EnvironmentOutput` guard in `emit_environment`; plus filesystem I/O error-map closures in `prompt.rs`, `config.rs`, and `context.rs` (`create`/`read`/`write` failures) that require OS-specific or permission errors not reproducible in a portable, unprivileged test. `main.rs`'s `project_inputs` registry-load and discovery-error warning branches (`eprintln!` on a malformed `projects.toml` or an unnormalizable origin remote) are exercised by `tests/symlink_mode.rs` integration tests that assert on the spawned binary's stderr, but `cargo-llvm-cov`'s subprocess instrumentation does not reliably attribute their hits back to the parent report, so they read as uncovered despite being genuinely tested; do not chase that gap with more subprocess tests. Measured line coverage is 97.03%, held there by inline tests of axis resolution and provenance, prompt composition/injection and cache GC, config validation including reserved-environment collisions and both context-directory shapes, `.clanker` discovery and per-field inheritance, session marker parsing, project resolution and its registry/doctor checks, and exec-plan construction, and by driving the binary for every offline command via integration tests; do not use `#[allow]`, skips, or deleted tests to reach the floor. Lowering the floor further requires an explicit invariant bypass. | `mise run check:coverage` |

## CLI invariants

| ID | Invariant | Enforcement |
|---|---|---|
| CLI-001 | Keep command parsing and process I/O in `src/main.rs`; delegate domain behavior into `src/lib.rs`. | Tests + review |
| CLI-002 | Console output is user-facing API; cover non-trivial output behavior with integration tests. | Tests |
