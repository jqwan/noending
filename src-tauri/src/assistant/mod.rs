//! Persistent assistant profiles.
//! Each profile owns a registered directory; native sessions reuse the shared
//! launcher, ingestion, project and Skill loading services.
mod files;
mod models;
mod profiles;
pub use models::*;
pub use profiles::*;

use crate::workspace::home::NoEndingHome;
use std::path::PathBuf;

pub fn directory(home: &NoEndingHome) -> PathBuf {
    home.root.join("assistant")
}
