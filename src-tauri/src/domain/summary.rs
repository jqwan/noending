//! Current summary content, grouped into freely reorganizable topics.
use crate::error::{other, Result};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub items: Vec<SummaryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryItem {
    /// Stable within this summary; omit for a newly created item.
    #[serde(default)]
    pub id: String,
    pub title: String,
    pub description: String,
    pub details: Vec<String>,
}

impl Summary {
    pub fn single(title: impl Into<String>) -> Self {
        Self {
            items: vec![SummaryItem {
                id: uuid::Uuid::new_v4().to_string(),
                title: title.into(),
                description: String::new(),
                details: Vec::new(),
            }],
        }
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// Allocate missing IDs only when writing, never while reading stored data.
    pub fn resolve_ids(mut self) -> Result<Self> {
        for item in &mut self.items {
            if item.id.is_empty() {
                item.id = uuid::Uuid::new_v4().to_string();
            }
        }
        if self.is_empty() {
            Ok(self)
        } else {
            self.validated()
        }
    }
    pub fn resolve_extracted_ids(self, previous: &Self) -> Result<Self> {
        for item in &self.items {
            if !item.id.is_empty() && !previous.items.iter().any(|old| old.id == item.id) {
                return Err(other("提取结果包含未知事项 ID；新事项应省略 ID"));
            }
        }
        self.resolve_ids()
    }
    pub fn identities(&self) -> Vec<serde_json::Value> {
        self.items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "id": item.id, "title": item.title,
                })
            })
            .collect()
    }
    pub fn validated(self) -> Result<Self> {
        let mut ids = std::collections::HashSet::new();
        for item in &self.items {
            if uuid::Uuid::parse_str(&item.id).is_err() || !ids.insert(&item.id) {
                return Err(other("事项 ID 必须是非空且不重复的 UUID"));
            }
        }
        self.validated_content()
    }
    /// Normalize surrounding whitespace, rejecting malformed or empty output.
    /// An empty summary is represented by absence, never a successful extraction.
    pub fn validated_content(mut self) -> Result<Self> {
        if self.is_empty() {
            return Err(other("摘要至少需要一个事项"));
        }
        for item in &mut self.items {
            item.title = item.title.trim().into();
            item.description = item.description.trim().into();
            if item.title.is_empty() || item.title.contains(['\n', '\r']) {
                return Err(other("事项标题必须是非空单行文字"));
            }
            for detail in &mut item.details {
                *detail = detail.trim().into();
                if detail.is_empty() {
                    return Err(other("事项细则不能为空"));
                }
            }
        }
        Ok(self)
    }
    /// Markdown is a read/export projection, not another persisted copy.
    pub fn markdown(&self) -> String {
        self.items
            .iter()
            .map(|item| {
                let mut parts = vec![format!("### {}", item.title)];
                if !item.description.is_empty() {
                    parts.push(item.description.clone());
                }
                if !item.details.is_empty() {
                    parts.push(
                        item.details
                            .iter()
                            .map(|detail| format!("- {}", detail.replace('\n', "\n  ")))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                parts.join("\n\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
