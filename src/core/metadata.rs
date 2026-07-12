use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::core::{Error, Result};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialOrd, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModuleIdentifier(String);

impl ModuleIdentifier {
    pub fn new(s: impl Into<String>) -> Result<Self> {
        let s = s.into();
        if s.is_empty() || s.chars().any(char::is_whitespace) {
            return Err(Error::InvalidIdentifier(s));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModuleIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ModuleIdentifier {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<ModuleIdentifier> for String {
    fn from(id: ModuleIdentifier) -> Self {
        id.0
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MetadataEntries {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub js: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    pub version: String,
    pub authors: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
    #[serde(default)]
    pub entries: MetadataEntries,
    #[serde(default)]
    pub dependencies: BTreeMap<ModuleIdentifier, semver::VersionReq>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_mixins: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_vault: Option<bool>,
}

impl Metadata {
    pub fn has_js(&self) -> bool {
        self.entries.js.is_some()
    }

    pub fn has_css(&self) -> bool {
        self.entries.css.is_some()
    }

    pub fn module_root_from(&self, metadata_path: &std::path::Path) -> Option<PathBuf> {
        metadata_path.parent().map(|p| p.to_path_buf())
    }

    pub fn parse_version(&self) -> Option<semver::Version> {
        semver::Version::from_str(&self.version).ok()
    }
}
