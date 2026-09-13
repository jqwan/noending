pub mod exec_resolver;
pub mod exec_runner;
pub mod launcher;
pub mod paths;

use serde::Serialize;

/// Operating systems we explicitly support. v0.2 keeps Windows/macOS
/// differences behind this layer so Domain code never branches on OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

impl Platform {
    pub fn current() -> Platform {
        #[cfg(target_os = "macos")]
        return Platform::Macos;
        #[cfg(target_os = "windows")]
        return Platform::Windows;
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return Platform::Linux;
    }
}
