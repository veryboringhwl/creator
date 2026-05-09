use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::util::read_json;

pub struct TimestampResolver {
    config_dir: PathBuf,
    modules: Vec<String>,
}

impl TimestampResolver {
    pub fn new() -> Option<Self> {
        let config_dir = env::var("SPICETIFY_CONFIG_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(default_config_dir)?;
        let vault_path = config_dir.join("modules").join("vault.json");
        let vault: Value = read_json(&vault_path).ok()?;
        let modules_obj = vault.get("modules")?.as_object()?;
        let mut modules: Vec<String> = modules_obj.keys().cloned().collect();
        modules.sort_by(|a, b| b.len().cmp(&a.len()));
        Some(Self {
            config_dir,
            modules,
        })
    }

    pub fn resolve(&self, import_path: &str) -> Result<Option<String>> {
        let rel = match import_path.strip_prefix("/modules") {
            Some(rest) => rest,
            None => return Ok(None),
        };
        let rel_trimmed = rel.trim_start_matches('/');
        let module = match self.modules.iter().find(|module| {
            rel.starts_with(module.as_str()) || rel_trimmed.starts_with(module.as_str())
        }) {
            Some(module) => module,
            None => return Ok(None),
        };
        let timestamp_path = self
            .config_dir
            .join("modules")
            .join(module.trim_start_matches('/'))
            .join("timestamp");
        match std::fs::read_to_string(&timestamp_path) {
            Ok(contents) => Ok(Some(contents.trim().to_string())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| {
                format!(
                    "Failed to read timestamp file: {}",
                    timestamp_path.display()
                )
            }),
        }
    }
}

fn default_config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            return Some(PathBuf::from(local_app_data).join("spicetify"));
        }
    }

    if let Ok(xdg_config_home) = env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg_config_home).join("spicetify"));
    }

    if let Ok(home) = env::var("HOME") {
        return Some(PathBuf::from(home).join(".config").join("spicetify"));
    }

    None
}
