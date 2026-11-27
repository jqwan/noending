//! Persistent agent profiles.
//! Each profile owns a registered directory; native sessions reuse the shared
//! launcher, ingestion, project and Skill loading services.
mod files;
mod models;
mod profiles;
pub use models::*;
pub use profiles::*;

use crate::workspace::home::NoEndingHome;
use std::path::PathBuf;

// Existing Homes store absolute archive paths with this directory name.
const ARCHIVE_DIRECTORY_NAME: &str = "assistant";

pub fn directory(home: &NoEndingHome) -> PathBuf {
    home.root.join(ARCHIVE_DIRECTORY_NAME)
}
