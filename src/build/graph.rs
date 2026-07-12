use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blake3::Hasher;

use crate::core::{Error, Result};

#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({})", self.0)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn from_blake3(bytes: &[u8; 32]) -> Self {
        Self(*bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(bytes);
        Self::from_blake3(hasher.finalize().as_bytes())
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|source| Error::io(path, source))?;
        Ok(Self::from_bytes(&bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            use std::fmt::Write;
            let _ = write!(&mut out, "{byte:02x}");
        }
        out
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", &self.to_hex()[..12])
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum SourceKind {
    Js,
    Ts,
    Tsx,
    Jsx,
    Mjs,
    Mts,
    Scss,
    Css,
    Other,
}

impl SourceKind {
    pub fn is_js_like(self) -> bool {
        matches!(
            self,
            Self::Js | Self::Ts | Self::Tsx | Self::Jsx | Self::Mjs | Self::Mts
        )
    }

    pub fn is_css_like(self) -> bool {
        matches!(self, Self::Scss | Self::Css)
    }

    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
            "ts" => Self::Ts,
            "tsx" => Self::Tsx,
            "js" => Self::Js,
            "jsx" => Self::Jsx,
            "mjs" => Self::Mjs,
            "mts" => Self::Mts,
            "scss" => Self::Scss,
            "css" => Self::Css,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceNode {
    pub id: NodeId,
    pub path: PathBuf,
    pub rel_path: PathBuf,
    pub kind: SourceKind,
    pub content_hash: ContentHash,
    pub content: String,
}

impl SourceNode {
    pub fn is_js_like(&self) -> bool {
        self.kind.is_js_like()
    }

    pub fn is_css_like(&self) -> bool {
        self.kind.is_css_like()
    }
}

#[derive(Clone, Debug, Default)]
pub struct BuildGraph {
    nodes: Vec<SourceNode>,
    by_path: HashMap<PathBuf, NodeId>,
}

impl BuildGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn nodes(&self) -> &[SourceNode] {
        &self.nodes
    }

    pub fn node(&self, id: NodeId) -> Result<&SourceNode> {
        self.nodes
            .get(id.raw() as usize)
            .ok_or(Error::UnknownNode(id))
    }

    pub fn node_mut(&mut self, id: NodeId) -> Result<&mut SourceNode> {
        self.nodes
            .get_mut(id.raw() as usize)
            .ok_or(Error::UnknownNode(id))
    }

    pub fn lookup(&self, path: &Path) -> Option<NodeId> {
        self.by_path.get(path).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SourceNode> {
        self.nodes.iter()
    }

    pub fn insert(&mut self, mut node: SourceNode) -> Result<NodeId> {
        if self.by_path.contains_key(&node.path) {
            return Err(Error::Plan(format!(
                "duplicate node for path {}",
                node.path.display()
            )));
        }
        let id = NodeId(self.nodes.len() as u32);
        node.id = id;
        self.by_path.insert(node.path.clone(), id);
        self.nodes.push(node);
        Ok(id)
    }

    pub fn remove(&mut self, id: NodeId) -> Option<SourceNode> {
        let node = self.nodes.get(id.raw() as usize)?;
        self.by_path.remove(&node.path);
        Some(self.nodes.remove(id.raw() as usize))
    }

    pub fn replace(&mut self, id: NodeId, mut node: SourceNode) -> Result<()> {
        let slot = self
            .nodes
            .get(id.raw() as usize)
            .ok_or(Error::UnknownNode(id))?;
        if slot.path != node.path {
            return Err(Error::Plan(format!(
                "replace at {id:?} would change path from {} to {}",
                slot.path.display(),
                node.path.display()
            )));
        }
        node.id = id;
        self.nodes[id.raw() as usize] = node;
        Ok(())
    }

    pub fn refresh_from_disk(&mut self, root: &Path) -> Result<()> {
        self.nodes.clear();
        self.by_path.clear();

        let paths = crate::build::walk(root)?;
        for path in paths {
            self.read_and_insert(root, &path)?;
        }
        Ok(())
    }

    pub fn upsert(&mut self, root: &Path, path: &Path) -> Result<NodeId> {
        if let Some(id) = self.lookup(path) {
            self.read_and_replace(root, id, path)?;
            Ok(id)
        } else {
            self.read_and_insert(root, path)
        }
    }

    pub fn remove_by_path(&mut self, path: &Path) -> Option<NodeId> {
        let id = self.lookup(path)?;
        self.remove(id);
        Some(id)
    }

    fn read_and_insert(&mut self, root: &Path, path: &Path) -> Result<NodeId> {
        let content = std::fs::read_to_string(path).map_err(|source| Error::io(path, source))?;
        let content_hash = ContentHash::from_bytes(content.as_bytes());
        let kind = SourceKind::from_path(path);
        let rel_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
        self.insert(SourceNode {
            id: NodeId(0),
            path: path.to_path_buf(),
            rel_path,
            kind,
            content_hash,
            content,
        })
    }

    fn read_and_replace(&mut self, root: &Path, id: NodeId, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path).map_err(|source| Error::io(path, source))?;
        let content_hash = ContentHash::from_bytes(content.as_bytes());
        let kind = SourceKind::from_path(path);
        let rel_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
        self.replace(
            id,
            SourceNode {
                id: NodeId(0),
                path: path.to_path_buf(),
                rel_path,
                kind,
                content_hash,
                content,
            },
        )
    }
}

#[derive(Clone)]
pub struct SharedGraph(Arc<std::sync::RwLock<BuildGraph>>);

impl SharedGraph {
    pub fn new(graph: BuildGraph) -> Self {
        Self(Arc::new(std::sync::RwLock::new(graph)))
    }

    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, BuildGraph> {
        self.0.read().expect("graph lock poisoned")
    }

    pub fn write(&self) -> std::sync::RwLockWriteGuard<'_, BuildGraph> {
        self.0.write().expect("graph lock poisoned")
    }
}

impl Default for SharedGraph {
    fn default() -> Self {
        Self::new(BuildGraph::default())
    }
}
