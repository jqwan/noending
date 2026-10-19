use crate::error::Result;
use std::{fs, path::Path};

pub(super) fn read(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !path.is_symlink() => {
            Ok(String::new())
        }
        Err(e) => Err(e.into()),
    }
}

pub(super) fn revision(path: &Path) -> Result<String> {
    Ok(super::key(read(path)?))
}
