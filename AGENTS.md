# AGENTS.md

## Project

NoEnding is a local multi-Agent workspace built around **long-lived Workstream Context**.

Core stack:

* Tauri 2
* Rust + SQLite
* React + TypeScript
* Codex / Claude Code / Pi adapters

The core product invariant is:

> Sessions end. Agents change. Workstream Context persists and remains trustworthy.

## Core Invariants

When changing the system, preserve these rules:

* **Workstream is the continuity unit.** Project is only an optional organization layer.
* A Session may have **zero, one, or multiple Workstreams**.
* Never equate Project with repository, cwd, workspace, or filesystem path.
* A Workstream may carry an optional **default working directory** (`default_cwd`) as a launch convenience: it suggests where New Sessions start. It is never identity — a Workstream is not a path, Sessions keep their own authoritative cwd, and a Workstream without one stays fully valid.
* Raw Agent session files are **read-only**. Never modify or delete them.
* Ingested Session Events are **append-only history**. Never overwrite historical events.
* Event identity is app-owned and stable. Source file position is metadata, not identity.
* The fallback event-identity chain continues from the **SourceCursor's tail hash** (the current source chain), never from "the last event in the store" — after a compact the store is ahead of the source.
* A schema change to identity semantics requires a **data migration**, not just a column addition. That migration is **lazy and source-driven**: the append-only Event Store is never treated as the source's chain — identities are re-derived from the real source on the next full re-scan, with event ids preserved.
* `SourceReference` must remain resolvable after truncate, compact, rewrite, retry, or re-ingest.
* `processed_cursor` must only move forward after a successful atomic Sync commit.
* A Sync run must be **all-or-nothing**: Context mutations, audit records, conflicts, SyncRun and cursor advancement commit together.
* Never silently overwrite `user_explicit` or `user_edit` Context with Agent-derived information. Preserve disagreement as `ContextConflict`.
* Every Context change must leave an auditable Revision.
* Resume Context is based on **delivered revision snapshots**, not timestamps or full-history replay.
* The token budget filters **before** rendering: `bundle.sections` describes exactly what the markdown delivers, so delivery snapshots can never over-report.
* User-selected Workstream bindings are stronger than automatic classification; automatic bindings stay **revisable** — re-classification replaces them, it never freezes them.
* A user-removed Workstream binding is a **permanent negative decision** (removal tombstone): sync rounds and elapsed time never revive it, and auto-classification must never re-propose the pair — not even as an extraction candidate. Only a user strong write (explicit launch / user assignment) lifts it. If rejection ever needs to become revisable, that must be an explicit user action ("allow classification again"), never a TTL.
* A Sync run is committed against the binding decision it prepared with: the decision snapshot (strong bindings + removal tombstones) is re-checked at commit, and any change during extraction discards the run as stale without advancing the cursor.
* **Context Delivery controls outbound context injection only.** Turning delivery Off MUST NOT disable ingestion, sync, extraction, Workstream bindings, or context evolution. A ContextDelivery snapshot MUST be advanced only when the corresponding context was actually delivered to the Agent.
* **Launch Preparation Integrity**: Prepare may refresh observed state (ingestion / sync / read DB), but MUST NOT commit launch-specific user decisions (bindings, LaunchIntent) or delivery state (`ContextDelivery`).
* **Preview-Launch Identity**: What you preview is what the Agent receives. Launch Prepared uses the exact prepared bundle and aborts if the state fingerprint changed since preview.
* macOS and Windows are first-class platforms.

When uncertain, prefer **preserving history, provenance, and user intent** over convenience.

## Architecture Boundaries

Keep responsibilities separated:

* `src-tauri/src/adapters/` — Agent-specific discovery, parsing and command description.
* `src-tauri/src/platform/` — OS-specific paths, processes, terminals, quoting and executable resolution.
* `src-tauri/src/ingestion/` — discovery → normalized events → read cursor.
* `src-tauri/src/sync/` — extraction, classification, policy and deterministic merge.
* `src-tauri/src/context/` — Core Context projection and Session Context bundles.
* `src-tauri/src/launcher/` — New / Resume flows, LaunchIntent and delivery tracking.
* `src-tauri/src/storage/` — SQLite schema, transactions and persistence.
* `src/` — UI only; domain invariants belong in Rust.

Do not move OS-specific behavior into Agent adapters.

Do not let LLM output directly mutate storage. LLMs propose changes; deterministic Rust code validates and applies them.

## Development Rules

* Do not hold the DB mutex while running external Agent/LLM processes.
* Revalidate state before committing work prepared while the DB lock was released.
* Do not generate shell syntax such as `$(...)` inside adapters. Pass structured commands and literal arguments.
* Do not infer metadata from lossy path conventions when the transcript contains authoritative metadata.
* Automatic classification should persist its evidence/source/confidence when it affects domain state.
* Schema changes require an explicit migration or development reset strategy.
* Preserve the product UX:

  * Primary creation actions are **New Workstream** and **New Session**.
  * Resume is an action on an existing Session.
  * Selecting a Workstream for New Session is optional.

## Testing

Before finishing a change, run:

```bash
cd src-tauri
cargo fmt --check
cargo check --all-targets
cargo test --all-targets

cd ..
pnpm install --frozen-lockfile
pnpm build
```

For changes involving ingestion, sync, authority, launcher, resume, storage, or platform code, add regression tests for the invariant being changed.

Important edge cases include:

* append / truncate / rewrite / file replacement
* retry and concurrent Sync
* duplicate-looking but distinct events
* SourceReference preservation
* user authority conflicts
* crash recovery for LaunchIntent
* revision-based Resume delta
* paths and arguments containing spaces, quotes, Unicode and shell metacharacters
* macOS and Windows behavior

## Change Discipline

Prefer small changes that preserve existing domain semantics.

Before introducing a shortcut, ask:

1. Can this lose historical information?
2. Can retry apply the same semantic change twice?
3. Can this break SourceReference or Audit history?
4. Can Agent inference override explicit user intent?
5. Does this behave correctly on both macOS and Windows?

If any answer is uncertain, fix the invariant first.
