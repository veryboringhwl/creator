use std::path::PathBuf;

use crate::core::{Error, Result, classmap, util};

#[derive(Debug, Clone)]
pub struct FetchOpts {
    pub url: String,
    pub output: PathBuf,
    pub modules_dir: PathBuf,
}

pub fn run(opts: FetchOpts) -> Result<()> {
    let info = classmap::fetch_classmap_info(&opts.url)?;
    let json = serde_json::to_string_pretty(&info.mapping)
        .map_err(|e| Error::Scaffold(format!("serialize classmap: {e}")))?;
    util::write_text(&opts.output, &json)?;
    println!("Saved classmap to {}", opts.output.display());

    let dts = classmap::gen_classmap_dts(&info.mapping);
    let modules = classmap::discover_module_dirs(&opts.modules_dir)?;
    for module in modules {
        let dts_path = module.join("classmap.d.ts");
        util::write_text(&dts_path, &dts)?;
    }
    Ok(())
}
