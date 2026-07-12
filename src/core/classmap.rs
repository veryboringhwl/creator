use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum MapInner {
    Leaf(String),
    Node(BTreeMap<String, Mapping>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Mapping(MapInner);

impl Mapping {
    pub fn empty() -> Self {
        Self(MapInner::Node(BTreeMap::new()))
    }

    pub fn lookup(&self, path: &[&str]) -> Result<Option<&str>> {
        let mut current = self;
        for (i, key) in path.iter().enumerate() {
            match &current.0 {
                MapInner::Leaf(_) => {
                    return Err(Error::Classmap(ResolveError::NotALeaf {
                        path: path[..i].iter().map(|s| s.to_string()).collect(),
                    }));
                }
                MapInner::Node(map) => match map.get(*key) {
                    Some(next) => current = next,
                    None => {
                        return Ok(None);
                    }
                },
            }
        }
        match &current.0 {
            MapInner::Leaf(value) => Ok(Some(value.as_str())),
            MapInner::Node(_) => Err(Error::Classmap(ResolveError::NotALeaf {
                path: path.iter().map(|s| s.to_string()).collect(),
            })),
        }
    }

    pub fn resolve(&self, path: &[&str]) -> Result<&str> {
        self.lookup(path)?.ok_or_else(|| {
            Error::Classmap(crate::core::ResolveError::NotFound {
                matched: path.len(),
                path: path.iter().map(|s| s.to_string()).collect(),
            })
        })
    }
}

impl Default for Mapping {
    fn default() -> Self {
        Self::empty()
    }
}

pub struct CssMappingRef<'a> {
    inner: &'a Mapping,
}

impl<'a> CssMappingRef<'a> {
    pub fn new(inner: &'a Mapping) -> Self {
        Self { inner }
    }

    pub fn lookup(&self, css_path: &str) -> Result<Option<&str>> {
        let path: Vec<&str> = css_path.split("__").collect();
        let mut current = self.inner;
        for (i, css_key) in path.iter().enumerate() {
            match &current.0 {
                MapInner::Leaf(_) => {
                    return Err(Error::Classmap(ResolveError::NotALeaf {
                        path: path[..i].iter().map(|s| s.to_string()).collect(),
                    }));
                }
                MapInner::Node(map) => {
                    let next = map
                        .get(*css_key)
                        .or_else(|| map.get(&css_key.replace('-', "_")));
                    match next {
                        Some(n) => current = n,
                        None => return Ok(None),
                    }
                }
            }
        }
        match &current.0 {
            MapInner::Leaf(value) => Ok(Some(value.as_str())),
            MapInner::Node(_) => Err(Error::Classmap(ResolveError::NotALeaf {
                path: path.iter().map(|s| s.to_string()).collect(),
            })),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ResolveError {
    #[error("classmap path matched {matched} segment(s) but next segment not found: {path:?}")]
    NotFound { matched: usize, path: Vec<String> },

    #[error("classmap path is not a leaf: {path:?}")]
    NotALeaf { path: Vec<String> },
}

#[derive(Clone, Debug)]
pub struct ClassmapInfo {
    pub mapping: Mapping,
    pub version: u64,
}

pub fn gen_classmap_dts(mapping: &Mapping) -> String {
    fn gen_type(mapping: &Mapping) -> String {
        match &mapping.0 {
            MapInner::Leaf(value) => format!("\"{value}\""),
            MapInner::Node(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                let mut out = String::new();
                for (key, value) in entries.drain(..) {
                    out.push_str(&format!("readonly \"{key}\":{},", gen_type(value)));
                }
                format!("{{{out}}}")
            }
        }
    }

    format!(
        "/* Spicetify Classmap */\n\ndeclare const MAP: {};\n",
        gen_type(mapping)
    )
}

pub fn fetch_classmap_info(url: &str) -> Result<ClassmapInfo> {
    let semver_re = Regex::new(
        r"^https://raw\.githubusercontent\.com/[^/]+/[^/]+/[^/]+/(?P<semver>\d+\.\d+\.\d+)/classmap\.json$",
    )
    .map_err(|e| Error::classmap_fetch(e.to_string()))?;

    let caps = semver_re.captures(url).ok_or_else(|| {
        Error::classmap_fetch(format!(
            "invalid classmap url: {url}. Expected https://raw.githubusercontent.com/<owner>/<repo>/<ref>/<major>.<minor>.<patch>/classmap.json"
        ))
    })?;

    let semver = caps
        .name("semver")
        .ok_or_else(|| Error::classmap_fetch(format!("missing classmap semver in url: {url}")))?
        .as_str();

    let mut parts = semver.split('.').map(|part| part.parse::<u64>());
    let major = parts
        .next()
        .ok_or_else(|| Error::classmap_fetch(format!("invalid classmap semver in url: {url}")))?
        .map_err(|e| {
            Error::classmap_fetch(format!("invalid classmap semver in url: {url}: {e}"))
        })?;
    let minor = parts
        .next()
        .ok_or_else(|| Error::classmap_fetch(format!("invalid classmap semver in url: {url}")))?
        .map_err(|e| {
            Error::classmap_fetch(format!("invalid classmap semver in url: {url}: {e}"))
        })?;
    let patch = parts
        .next()
        .ok_or_else(|| Error::classmap_fetch(format!("invalid classmap semver in url: {url}")))?
        .map_err(|e| {
            Error::classmap_fetch(format!("invalid classmap semver in url: {url}: {e}"))
        })?;
    if parts.next().is_some() {
        return Err(Error::classmap_fetch(format!(
            "invalid classmap semver in url: {url}"
        )));
    }
    let version = major * 1_000_000 + minor * 1_000 + patch;

    let body = reqwest::blocking::get(url)
        .map_err(|e| Error::classmap_fetch(format!("failed to fetch classmap from {url}: {e}")))?
        .error_for_status()
        .map_err(|e| Error::classmap_fetch(format!("failed to fetch classmap from {url}: {e}")))?
        .text()
        .map_err(|e| Error::classmap_fetch(format!("failed to read classmap from {url}: {e}")))?;

    let mapping: Mapping = serde_json::from_str(&body)
        .map_err(|e| Error::classmap_fetch(format!("failed to parse classmap from {url}: {e}")))?;

    Ok(ClassmapInfo { mapping, version })
}

pub fn discover_module_dirs(modules_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    let entries =
        std::fs::read_dir(modules_dir).map_err(|source| Error::io(modules_dir, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::io(modules_dir, source))?;
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    Ok(dirs)
}

pub fn classmap_semver_display(version: u64) -> String {
    let major = version / 1_000_000;
    let minor = (version / 1_000) % 1_000;
    let patch = version % 1_000;
    format!("{major}.{minor}.{patch}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Mapping {
        let json = r#"{
            "main": {
                "top_bar": {
                    "left": { "button": { "wrapper": "X1" } },
                    "right": { "button": { "wrapper": "X2" } }
                }
            }
        }"#;
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn lookup_full_path() {
        let m = sample();
        assert_eq!(
            m.lookup(&["main", "top_bar", "left", "button", "wrapper"])
                .unwrap(),
            Some("X1")
        );
    }

    #[test]
    fn lookup_not_found_returns_none() {
        let m = sample();
        assert!(m.lookup(&["main", "top_bar", "middle"]).unwrap().is_none());
    }

    #[test]
    fn lookup_not_a_leaf_errors() {
        let m = sample();
        let err = m.lookup(&["main", "top_bar"]).unwrap_err();
        assert!(matches!(
            err,
            Error::Classmap(ResolveError::NotALeaf { .. })
        ));
    }

    #[test]
    fn resolve_wraps_lookup() {
        let m = sample();
        assert_eq!(
            m.resolve(&["main", "top_bar", "left", "button", "wrapper"])
                .unwrap(),
            "X1"
        );
        assert!(m.resolve(&["main", "top_bar", "middle"]).is_err());
    }

    #[test]
    fn css_ref_resolves_with_dash() {
        let m = sample();
        let css = CssMappingRef::new(&m);
        assert_eq!(
            css.lookup("main__top-bar__left__button__wrapper").unwrap(),
            Some("X1")
        );
    }

    #[test]
    fn css_ref_resolves_original_form() {
        let m = sample();
        let css = CssMappingRef::new(&m);
        assert_eq!(
            css.lookup("main__top_bar__left__button__wrapper").unwrap(),
            Some("X1")
        );
    }

    #[test]
    fn css_ref_not_found_returns_none() {
        let m = sample();
        let css = CssMappingRef::new(&m);
        assert!(css.lookup("nope__top_bar").unwrap().is_none());
    }
}
