use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read JSON file: {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse JSON file: {}", path.display()))
}

pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    Ok(())
}

pub fn write_text(path: &Path, contents: &str) -> Result<()> {
    ensure_parent(path)?;
    fs::write(path, contents).with_context(|| format!("Failed to write file: {}", path.display()))
}

pub fn normalize_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
