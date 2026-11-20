//! Local changes for specific published M HUB frontend builds.
use std::{
    fs::File,
    io::{ErrorKind, Read},
    path::Path,
};

#[path = "fixes/g75_patch.rs"]
mod g75;
use g75::{patched, MODULE, STAMP};

pub(super) fn apply(root: &Path) -> Result<(), String> {
    let path = root.join(MODULE);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let mut prefix = [0; STAMP.len()];
    match file.read_exact(&mut prefix) {
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(()),
        Err(e) => return Err(e.to_string()),
        Ok(()) if prefix == STAMP.as_bytes() => return Ok(()),
        Ok(()) => {}
    }
    let mut source = prefix.to_vec();
    file.read_to_end(&mut source).map_err(|e| e.to_string())?;
    drop(file);
    let source = std::str::from_utf8(&source).map_err(|e| e.to_string())?;
    if let Some(result) = patched(source) {
        crate::storage::atomic_write(&path, result.as_bytes())?;
    }
    Ok(())
}
