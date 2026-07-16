use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use owo_colors::OwoColorize;

use crate::build::{Builder, BuilderOutcome, DriverOptions};
use crate::core::{BuildEnvironment, Metadata, Plan, Result, classmap, util};

pub struct Workspace {
    modules: Vec<Module>,
    dep_graph: HashMap<String, Vec<String>>,
    rev_deps: HashMap<String, Vec<String>>,
}

struct Module {
    name: String,
    dir: PathBuf,
    builder: Builder,
}

impl Workspace {
    pub fn from_modules(
        module_dirs: &[PathBuf],
        classmap_path: &Path,
        env: Arc<BuildEnvironment>,
    ) -> Result<Self> {
        let mapping: classmap::Mapping = util::read_json(classmap_path)?;
        let classmap = Arc::new(mapping);

        let mut modules = Vec::new();
        let mut dep_graph: HashMap<String, Vec<String>> = HashMap::new();
        let mut seen: HashMap<String, PathBuf> = HashMap::new();

        for dir in module_dirs {
            let dir = std::path::absolute(dir).map_err(|e| {
                crate::core::Error::Watch(format!("resolve {}: {e}", dir.display()))
            })?;
            let metadata_path = dir.join("metadata.json");
            let metadata: Metadata = util::read_json(&metadata_path)?;
            let name = metadata.name.clone();

            if let Some(existing) = seen.get(&name) {
                return Err(crate::core::Error::Plan(format!(
                    "duplicate module name '{}': {} and {}",
                    name,
                    existing.display(),
                    dir.display(),
                )));
            }
            seen.insert(name.clone(), dir.clone());

            let deps: Vec<String> = metadata
                .dependencies
                .keys()
                .map(|id| id.as_str().to_string())
                .collect();
            dep_graph.insert(name.clone(), deps);

            let plan = Plan {
                metadata,
                source_dir: dir.clone(),
                output_dir: dir.clone(),
                classmap: classmap.clone(),
                env: env.clone(),
            };
            let builder = Builder::with_options(plan, DriverOptions::default())?;
            modules.push(Module {
                name,
                dir: dir.clone(),
                builder,
            });
        }

        let mut rev_deps: HashMap<String, Vec<String>> = HashMap::new();
        for (name, deps) in &dep_graph {
            for dep in deps {
                rev_deps.entry(dep.clone()).or_default().push(name.clone());
            }
        }

        Ok(Self {
            modules,
            dep_graph,
            rev_deps,
        })
    }

    pub fn module_names(&self) -> Vec<&str> {
        self.modules.iter().map(|m| m.name.as_str()).collect()
    }

    pub fn build_all(&self) -> Result<BuildOutcomeMap> {
        let mut outcome = BuildOutcomeMap::new();
        let levels = self.build_levels();
        let num_levels = levels.len();
        for (i, level) in levels.iter().enumerate() {
            std::thread::scope(|s| {
                let handles: Vec<_> = level
                    .iter()
                    .map(|name| {
                        let module = self.find_module(name).expect("module missing");
                        s.spawn(move || (name.clone(), module.builder.run()))
                    })
                    .collect();
                for handle in handles {
                    let (name, result) = handle.join().expect("thread panicked");
                    match result {
                        Ok(build) => {
                            outcome.insert(name, build);
                        }
                        Err(err) => {
                            tracing::error!(%name, ?err, "module build failed");
                        }
                    }
                }
            });
            tracing::info!(level = i + 1, num_levels, "build level complete");
        }
        Ok(outcome)
    }

    fn build_levels(&self) -> Vec<Vec<String>> {
        let all = self.all_names_set();
        let mut remaining: HashSet<String> = all;
        let mut levels: Vec<Vec<String>> = Vec::new();

        while !remaining.is_empty() {
            let ready: Vec<String> = remaining
                .iter()
                .filter(|name| {
                    self.dep_graph
                        .get(*name)
                        .map(|deps| deps.iter().all(|d| !remaining.contains(d)))
                        .unwrap_or(true)
                })
                .cloned()
                .collect();

            if ready.is_empty() {
                tracing::warn!(?remaining, "dependency cycle detected, breaking");
                break;
            }

            for name in &ready {
                remaining.remove(name);
            }
            levels.push(ready);
        }

        levels
    }

    pub fn watch(&self, debounce: Duration) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default(),
        )
        .map_err(|e| crate::core::Error::Watch(e.to_string()))?;

        for module in &self.modules {
            watcher
                .watch(&module.dir, RecursiveMode::Recursive)
                .map_err(|e| {
                    crate::core::Error::Watch(format!("watch {}: {e}", module.dir.display()))
                })?;
        }

        let mut pending = PendingChanges::new();

        loop {
            let event = match rx.recv() {
                Ok(ev) => ev,
                Err(_) => {
                    return Err(crate::core::Error::Watch(
                        "file watcher channel closed".into(),
                    ));
                }
            };
            let event = event.map_err(|e| crate::core::Error::Watch(e.to_string()))?;
            self.apply_watch_event(&event, &mut pending);

            if let Err(err) = self.rebuild_cascade(&mut pending) {
                tracing::error!(?err, "watch build failed");
            }

            let mut deadline = Instant::now() + debounce;
            loop {
                let timeout = deadline.saturating_duration_since(Instant::now());
                match rx.recv_timeout(timeout) {
                    Ok(event) => {
                        let event = event.map_err(|e| crate::core::Error::Watch(e.to_string()))?;
                        self.apply_watch_event(&event, &mut pending);
                        deadline = Instant::now() + debounce;
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(crate::core::Error::Watch(
                            "file watcher disconnected".into(),
                        ));
                    }
                }
            }

            if let Err(err) = self.rebuild_cascade(&mut pending) {
                tracing::error!(?err, "watch build failed");
            }
        }
    }

    fn apply_watch_event(&self, event: &notify::Event, pending: &mut PendingChanges) {
        if !matches!(
            event.kind,
            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
        ) {
            return;
        }
        for path in &event.paths {
            if let Some(module) = self.find_module_for_path(path) {
                pending.add(module.name.clone(), path.clone());
            }
        }
    }

    fn rebuild_cascade(&self, pending: &mut PendingChanges) -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }

        let changed: HashMap<String, HashSet<PathBuf>> = pending.drain();
        let directly_changed: HashSet<String> = changed.keys().cloned().collect();
        let affected = self.affected_closure(&directly_changed);
        let order = self.topological_order_of(&affected);

        let mut outcome = BuildOutcomeMap::new();
        for name in &order {
            let module = self
                .find_module(name)
                .expect("module in rebuild order missing");
            let build = if let Some(paths) = changed.get(name) {
                self.upsert_and_run(module, paths)?
            } else {
                module.builder.run()?
            };
            outcome.insert(name.clone(), build);
        }

        let max_name = outcome.keys().map(|n| n.len()).max().unwrap_or(0);
        for (name, build) in &outcome {
            let n = build.transpiled.len() + build.copied.len();
            if changed.contains_key(name) {
                println!(
                    "{} {:<max_name$} {}",
                    "  Δ".yellow().bold(),
                    name.cyan(),
                    format!("{:>3} files", n).yellow(),
                );
            } else {
                println!(
                    "{} {:<max_name$} {}",
                    "  ∟".dimmed(),
                    name.dimmed(),
                    format!("{:>3} files", n).dimmed(),
                );
            }
        }

        for name in outcome.keys() {
            let uri = format!("spotify:app:rpc:reload?module={name}");
            if let Err(e) = opener::open(&uri) {
                tracing::warn!(%name, ?e, "failed to trigger reload");
            }
        }
        Ok(())
    }

    fn upsert_and_run(&self, module: &Module, paths: &HashSet<PathBuf>) -> Result<BuilderOutcome> {
        let mut node_ids = Vec::new();
        for path in paths {
            match module.builder.upsert_node(path) {
                Ok(Some(id)) => {
                    node_ids.push(id);
                }
                Ok(None) => {
                    let _ = util::remove_if_exists(&self.corresponding_output(module, path));
                }
                Err(err) => {
                    tracing::warn!(?err, path = %path.display(), "upsert node failed");
                }
            }
        }
        if node_ids.is_empty() {
            return Ok(BuilderOutcome::default());
        }
        module.builder.run_for(&node_ids)
    }

    fn corresponding_output(&self, module: &Module, source: &Path) -> PathBuf {
        let rel = source.strip_prefix(&module.dir).unwrap_or(source);
        let mut out = module.dir.join(rel);
        let kind = crate::build::SourceKind::from_path(source);
        match kind {
            crate::build::SourceKind::Js
            | crate::build::SourceKind::Ts
            | crate::build::SourceKind::Tsx
            | crate::build::SourceKind::Jsx
            | crate::build::SourceKind::Mjs
            | crate::build::SourceKind::Mts => {
                out.set_extension("js");
            }
            crate::build::SourceKind::Scss => {
                out.set_extension("css");
            }
            _ => {}
        }
        out
    }

    fn find_module(&self, name: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.name == name)
    }

    fn find_module_for_path(&self, path: &Path) -> Option<&Module> {
        self.modules.iter().find(|m| path.starts_with(&m.dir))
    }

    fn all_names_set(&self) -> HashSet<String> {
        self.modules.iter().map(|m| m.name.clone()).collect()
    }

    fn affected_closure(&self, seeds: &HashSet<String>) -> HashSet<String> {
        let mut closure = seeds.clone();
        let mut queue: VecDeque<String> = seeds.iter().cloned().collect();
        while let Some(current) = queue.pop_front() {
            if let Some(dependents) = self.rev_deps.get(&current) {
                for dep in dependents {
                    if closure.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }
        closure
    }

    fn topological_order_of(&self, subset: &HashSet<String>) -> Vec<String> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for name in subset {
            let dep_count = self
                .dep_graph
                .get(name)
                .map(|deps| deps.iter().filter(|d| subset.contains(*d)).count())
                .unwrap_or(0);
            in_degree.insert(name.clone(), dep_count);
        }

        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(m, _)| m.clone())
            .collect();
        let mut sorted = Vec::new();

        while let Some(name) = queue.pop_front() {
            sorted.push(name.clone());
            if let Some(dependents) = self.rev_deps.get(&name) {
                for dep in dependents {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            queue.push_back(dep.clone());
                        }
                    }
                }
            }
        }

        sorted
    }
}

struct PendingChanges {
    per_module: HashMap<String, HashSet<PathBuf>>,
}

impl PendingChanges {
    fn new() -> Self {
        Self {
            per_module: HashMap::new(),
        }
    }

    fn add(&mut self, module: String, path: PathBuf) {
        self.per_module.entry(module).or_default().insert(path);
    }

    fn is_empty(&self) -> bool {
        self.per_module.is_empty()
    }

    fn drain(&mut self) -> HashMap<String, HashSet<PathBuf>> {
        std::mem::take(&mut self.per_module)
    }
}

pub type BuildOutcomeMap = HashMap<String, BuilderOutcome>;
