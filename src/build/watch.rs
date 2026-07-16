use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::build::{Builder, NodeId, SourceKind};
use crate::core::{Error, Result, remove_if_exists};

pub struct WatchOptions {
    pub debounce: Duration,
    pub module: Option<String>,
}

pub fn watch(builder: &Builder, options: WatchOptions) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default(),
    )
    .map_err(|e| Error::Watch(e.to_string()))?;

    watcher
        .watch(builder.source_dir(), RecursiveMode::Recursive)
        .map_err(|e| Error::Watch(format!("watch {}: {e}", builder.source_dir().display())))?;

    let mut affected: HashSet<NodeId> = HashSet::new();
    let module = options
        .module
        .unwrap_or_else(|| builder.plan().metadata.name.clone());

    loop {
        let event = match rx.recv() {
            Ok(ev) => ev,
            Err(_) => return Err(Error::Watch("file watcher channel closed".into())),
        };
        let event = event.map_err(|e| Error::Watch(e.to_string()))?;
        apply_event(&event, builder, &mut affected);

        if let Err(err) = run_affected(builder, &mut affected) {
            tracing::error!(?err, "watch build failed");
        } else {
            reload_module(&module);
        }

        let mut deadline = Instant::now() + options.debounce;
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(event) => {
                    let event = event.map_err(|e| Error::Watch(e.to_string()))?;
                    apply_event(&event, builder, &mut affected);
                    deadline = Instant::now() + options.debounce;
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Watch("file watcher disconnected".into()));
                }
            }
        }

        if let Err(err) = run_affected(builder, &mut affected) {
            tracing::error!(?err, "watch build failed");
        } else {
            reload_module(&module);
        }
    }
}

fn apply_event(event: &notify::Event, builder: &Builder, affected: &mut HashSet<NodeId>) {
    if !matches!(
        event.kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    ) {
        return;
    }

    for path in &event.paths {
        if should_ignore_path(path, builder) {
            continue;
        }
        if let Some(id) = match builder.upsert_node(path) {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!(?err, path = %path.display(), "watch event apply failed");
                continue;
            }
        } {
            affected.insert(id);
        }
    }
}

fn should_ignore_path(path: &Path, builder: &Builder) -> bool {
    if builder.source_dir() == builder.output_dir() {
        return false;
    }
    path.strip_prefix(builder.output_dir()).is_ok()
}

fn run_affected(builder: &Builder, affected: &mut HashSet<NodeId>) -> Result<()> {
    if affected.is_empty() {
        return Ok(());
    }
    let graph = builder.graph();
    {
        let graph = graph.read();
        for id in &*affected {
            if let Ok(node) = graph.node(*id)
                && !node.path.exists()
            {
                let output = corresponding_output(builder, &node.path);
                if output.exists() {
                    let _ = remove_if_exists(&output);
                }
            }
        }
    }
    let node_ids: Vec<NodeId> = affected.drain().collect();
    let outcome = builder.run_for(&node_ids)?;
    tracing::info!(?outcome, "watch build done");
    Ok(())
}

fn corresponding_output(builder: &Builder, source: &Path) -> PathBuf {
    let rel = source.strip_prefix(builder.source_dir()).unwrap_or(source);
    let mut out = builder.output_dir().join(rel);
    match SourceKind::from_path(source) {
        SourceKind::Js
        | SourceKind::Ts
        | SourceKind::Tsx
        | SourceKind::Jsx
        | SourceKind::Mjs
        | SourceKind::Mts => {
            out.set_extension("js");
        }
        SourceKind::Scss => {
            out.set_extension("css");
        }
        _ => {}
    }
    out
}

fn reload_module(name: &str) {
    let uri = format!("spotify:app:rpc:reload?module={name}");
    match opener::open(&uri) {
        Ok(()) => tracing::info!(%name, "reload triggered"),
        Err(e) => tracing::warn!(%name, ?e, "failed to trigger reload"),
    }
}
