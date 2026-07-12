use std::path::PathBuf;

use thiserror::Error;

use crate::build::NodeId;
use crate::core::ResolveError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse {path}: {message}")]
    Parse { path: PathBuf, message: String },

    #[error("failed to parse metadata at {path}: {message}")]
    Metadata { path: PathBuf, message: String },

    #[error("classmap resolve error: {0}")]
    Classmap(#[from] ResolveError),

    #[error("invalid module identifier: {0}")]
    InvalidIdentifier(String),

    #[error("plan error: {0}")]
    Plan(String),

    #[error("transpile error in {path}: {message}")]
    Transpile { path: PathBuf, message: String },

    #[error("graph cycle detected through node {0:?}")]
    GraphCycle(NodeId),

    #[error("node {0:?} not found in graph")]
    UnknownNode(NodeId),

    #[error("classmap fetch failed: {0}")]
    ClassmapFetch(String),

    #[error("scaffold error: {0}")]
    Scaffold(String),

    #[error("watch error: {0}")]
    Watch(String),

    #[error("release error: {0}")]
    Release(String),

    #[error("zip error: {0}")]
    Zip(String),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn parse(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Parse {
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn metadata(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Metadata {
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn transpile(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Transpile {
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn classmap_fetch(message: impl Into<String>) -> Self {
        Self::ClassmapFetch(message.into())
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            path: PathBuf::new(),
            source,
        }
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(source: zip::result::ZipError) -> Self {
        Self::Zip(source.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Self::Parse {
            path: PathBuf::new(),
            message: source.to_string(),
        }
    }
}
