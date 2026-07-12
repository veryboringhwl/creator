use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::build::{BuildGraph, ContentHash, SharedGraph, SourceKind, SourceNode};
use crate::core::{Error, Result};

const INTERESTING_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "mts", "scss", "css"];

fn is_interesting(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| INTERESTING_EXTENSIONS.contains(&ext))
}

pub fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let iter = WalkDir::new(root).into_iter();
    for entry in iter {
        let entry = entry.map_err(|source| Error::io(root, source.into()))?;
        if entry.file_type().is_file() && is_interesting(entry.path()) {
            out.push(entry.path().to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

pub fn build_graph(root: &Path) -> Result<BuildGraph> {
    let mut graph = BuildGraph::new();
    let paths = walk(root)?;
    for path in paths {
        read_into(&mut graph, root, &path)?;
    }
    Ok(graph)
}

pub fn build_shared_graph(root: &Path) -> Result<SharedGraph> {
    Ok(SharedGraph::new(build_graph(root)?))
}

pub fn read_into(graph: &mut BuildGraph, root: &Path, path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(path).map_err(|source| Error::io(path, source))?;
    let content_hash = ContentHash::from_bytes(content.as_bytes());
    let kind = SourceKind::from_path(path);
    let rel_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    graph.insert(SourceNode {
        id: crate::build::NodeId(0),
        path: path.to_path_buf(),
        rel_path,
        kind,
        content_hash,
        content,
    })?;
    Ok(())
}
