//! Context quality corpus: fixture schema, loading and structural validation.
//!
//! A fixture serves two evaluation layers from one document (see the crate
//! module docs on [`super`] for the layer model):
//!
//! - Layer A — domain golden: `recorded_model_output` + exact `gold`
//! - Layer B — extraction eval: `input.events` + structural `extractor_gold`
//!
//! This module owns the document format only: schema, parsing and
//! well-formedness validation. No database, no extractor.

use std::collections::HashSet;

use serde::Deserialize;

use crate::error::{other, Result};

/// Implicit check id for `extractor_gold.conflicts_count`.
pub const CONFLICTS_CHECK_ID: &str = "conflicts:count";

#[derive(Debug, Deserialize)]
pub struct FixtureWorkstream {
    pub id: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct FixtureInitialItem {
    pub id: String,
    pub workstream_id: String,
    pub kind: String,
    pub title: String,
    pub content: String,
    pub authority: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct FixtureSession {
    pub agent: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct FixtureEvent {
    pub sequence: i64,
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct FixtureInput {
    pub workstreams: Vec<FixtureWorkstream>,
    #[serde(default)]
    pub initial_context: Vec<FixtureInitialItem>,
    pub session: FixtureSession,
    pub events: Vec<FixtureEvent>,
}

/// Layer A gold: exact expectations against the recorded model output.
#[derive(Debug, Deserialize)]
pub struct ExpectedMutation {
    pub op: String,
    #[serde(default)]
    pub workstream_id: Option<String>,
    #[serde(default)]
    pub item_kind: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub authority: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExpectedCoreItem {
    pub kind: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct FixtureGold {
    pub mutations: Vec<ExpectedMutation>,
    #[serde(default)]
    pub effective_context: std::collections::HashMap<String, Vec<ExpectedCoreItem>>,
    #[serde(default)]
    pub conflicts_count: usize,
}

/// Layer B gold: structural expectations for a real extractor. Fields left
/// out are unconstrained; wording is never pinned exactly. Every expectation
/// carries a stable `id` — check ids are `<family>:<id>` and form the
/// criterion-level baseline.
#[derive(Debug, Deserialize, Default)]
pub struct StructuralMutationExpectation {
    pub id: String,
    /// Any-of: the mutation's op must be one of these.
    #[serde(default)]
    pub op: Vec<String>,
    #[serde(default)]
    pub workstream_id: Option<String>,
    /// Any-of.
    #[serde(default)]
    pub item_kind: Vec<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    /// All-of substrings of the title.
    #[serde(default)]
    pub title_contains: Vec<String>,
    /// All-of substrings of the content.
    #[serde(default)]
    pub content_contains: Vec<String>,
    #[serde(default)]
    pub authority: Option<String>,
    /// The mutation must cite every one of these event sequences.
    #[serde(default)]
    pub source_events: Vec<i64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ContextItemExpectation {
    pub id: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title_contains: Vec<String>,
}

/// Status expectation for a specific item. Non-core kinds (e.g. `todo`)
/// never appear in the core projection, so their correct end state must be
/// asserted directly on the item.
#[derive(Debug, Deserialize, Clone)]
pub struct ItemStatusExpectation {
    pub id: String,
    pub item_id: String,
    pub status: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct ExtractorGold {
    #[serde(default)]
    pub required_mutations: Vec<StructuralMutationExpectation>,
    #[serde(default)]
    pub forbidden_mutations: Vec<StructuralMutationExpectation>,
    #[serde(default)]
    pub required_context: std::collections::HashMap<String, Vec<ContextItemExpectation>>,
    #[serde(default)]
    pub forbidden_context: std::collections::HashMap<String, Vec<ContextItemExpectation>>,
    #[serde(default)]
    pub required_item_status: Vec<ItemStatusExpectation>,
    #[serde(default)]
    pub conflicts_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct ContextQualityFixture {
    pub name: String,
    pub description: String,
    pub dimension: String,
    pub input: FixtureInput,
    pub recorded_model_output: String,
    pub gold: FixtureGold,
    pub extractor_gold: ExtractorGold,
    /// The exact set of check ids (`required:<id>`, `forbidden:<id>`,
    /// `required_context:<id>`, `forbidden_context:<id>`,
    /// `item_status:<id>`, `conflicts:count`)
    /// the extractor under test is known to fail. Must equal the actual
    /// failed set: improvements and regressions both turn CI red until
    /// re-baselined in the same commit that changed the extractor.
    #[serde(default)]
    pub heuristic_expected_failed_checks: Vec<String>,
}

/// Parse and validate one corpus fixture document.
pub fn load_fixture(json: &str) -> Result<ContextQualityFixture> {
    let fixture: ContextQualityFixture =
        serde_json::from_str(json).map_err(|e| other(format!("fixture JSON parse failed: {e}")))?;
    validate(&fixture)?;
    Ok(fixture)
}

/// Structural well-formedness of the check set. Check ids must be non-empty
/// and unique per family — a duplicate or empty id would silently weaken
/// set-equality baseline enforcement. A mutation expectation without any
/// constraint would match every mutation — a fixture bug, not an eval
/// result.
fn validate(fixture: &ContextQualityFixture) -> Result<()> {
    let gold = &fixture.extractor_gold;
    let mut seen: HashSet<String> = HashSet::new();
    let mut require = |family: &str, id: &str| -> Result<()> {
        if id.is_empty() {
            return Err(other(format!(
                "fixture '{}': {family} entry has an empty id",
                fixture.name
            )));
        }
        if !seen.insert(format!("{family}:{id}")) {
            return Err(other(format!(
                "fixture '{}': duplicate check id {family}:{id}",
                fixture.name
            )));
        }
        Ok(())
    };
    for exp in &gold.required_mutations {
        require("required", &exp.id)?;
    }
    for exp in &gold.forbidden_mutations {
        require("forbidden", &exp.id)?;
    }
    for entries in gold.required_context.values() {
        for exp in entries {
            require("required_context", &exp.id)?;
        }
    }
    for entries in gold.forbidden_context.values() {
        for exp in entries {
            require("forbidden_context", &exp.id)?;
        }
    }
    for exp in &gold.required_item_status {
        require("item_status", &exp.id)?;
    }
    for exp in gold
        .required_mutations
        .iter()
        .chain(&gold.forbidden_mutations)
    {
        if !constrained(exp) {
            return Err(other(format!(
                "fixture '{}': extractor_gold check '{}' has no constraint and would match every mutation",
                fixture.name, exp.id
            )));
        }
    }
    Ok(())
}

fn constrained(exp: &StructuralMutationExpectation) -> bool {
    !exp.op.is_empty()
        || exp.workstream_id.is_some()
        || !exp.item_kind.is_empty()
        || exp.item_id.is_some()
        || !exp.title_contains.is_empty()
        || !exp.content_contains.is_empty()
        || exp.authority.is_some()
        || !exp.source_events.is_empty()
}
