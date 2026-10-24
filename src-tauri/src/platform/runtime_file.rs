//! Private runtime payloads. Owners keep files alive until consumers are done.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
pub struct RuntimeFile {
    root: PathBuf,
    path: PathBuf,
    cleanup: bool,
}
impl RuntimeFile {
    pub fn create(bytes: &[u8], extension: &str) -> std::io::Result<Self> {
        let root =
            std::env::temp_dir().join(format!("noending-runtime-{}", crate::storage::new_id()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&root)?;
        let file = Self {
            path: root.join(format!("message.{extension}")),
            root,
            cleanup: true,
        };
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&file.path)?.write_all(bytes)?;
        Ok(file)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// External terminals may outlive NoEnding; leave their files to OS temp cleanup.
    pub fn keep(mut self) {
        self.cleanup = false;
    }
}
impl Drop for RuntimeFile {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
