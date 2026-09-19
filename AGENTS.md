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

* **Workstream is the continuity unit.** Project is an app-managed organization layer, never a lifecycle owner.
* A Session may have **zero, one, or multiple Workstreams**.
* **Project is derived, never assigned.** A Project is the app-maintained physical *workspace family*: it exists because a `WorkspacePath` exists, it is renamed only by `name_customized`, and it disappears when its last path goes. It is still not a synonym for a repository — one Project may hold N paths, and some of them are not repos. Users never hand-create a Project or attach a Workstream to one.
* **A Workstream carries an ordered list of `WorkstreamPath`s**, not a single directory. Position 0 is primary. The list is a *launch and membership* anchor, not the Workstream's identity — a Workstream is still not a path, Sessions keep their own authoritative cwd, and a Workstream with zero paths stays fully valid.
* **Path identity is pure lexical.** `workspace_paths.id = "path-" + sha256(path_key(canonical_path))`, computed by `workspace::identity` with no filesystem access: no `fs::canonicalize`, no symlink resolution, no "does it exist" input. Existence and Git state are *observations* on the row, never part of the key.
* **`sessions.project_id` is a derived cache with exactly three writers**: the in-statement derivation inside `upsert_session`, the bulk refresh when a WorkspacePath changes Project, and the v12 migration. Any other `UPDATE sessions SET project_id` is a violation.
* **Session cwd drift does not grow a Workstream's path list.** A binding whose `workstream_path_id` stops matching is set to `NULL` (meaning "not brought in by any path"); it is never silently re-pointed or appended. Path lists grow only through a user action or an explicit binding.
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
* **Current Context is an authoritative projection, not raw item filtering**: `resolve_core_context` determines the active fact set; UI display and Agent delivery bundle must remain 100% isomorphic and derived from the same domain projection.
* **Context Provenance Fidelity**: Every context fact must be resolvable back to its originating Session, Event sequence, observed timestamp, Agent identity, and raw transcript evidence.
* **Never silently overwrite on conflict**: When Agent-extracted information disagrees with user-explicit or user-edited facts, NoEnding must materialize an explicit `ContextConflict`. Silent, automated, or heuristic overwrites are strictly prohibited.
* **Auditable Conflict Resolution**: Resolving a conflict must atomically record the decision action, actor, resolution rationale/note, and exact snapshot of involved revisions into `context_conflict_events`, forming an immutable audit trail.
* **Explicit Fact Evolution & Supersession**: Relations between superseded and replacement items form a directed acyclic evolution graph; replacing an item must explicitly track `supersedes` / `superseded_by` rather than breaking lineage.
* **Activity reflects true event stream**: Activity and recent changes must reflect the actual `ContextChange` event stream (revisions and conflict events), never a pseudo-timeline sorting items by `updated_at`.
* **User edits elevate authority**: Any user inline edit elevates the item to `user_edit` authority and produces a new Revision, preserving the full revision chain and immutable historical facts.
* **Review State Integrity**: ReviewState records what the human has observed; it is completely independent from ContextDelivery, Sync cursors, and Session bindings.
* **Mark Reviewed is observational acknowledgement only**: It MUST NOT mutate Context facts, resolve conflicts, advance ContextDelivery, or alter Session state.
* **Mark Reviewed MUST advance only to the exact ReviewFrontier that accompanied the ReviewWindow observed by the user**: Concurrent later changes remain unseen.
* **Context review ordering uses local Context mutation commit time**: It uses local `created_at`, never the originating transcript/event timestamp.
* **Home Attention Integrity**: Home may surface ReviewSummary state and navigate the user to a Workstream review experience. Home MUST NOT advance ReviewState. Viewing a Home attention item, clicking a Workstream card, or navigating into a Workstream MUST NOT mark anything reviewed.
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
* `src-tauri/src/workspace/` — path identity (pure lexical normalization), Home resolution, Project/Workstream/Session path rules. `workspace::identity` is the **only** place a path key may be computed.
* `src-tauri/src/storage/` — SQLite schema, transactions and persistence.
* `src/` — UI only; domain invariants belong in Rust.

Do not move OS-specific behavior into Agent adapters.

Observing the filesystem (existence, git dirs, symlinks) belongs to `platform/`; deciding *what a path is* belongs to `workspace/` and must stay filesystem-free.

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
