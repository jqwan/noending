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
* `assistant/` — Workspace Assistant (Interactive Mode, driven through the user's own Agent CLI)
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
* Add regression tests for schema or behavior changes.
* No database migrations: `storage/schema.rs` is the only schema source, and a
  breaking change bumps `DATABASE_FORMAT_VERSION` and rebuilds the local
  database. Never add an old-format compatibility branch.
* Consider both macOS and Windows for platform-sensitive code.

For domain-specific behavior, inspect the current code, tests, and relevant documentation before making changes.

Documents under `docs/` may include current design, implementation plans, or historical records. Do not assume every document is an active specification.

## Verify

Match verification to the change; do not run full tests for every turn or commit.

* Docs: `git diff --check` only.
* Copy/style/layout: inspect the affected UI; type-check TS/TSX changes.
* Frontend behavior: `pnpm exec tsc --noEmit` + relevant test files.
* Rust: in `src-tauri`, `cargo fmt --check`, `cargo check` + relevant tests.
* Schema/storage/ingestion/statistics: add regression tests and check affected
  consumers. API contract changes require checks on both sides.
* Build/import/asset/config changes: run the affected build. Dependency changes:
  reinstall and run the affected-side full tests and build.

Run full verification for major refactors, broad changes, stage acceptance,
release preparation, or explicit requests:

* Backend (`src-tauri`): `cargo fmt --check`, `cargo check --all-targets`,
  `cargo test --all-targets`.
* Frontend: `pnpm test`, `pnpm build`.

Run `pnpm install --frozen-lockfile` only when dependencies/lockfile change or
are missing. Reuse passing checks while covered code is unchanged; broaden
checks when impact is uncertain. Confirm filtered tests run and report actual
results and gaps.

For platform-sensitive changes, check macOS and Windows; use remote CI only
when authorized, and report any unverified platform.
