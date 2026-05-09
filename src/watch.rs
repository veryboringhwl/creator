use crate::builder::{parse_file_type, BuildOpts, Builder, FileType};
use anyhow::{Context, Result};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

pub fn watch(builder: &Builder, debounce_ms: i64) -> Result<()> {
    println!("Watching for changes...");

    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default(),
    )
    .context("Failed to initialize file watcher")?;

    watcher
        .watch(&builder.input_dir, RecursiveMode::Recursive)
        .with_context(|| {
            format!(
                "Failed to watch input directory: {}",
                builder.input_dir.display()
            )
        })?;

    let debounce_ms = if debounce_ms < 0 { 0 } else { debounce_ms as u64 };
    let debounce_duration = Duration::from_millis(debounce_ms);

    let mut pending = BuildOpts::default();

    loop {
        let event = rx.recv().context("File watcher channel closed")??;
        apply_event(&event, &mut pending, builder)?;

        let mut deadline = Instant::now() + debounce_duration;
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(event) => {
                    let event = event?;
                    apply_event(&event, &mut pending, builder)?;
                    deadline = Instant::now() + debounce_duration;
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(anyhow::anyhow!("File watcher channel disconnected"));
                }
            }
        }

        if pending.js || pending.css || pending.unknown {
            println!("Building...");

            let opts = pending;
            pending = BuildOpts::default();

            match builder.build(opts) {
                Ok(()) => {
                    if (opts.js || opts.css)
                        && let Err(err) = reload_module(builder.identifier.as_str())
                    {
                        eprintln!("Failed to trigger spotify reload: {err:#}");
                    }
                }
                Err(err) => {
                    eprintln!("Build failed: {err:#}");
                }
            }
        }
    }
}

fn apply_event(event: &notify::Event, opts: &mut BuildOpts, builder: &Builder) -> Result<()> {
    if !matches!(
        event.kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    ) {
        return Ok(());
    }

    let include_unknown = builder.input_dir != builder.output_dir;
    let is_remove = matches!(event.kind, EventKind::Remove(_));

    for path in &event.paths {
        if should_ignore_path(path, builder) {
            continue;
        }

        let rel = path.strip_prefix(&builder.input_dir).unwrap_or(path);
        match parse_file_type(path, rel) {
            FileType::ToJS => {
                opts.js = true;
                println!("Building {}...", path.display());
            }
            FileType::ToCSS => {
                opts.css = true;
                println!("Building {}...", path.display());
            }
            FileType::UNKNOWN if include_unknown => {
                if is_remove {
                    remove_unknown_output(builder, rel)?;
                } else {
                    opts.unknown = true;
                }
            }
            _ => continue,
        }
    }

    Ok(())
}

fn should_ignore_path(path: &Path, builder: &Builder) -> bool {
    if builder.input_dir == builder.output_dir {
        return false;
    }

    path.strip_prefix(&builder.output_dir).is_ok()
}

fn remove_unknown_output(builder: &Builder, rel: &Path) -> Result<()> {
    let output = builder.get_output_path(rel);

    match fs::remove_file(&output) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::IsADirectory => {
            match fs::remove_dir_all(&output) {
                Ok(()) => Ok(()),
                Err(dir_err) if dir_err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(dir_err) => Err(dir_err).with_context(|| {
                    format!("Failed to remove output path: {}", output.display())
                }),
            }
        }
        Err(err) => {
            Err(err).with_context(|| format!("Failed to remove output file: {}", output.display()))
        }
    }
}

fn reload_module(module: &str) -> Result<()> {
    let url = if module.is_empty() {
        "spotify:app:rpc:reload".to_string()
    } else {
        format!("spotify:app:rpc:reload?module={module}")
    };
    open::that(url).context("Failed to trigger spotify reload")?;
    Ok(())
}
