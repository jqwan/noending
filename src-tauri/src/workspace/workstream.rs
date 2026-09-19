//! WorkstreamPaths, lifecycle and the recycle bin.
//!
//! Owned by Agent C (方案 §18).
//!
//! ## The ordered list is the whole model (§1.5)
//!
//! ```text
//! workstream_paths(workstream_id, workspace_path_id, position, source)
//!   UNIQUE(workstream_id, workspace_path_id)
//!   UNIQUE(workstream_id, position)
//! ```
//!
//! `position = 0` IS the primary path. There is no `is_primary` column and no
//! primary/secondary pair — two authorities for one fact is precisely how
//! `default_cwd` and `workstreams.project_id` came to disagree with reality.
//! Therefore: `paths.is_empty() || paths[0]` exists by construction, and
//! "secondary without primary" is not representable.
//!
//! * remove at index k → recompact positions, so the next entry becomes 0
//!   without asking the user (§1.6).
//! * `reorder_workstream_paths(ordered_workspace_path_ids)` takes the FULL list;
//!   "make this the primary path" is a reorder to index 0 (§21).
//! * `create_workstream(title, description, initial_path?)` — no `project_id`,
//!   no `default_cwd`. `default_cwd` survives only as a v12 migration input.
//! * Workstream→Project is a projection through the paths. New code must not
//!   write `workstreams.project_id` (`upsert_workstream_conn` no longer accepts
//!   it — §42.2-E6).
//!
//! ## Lifecycle (§1.13 / §5.7)
//!
//! ```text
//! lifecycle   active | completed     classification only, no behavior, free to switch
//! visibility  normal | archived       archived IS the recycle bin
//! ```
//!
//! `archive` flips visibility and nothing else; `restore` flips back, so
//! lifecycle / paths / bindings / configuration are all still there and the
//! previous state returns naturally. Permanent deletion is only reachable from
//! `archived`.
//!
//! ## Permanent deletion table order (§42.3-M6)
//!
//! `context_conflict_events` → `context_conflicts` → `context_item_revisions` →
//! `context_items` → `context_deliveries` → `session_workstream_bindings` →
//! `workstream_paths` → `session_binding_removals` (no FK, so it never cascades
//! and would leak) → `workstream_review_state` (cascades) → `workstreams`.
//!
//! Never touched: `sessions`, `session_events`, `session_cursors`,
//! `launch_intents`, `workspace_paths`, and the Agents' raw transcript files.
//! A Session survives the Workstream that referenced it.
//!
//! ## Reindex
//!
//! Any path-list mutation changes the primary-path Project projection, so it
//! must re-index the Workstream's search row (§42.3-M18).
