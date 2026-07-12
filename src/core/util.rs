use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::core::{Error, Result};

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let contents = fs::read_to_string(path).map_err(|source| Error::io(path, source))?;
    serde_json::from_str(&contents).map_err(|e| Error::parse(path, format!("JSON: {e}")))
}

pub fn read_json_value(path: &Path) -> Result<serde_json::Value> {
    let contents = fs::read_to_string(path).map_err(|source| Error::io(path, source))?;
    serde_json::from_str(&contents).map_err(|e| Error::parse(path, format!("JSON: {e}")))
}

pub fn write_text(path: &Path, contents: &str) -> Result<()> {
    ensure_parent(path)?;
    fs::write(path, contents).map_err(|source| Error::io(path, source))
}

pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_parent(path)?;
    fs::write(path, bytes).map_err(|source| Error::io(path, source))
}

pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
    }
    Ok(())
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| Error::io(path, source))
}

pub fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::metadata(path) {
        Ok(_) => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::IsADirectory => {
                fs::remove_dir_all(path).map_err(|source| Error::io(path, source))
            }
            Err(err) => Err(Error::io(path, err)),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::io(path, err)),
    }
}

pub fn normalize_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn unix_millis_now() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
