# AGENTS.md

## Project

NoEnding is a local multi-Agent workspace built with:

* Tauri 2
* Rust + SQLite
* React + TypeScript
* Agent adapters — the roster is `Agent` in `domain/models.rs` plus
  `all_adapters()` in `adapters/mod.rs`, and it changes; not every Agent has a
  CLI (some adapters are history-ingestion only, so `detect()` never succeeds
  by design, not by bug)

Core concepts include `Workstream`, `Session`, `Project`, and `WorkspacePath`.

## Repository

Backend code lives under `src-tauri/src/`.

Key areas:

* `adapters/` — Agent integrations
* `domain/` — platform-independent domain model (Session / SessionMember / SessionMessage / …)
* `platform/` — OS-specific behavior
* `workspace/` — workspace and project logic
* `ingestion/` — session discovery and ingestion
* `sync/` — synchronization and extraction
* `context/` — explicit Context update service and read projections
* `agent_runtime/` — Agent runtime configuration (Agent owns defaults, NoEnding owns overrides)
* `assistant/` — built-in Assistant project and managed capabilities; native Agent sessions reuse the launcher and ingestion
* `search/` — FTS5 search
* `launcher/` — New / Resume launch flows
* `lifecycle/` — session lifecycle
* `storage/` — SQLite and persistence
* `commands/` — Tauri API boundary

Frontend code lives under `src/`.

Key areas:

* `app/` — app shell, router and routes
* `features/` — product features
* `components/` — shared UI
* `layout/` — application shell and navigation
* `hooks/` — shared React hooks
* `api.ts` — frontend/backend API
* `types.ts` — frontend API types

## Development

* Keep changes small and scoped.
* Follow the existing module boundaries and reuse existing APIs where practical.
* Add or extend regression tests for schema or behavior changes using the rules below.
* `storage/schema.rs` is the current schema source. Database versioning and
  compatibility changes must preserve existing data and include regression
  tests. Do not use database deletion/rebuilding as an upgrade path or retain
  disposable one-off migration tools as maintained product code.
* The database compatibility baseline is version 1. Keep `application_id` stable
  and increment the schema version for persisted format changes. Add ordered,
  transactional forward migrations for published versions; desktop, CLI and
  MCP must use the same storage opening path. Preserve existing domain data,
  verify upgrade/fresh-schema equivalence and rollback, and refuse unsupported
  newer databases without modifying them. Keep migration steps immutable.
* Keep current schema documentation focused on the supported format. Temporary
  conversion tools, development-format fixtures, obsolete object inventories
  and historical schema commentary do not belong in maintained source or docs.
* Consider both macOS and Windows for platform-sensitive code.

For domain-specific behavior, inspect the current code, tests, and relevant documentation before making changes.

Use `docs/README.md` to find maintained usage guides (`guides/`).
Temporary plans, investigations and work records
belong in ignored `.tmp/docs/`, not in committed documentation. Promote only
reviewed, reusable conclusions into `docs/`; local research and old plans are
not active specifications.

## Test design

* Test observable behavior, public contracts, data integrity or external protocols;
  changed code and coverage numbers alone do not justify a test.
* Exercise the actual scenario and assert independent expected results; avoid
  self-comparisons, vacuous checks and assertions that mirror the implementation.
  Assert through roles, accessible names and visible state (persistence: a
  remount); no write counts, private columns or storage keys — call counts only
  when the IPC call itself is the contract.
* A negative assertion needs a positive counterpart and must reference something
  that exists in source; the test name states the invariant its assertions prove.
* Extend existing coverage before adding tests. Merge equivalent cases, but keep
  distinct behavioral, platform and protocol boundaries — the two sides of one
  invariant (inside/outside, enabled/disabled) are boundaries, not duplicates.
* Avoid pinning incidental copy, icons, pixels or private structure; test UI
  details when they affect meaning, navigation or action availability.
* Isolate mocks, caches and persistent state. Declare external prerequisites;
  missing prerequisites must never silently pass without executing checks.

## Verify

Match verification to the change; do not run full tests for every turn or commit.

* Docs: `git diff --check` only.
* Copy/style/layout: inspect the affected UI; type-check TS/TSX changes.
* Frontend behavior: `pnpm exec tsc --noEmit` + `pnpm test <path>` (never
  `pnpm test -- --run <path>` — that silently runs the whole suite).
* Rust: in `src-tauri`, `cargo fmt --check`, `cargo check`, plus
  `cargo test <name>` or `cargo test --test <suite>`.
* Schema/storage/ingestion: add regression tests and check affected
  consumers. API contract changes require checks on both sides.
* Build/import/asset/config changes: run the affected build. Dependency changes:
  reinstall and run the affected-side full tests and build.

Read the output, not the command line: a targeted run must show it filtered
(vitest `Test Files 1`, a nonzero Rust test count — a filter matching nothing
still exits 0); a full suite is green only by its exit status, which pipes
through head/tail/grep discard, so redirect to a file before reading; and
`cargo test` stops at the first failing target, so re-running that one target
after a fix is not a green suite.

Run full verification for major refactors, broad changes, stage acceptance,
release preparation, or explicit requests — once, at the end, on the final
code state:

* Backend (`src-tauri`): `cargo fmt --check`, `cargo check --all-targets`,
  `cargo test --all-targets`.
* Frontend: `pnpm test`, `pnpm build`.

Run `pnpm install --frozen-lockfile` only when dependencies/lockfile change or
are missing. Reuse passing checks while covered code is unchanged; broaden
checks when impact is uncertain. A post-green touch-up re-runs only the
affected slice plus tsc. Report actual results and gaps.

For platform-sensitive changes, check macOS and Windows; use remote CI only
when authorized, and report any unverified platform.
