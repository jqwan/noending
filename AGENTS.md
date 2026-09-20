# AGENTS.md

## Project

NoEnding is a local multi-Agent workspace built with:

* Tauri 2
* Rust + SQLite
* React + TypeScript
* Codex / Claude Code / Pi adapters

Core concepts include `Workstream`, `Session`, `Project`, and `WorkspacePath`.

## Repository

Backend code lives under `src-tauri/src/`.

Key areas:

* `adapters/` — Agent integrations
* `platform/` — OS-specific behavior
* `workspace/` — workspace and project logic
* `ingestion/` — session discovery and ingestion
* `sync/` — synchronization and extraction
* `launcher/` — New / Resume launch flows
* `lifecycle/` — session lifecycle
* `storage/` — SQLite and persistence
* `commands/` — Tauri API boundary

Frontend code lives under `src/`.

Key areas:

* `features/` — product features
* `components/` — shared UI
* `layout/` — application shell and navigation
* `api.ts` — frontend/backend API
* `types.ts` — frontend API types

## Development

* Keep changes small and scoped.
* Follow the existing module boundaries and reuse existing APIs where practical.
* Add migrations and regression tests for schema or behavior changes.
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
