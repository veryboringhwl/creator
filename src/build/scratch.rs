use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::{Result, ensure_dir};

pub struct ScratchSession {
    dir: PathBuf,
    counter: AtomicU64,
}

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

impl ScratchSession {
    pub fn new() -> Result<Self> {
        let n = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("creator-scratch-{pid}-{n}"));
        ensure_dir(&dir)?;
        Ok(Self {
            dir,
            counter: AtomicU64::new(0),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn next_path(&self, ext: &str) -> PathBuf {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        self.dir.join(format!("scratch-{n}.{ext}"))
    }
}

impl Drop for ScratchSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
