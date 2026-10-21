//! Atomic replacement shared by preferences and exported device profiles.
use crate::protocol::Result;
use std::{
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temporary = parent.join(format!(
        ".peripheral-desk-{}-{stamp}.tmp",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        unsafe {
            use windows_sys::Win32::Storage::FileSystem::*;
            if MoveFileExW(
                crate::hid::wide(&temporary.to_string_lossy()).as_ptr(),
                crate::hid::wide(&path.to_string_lossy()).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Preserve fields introduced by newer plugins when saving known state.
pub fn merge_json(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                if let Some(old) = base.get_mut(&key) {
                    merge_json(old, value)
                } else {
                    base.insert(key, value);
                }
            }
        }
        (serde_json::Value::Array(base), serde_json::Value::Array(patch))
            if !patch.is_empty()
                && patch
                    .iter()
                    .all(|v| v.get("key").and_then(serde_json::Value::as_str).is_some()) =>
        {
            for value in patch {
                let key = value
                    .get("key")
                    .and_then(serde_json::Value::as_str)
                    .unwrap();
                if let Some(old) = base
                    .iter_mut()
                    .find(|v| v.get("key").and_then(serde_json::Value::as_str) == Some(key))
                {
                    merge_json(old, value)
                } else {
                    base.push(value)
                }
            }
        }
        (base, patch) => *base = patch,
    }
}
pub fn write_json_preserving(path: &Path, value: serde_json::Value) -> Result<()> {
    let mut original = match std::fs::read(path) {
        Ok(b) => {
            serde_json::from_slice(&b).map_err(|e| format!("现有配置无效，已保留原文件：{e}"))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Value::Null,
        Err(e) => return Err(e.to_string()),
    };
    merge_json(&mut original, value);
    atomic_write(
        path,
        &serde_json::to_vec_pretty(&original).map_err(|e| e.to_string())?,
    )
}
