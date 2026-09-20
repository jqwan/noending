//! Main-owned wiring for the physical layer.
//!
//! Each module deliberately depends on a seam instead of a concrete
//! sibling — `workspace::resolver` knows nothing about Projects,
//! `workspace::project` knows nothing about the launcher, and both are testable
//! with a scripted stand-in. That leaves one file in the crate that has to know
//! everybody, and it is this one: it assembles the resolver and Project policy
//! behind the single [`WorkspaceAttaching`] door that command,
//! ingestion and launch code call.
//!
//! Keeping it here is what preserves 方案 §29: there is exactly one object that
//! turns a raw path string into a `WorkspacePath`, so there is exactly one place
//! that decides which Project a directory belongs to.

use std::sync::Arc;

use rusqlite::Connection;

use crate::error::Result;
use crate::workspace::home::{NoEndingHome, ReservedPaths};
use crate::workspace::project::{ProjectProjection, WorkspacePolicy};
use crate::workspace::resolver::{ResolverContext, WorkspaceObserving, WorkspaceResolver};
use crate::workspace::WorkspaceAttaching;

/// NoEnding Home, seen through Project policy (§2, §37).
///
/// Reserved means "the Home itself and its app directories" — never
/// `<home>/workspace`, which is an ordinary working directory that happens to be
/// the default.
pub struct HomePolicy {
    reserved: ReservedPaths,
    default_workspace: Option<String>,
}

impl HomePolicy {
    pub fn new(home: &NoEndingHome) -> Self {
        Self {
            reserved: home.reserved(),
            default_workspace: Some(home.default_workspace_str()),
        }
    }
}

impl WorkspacePolicy for HomePolicy {
    fn is_reserved(&self, canonical_path: &str) -> bool {
        self.reserved.contains(canonical_path)
    }

    fn default_workspace(&self) -> Option<String> {
        self.default_workspace.clone()
    }

    fn exists_on_disk(&self, canonical_path: &str) -> bool {
        super::resolver::exists_on_disk(canonical_path)
    }
}

/// The owned, shareable handle to the physical layer.
///
/// `ProjectProjection` borrows its observer and policy, so it is built per call;
/// this type owns them so `AppState`, the ingestion seam and background sweeps
/// can hold one value.
pub struct WorkspaceLayer {
    observer: Arc<dyn WorkspaceObserving + Send + Sync>,
    policy: Arc<HomePolicy>,
}

impl WorkspaceLayer {
    /// Production wiring: Git detection against the real environment, resolved
    /// once because locating the binary is not free (§42.3-M10).
    pub fn new(home: &NoEndingHome) -> Self {
        Self::with_resolver(
            Arc::new(WorkspaceResolver::new(ResolverContext::from_environment(
                home.reserved(),
            ))),
            home,
        )
    }

    pub fn with_resolver(
        observer: Arc<dyn WorkspaceObserving + Send + Sync>,
        home: &NoEndingHome,
    ) -> Self {
        Self {
            observer,
            policy: Arc::new(HomePolicy::new(home)),
        }
    }

    pub fn projection(&self) -> ProjectProjection<'_> {
        ProjectProjection::with_policy(
            &*self.observer as &dyn WorkspaceObserving,
            &*self.policy as &dyn WorkspacePolicy,
        )
    }
}

impl WorkspaceAttaching for WorkspaceLayer {
    fn ensure_path(&self, conn: &Connection, raw_path: &str) -> Result<Option<String>> {
        self.projection().ensure_path(conn, raw_path)
    }
}
