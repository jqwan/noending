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
    pub title_locked: bool,
}

impl Summary {
    pub fn single(title: impl Into<String>) -> Self {
        Self {
            items: vec![SummaryItem {
                id: uuid::Uuid::new_v4().to_string(),
                title: title.into(),
                description: String::new(),
                details: Vec::new(),
                title_locked: false,
            }],
        }
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn locked_titles(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter(|item| item.title_locked)
            .map(|item| item.title.as_str())
            .collect()
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
                    "id": item.id, "title": item.title, "title_locked": item.title_locked,
                })
            })
            .collect()
    }
    /// Extraction owns descriptions/details; explicit actions own title locks.
    pub fn preserve_title_locks(&mut self, previous: &Self) -> Result<()> {
        for old in previous.items.iter().filter(|item| item.title_locked) {
            let item = self
                .items
                .iter_mut()
                .find(|item| item.id == old.id && item.title == old.title)
                .ok_or_else(|| other("不能修改、删除或合并已锁定的事项标题"))?;
            item.title_locked = true;
        }
        Ok(())
    }
    pub fn validate_title_locks(&self, previous: &Self) -> Result<()> {
        let locked = |s: &Self| {
            s.items
                .iter()
                .filter(|i| i.title_locked)
                .map(|i| (i.id.clone(), i.title.clone()))
                .collect::<std::collections::BTreeSet<_>>()
        };
        if locked(self) != locked(previous) {
            return Err(other(
                "Agent 不能改变事项标题锁定状态，或修改、删除、合并锁定标题",
            ));
        }
        Ok(())
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
