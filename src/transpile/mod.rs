pub mod css;
pub mod js;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::build::{ScratchSession, SourceKind, SourceNode};
use crate::core::{BuildEnvironment, Result};

#[derive(Debug, Clone)]
pub struct TranspileOutput {
    pub code: String,
    pub source_map: Option<String>,
}

pub struct TranspileContext {
    pub env: Arc<BuildEnvironment>,
    pub timestamp: u128,
    dep_timestamps: HashMap<String, u128>,
    scratch: OnceLock<ScratchSession>,
}

impl TranspileContext {
    pub fn new(
        env: Arc<BuildEnvironment>,
        timestamp: u128,
        dep_timestamps: HashMap<String, u128>,
    ) -> Self {
        Self {
            env,
            timestamp,
            dep_timestamps,
            scratch: OnceLock::new(),
        }
    }

    pub fn resolve_dep_timestamp(&self, module_name: &str) -> Option<u128> {
        self.dep_timestamps.get(module_name).copied()
    }

    pub fn scratch_session(&self) -> &ScratchSession {
        self.scratch
            .get_or_init(|| ScratchSession::new().expect("scratch session init failed"))
    }
}

pub trait Transpile: Send + Sync {
    fn kind(&self) -> SourceKind;

    fn transpile(&self, node: &SourceNode, ctx: &TranspileContext) -> Result<TranspileOutput>;
}
