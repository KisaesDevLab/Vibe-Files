//! Reading and writing the JSON files in `data/`: written to a temp file next to the target, then
//! renamed over it, so a crash never leaves a half-written file.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Writes `bytes` to `path` through `<path>.tmp` and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Pretty-printed JSON, written atomically.
pub fn save_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> anyhow::Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(value)?)
}

/// `None` when the file doesn't exist.
pub fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
