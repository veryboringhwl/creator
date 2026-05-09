use std::{
    collections::HashMap, path::{Path, PathBuf}
};

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Mapping {
    Str(String),
    Map(HashMap<String, Mapping>),
}

impl Default for Mapping {
    fn default() -> Self {
        Mapping::Map(HashMap::new())
    }
}

#[derive(Clone, Debug)]
pub struct ClassmapInfo {
    pub mapping: Mapping,
    pub version: u64,
}

pub fn gen_classmap_dts(mapping: &Mapping) -> String {
    fn gen_type(mapping: &Mapping) -> String {
        match mapping {
            Mapping::Str(value) => format!("\"{}\"", value),
            Mapping::Map(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                let mut out = String::new();
                for (key, value) in entries {
                    out.push_str(&format!("readonly \"{}\":{},", key, gen_type(value)));
                }
                format!("{{{}}}", out)
            }
        }
    }

    format!(
        "/* Bespoke Tailored Classmap (BTC) */\n\ndeclare const MAP: {};\n",
        gen_type(mapping)
    )
}

pub fn reformat_mapping_for_css(mapping: &Mapping) -> Mapping {
    match mapping {
        Mapping::Str(value) => Mapping::Str(value.clone()),
        Mapping::Map(map) => {
            let mut remapped = HashMap::new();
            for (key, value) in map {
                let key = key.replace('_', "-");
                remapped.insert(key, reformat_mapping_for_css(value));
            }
            Mapping::Map(remapped)
        }
    }
}

pub fn fetch_classmap_info(url: &str) -> Result<ClassmapInfo> {
    let semver_re = Regex::new(
        r"^https://raw\.githubusercontent\.com/[^/]+/[^/]+/[^/]+/(?P<semver>\d+\.\d+\.\d+)/classmap\.json$",
    )?;

    let version = if let Some(caps) = semver_re.captures(url) {
        let semver = caps
            .name("semver")
            .ok_or_else(|| anyhow!("Missing classmap semver in url: {url}"))?
            .as_str();
        let mut parts = semver.split('.').map(|part| part.parse::<u64>());
        let major = parts
            .next()
            .ok_or_else(|| anyhow!("Invalid classmap semver in url: {url}"))?
            .with_context(|| format!("Invalid classmap semver in url: {url}"))?;
        let minor = parts
            .next()
            .ok_or_else(|| anyhow!("Invalid classmap semver in url: {url}"))?
            .with_context(|| format!("Invalid classmap semver in url: {url}"))?;
        let patch = parts
            .next()
            .ok_or_else(|| anyhow!("Invalid classmap semver in url: {url}"))?
            .with_context(|| format!("Invalid classmap semver in url: {url}"))?;

        if parts.next().is_some() {
            return Err(anyhow!("Invalid classmap semver in url: {url}"));
        }

        major * 1_000_000 + minor * 1_000 + patch
    } else {
        return Err(anyhow!(
            "Invalid classmap url: {url}. Expected https://raw.githubusercontent.com/<owner>/<repo>/<ref>/<major>.<minor>.<patch>/classmap.json"
        ));
    };

    let response = reqwest::blocking::get(url)
        .with_context(|| format!("Failed to fetch classmap from {url}"))?;
    let body = response
        .text()
        .with_context(|| format!("Failed to read classmap from {url}"))?;
    let mapping: Mapping = serde_json::from_str(&body)
        .with_context(|| format!("Failed to parse classmap from {url}"))?;

    Ok(ClassmapInfo { mapping, version })
}

pub fn discover_module_dirs(modules_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(modules_dir)
        .with_context(|| format!("Failed to read modules dir: {}", modules_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    Ok(dirs)
}
