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

Before finishing a change, run:

```bash
cd src-tauri
cargo fmt --check
cargo check --all-targets
cargo test --all-targets

cd ..
pnpm install --frozen-lockfile
pnpm test
pnpm build
```

For platform-sensitive changes, verify macOS and Windows CI at the final HEAD.
