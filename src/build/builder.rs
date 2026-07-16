use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::build::{NodeId, SharedGraph, SourceKind, SourceNode, build_shared_graph};
use crate::core::{Plan, Result, util};
use crate::transpile::css::CssTranspiler;
use crate::transpile::js::JsTranspiler;
use crate::transpile::{Transpile, TranspileContext, TranspileOutput};

#[derive(Debug, Default, Clone, Copy)]
pub struct DriverOptions {
    pub include_unknown: bool,
}

#[derive(Debug, Default, Clone)]
pub struct BuilderOutcome {
    pub transpiled: Vec<PathBuf>,
    pub copied: Vec<PathBuf>,
}

pub struct Builder {
    plan: Plan,
    graph: SharedGraph,
    options: DriverOptions,
    js_transpiler: JsTranspiler,
    css_transpiler: CssTranspiler,
}

impl Builder {
    pub fn new(plan: Plan) -> Result<Self> {
        Self::with_options(plan, DriverOptions::default())
    }

    pub fn with_options(plan: Plan, options: DriverOptions) -> Result<Self> {
        let graph = build_shared_graph(&plan.source_dir)?;
        let js_transpiler = JsTranspiler::new((*plan.classmap).clone());
        let css_transpiler = CssTranspiler::new((*plan.classmap).clone());
        Ok(Self {
            plan,
            graph,
            options,
            js_transpiler,
            css_transpiler,
        })
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn source_dir(&self) -> &Path {
        &self.plan.source_dir
    }

    pub fn output_dir(&self) -> &Path {
        &self.plan.output_dir
    }

    pub fn options(&self) -> DriverOptions {
        self.options
    }

    pub fn graph(&self) -> SharedGraph {
        self.graph.clone()
    }

    pub fn replace_graph(&self, new_graph: crate::build::BuildGraph) {
        let mut guard = self.graph.write();
        *guard = new_graph;
    }

    pub fn upsert_node(&self, path: &Path) -> Result<Option<NodeId>> {
        let mut guard = self.graph.write();
        if path.exists() {
            Ok(Some(guard.upsert(&self.plan.source_dir, path)?))
        } else {
            Ok(guard.remove_by_path(path))
        }
    }

    pub fn run(&self) -> Result<BuilderOutcome> {
        let node_ids: Vec<NodeId> = {
            let guard = self.graph.read();
            (0..guard.nodes().len() as u32)
                .map(NodeId::from_raw)
                .collect()
        };
        self.run_for(&node_ids)
    }

    pub fn run_for(&self, node_ids: &[NodeId]) -> Result<BuilderOutcome> {
        util::ensure_dir(&self.plan.output_dir)?;
        let dep_timestamps = self.resolve_dependency_timestamps();
        let ctx = TranspileContext::new(
            self.plan.env.clone(),
            util::unix_millis_now(),
            dep_timestamps,
        );
        let mut outcome = BuilderOutcome::default();
        {
            let graph = self.graph.read();
            for &id in node_ids {
                let node = graph.node(id)?;
                if !self.should_process(node) {
                    continue;
                }
                self.process_node(node, &ctx, &mut outcome)?;
            }
        }
        self.write_timestamp(ctx.timestamp)?;
        Ok(outcome)
    }

    fn write_timestamp(&self, timestamp: u128) -> Result<()> {
        let path = self.plan.output_dir.join("timestamp");
        if self.plan.env.dev {
            util::write_text(&path, &timestamp.to_string())?;
        } else if path.exists() {
            std::fs::remove_file(&path).map_err(|source| crate::core::Error::io(&path, source))?;
        }
        Ok(())
    }

    fn resolve_dependency_timestamps(&self) -> HashMap<String, u128> {
        let mut dep_timestamps = HashMap::new();
        let Some(modules_dir) = self.plan.source_dir.parent() else {
            return dep_timestamps;
        };
        for dep_name in self.plan.metadata.dependencies.keys() {
            let dep_dir = modules_dir.join(dep_name.as_str());
            let ts_path = dep_dir.join("timestamp");
            if let Ok(contents) = std::fs::read_to_string(&ts_path)
                && let Ok(ts) = contents.trim().parse::<u128>()
            {
                dep_timestamps.insert(dep_name.as_str().to_string(), ts);
            }
        }
        dep_timestamps
    }

    fn should_process(&self, node: &SourceNode) -> bool {
        if node.kind == SourceKind::Other {
            return self.options.include_unknown;
        }
        if node.kind == SourceKind::Js && has_transpiled_sibling(&node.path) {
            return false;
        }
        true
    }

    fn process_node(
        &self,
        node: &SourceNode,
        ctx: &TranspileContext,
        outcome: &mut BuilderOutcome,
    ) -> Result<()> {
        if node.kind == SourceKind::Other {
            self.copy_other(node)?;
            outcome.copied.push(node.rel_path.clone());
            return Ok(());
        }

        let output: TranspileOutput = self.transpiler_for(node.kind).transpile(node, ctx)?;
        self.write_bytes(node, output.code.as_bytes())?;
        outcome.transpiled.push(node.rel_path.clone());
        Ok(())
    }

    fn transpiler_for(&self, kind: SourceKind) -> &dyn Transpile {
        match kind {
            SourceKind::Js
            | SourceKind::Ts
            | SourceKind::Tsx
            | SourceKind::Jsx
            | SourceKind::Mjs
            | SourceKind::Mts => &self.js_transpiler,
            SourceKind::Scss | SourceKind::Css => &self.css_transpiler,
            SourceKind::Other => unreachable!("Other is handled before dispatch"),
        }
    }

    fn write_bytes(&self, node: &SourceNode, bytes: &[u8]) -> Result<()> {
        let dest = self.output_path(node);
        util::write_bytes(&dest, bytes)
    }

    fn copy_other(&self, node: &SourceNode) -> Result<()> {
        let dest = self.plan.output_dir.join(&node.rel_path);
        util::ensure_parent(&dest)?;
        std::fs::copy(&node.path, &dest).map_err(|source| crate::core::Error::io(&dest, source))?;
        Ok(())
    }

    fn output_path(&self, node: &SourceNode) -> PathBuf {
        let mut rel = node.rel_path.clone();
        let ext = match node.kind {
            SourceKind::Js
            | SourceKind::Ts
            | SourceKind::Tsx
            | SourceKind::Jsx
            | SourceKind::Mjs
            | SourceKind::Mts => "js",
            SourceKind::Scss => "css",
            _ => return self.plan.output_dir.join(&rel),
        };
        rel.set_extension(ext);
        self.plan.output_dir.join(&rel)
    }
}

fn has_transpiled_sibling(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    ["ts", "tsx", "jsx", "mjs", "mts"]
        .iter()
        .any(|ext| parent.join(format!("{stem}.{ext}")).exists())
}
