use std::path::PathBuf;

use crate::core::{Error, Result, util};
use crate::release::{self, ReleaseSpec};

#[derive(Debug, Clone)]
pub struct ReleaseOpts {
    pub inputs: Vec<PathBuf>,
    pub classmap_url: String,
    pub output_dir: PathBuf,
    pub dev: bool,
}

pub fn run(opts: ReleaseOpts) -> Result<()> {
    let manifest = release::run(ReleaseSpec {
        inputs: opts.inputs,
        classmap_url: opts.classmap_url,
        output_dir: opts.output_dir,
        dev: opts.dev,
    })?;

    for entry in &manifest.artifacts {
        println!("  {}", entry.zip);
    }
    for entry in &manifest.failed {
        eprintln!("  FAILED {}: {}", entry.dir, entry.error);
    }
    let manifest_path = std::path::Path::new("dist").join("release-manifest.json");
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| Error::Scaffold(format!("serialize manifest: {e}")))?;
    util::write_text(&manifest_path, &manifest_json)?;

    println!(
        "\n{} artifacts, {} failed — manifest written to {}",
        manifest.artifacts.len(),
        manifest.failed.len(),
        manifest_path.display()
    );
    Ok(())
}
