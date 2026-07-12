use std::path::PathBuf;
use std::sync::Arc;

use crate::core::{BuildEnvironment, Mapping, Metadata};

#[derive(Debug, Clone)]
pub struct Plan {
    pub metadata: Metadata,
    pub source_dir: PathBuf,
    pub output_dir: PathBuf,
    pub classmap: Arc<Mapping>,
    pub env: Arc<BuildEnvironment>,
}

impl Plan {
    pub fn has_js(&self) -> bool {
        self.metadata.has_js()
    }

    pub fn has_css(&self) -> bool {
        self.metadata.has_css()
    }
}
