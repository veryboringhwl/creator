use std::{
    collections::HashMap, fs, path::{Path, PathBuf}, process::Command, time::{SystemTime, UNIX_EPOCH}
};

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use serde_json;
use swc_core::{
    common::{FileName, GLOBALS, Globals, Mark, SourceMap, sync::Lrc}, ecma::{
        ast::{
            CallExpr, Callee, ExportAll, Expr, ExprOrSpread, ImportDecl, Lit, NamedExport, Pass, Program, Str
        }, codegen::{Config as CodegenConfig, Emitter, Node, text_writer::JsWriter}, parser::{Parser, StringInput, Syntax, TsSyntax}, transforms::{
            base::{fixer::fixer, resolver}, react::{Options as ReactOptions, Runtime, react}, typescript::strip
        }, visit::{VisitMut, VisitMutWith}
    }
};

use crate::{
    classmap::{Mapping, reformat_mapping_for_css}, timestamp::TimestampResolver, util::{ensure_parent, normalize_slashes}
};

pub struct Transpiler {
    classmap: Mapping,
    css_mapping: Mapping,
    pub dev: bool,
    timestamp_resolver: Option<TimestampResolver>,
}

impl Transpiler {
    pub fn new(classmap: Mapping, dev: bool) -> Self {
        let mut js_root = HashMap::new();
        js_root.insert("MAP".to_string(), classmap.clone());

        let mut css_root = HashMap::new();
        css_root.insert("MAP".to_string(), reformat_mapping_for_css(&classmap));

        Self {
            css_mapping: Mapping::Map(css_root),
            classmap: Mapping::Map(js_root),
            dev,
            timestamp_resolver: TimestampResolver::new(),
        }
    }

    pub fn js(
        &self,
        input: &Path,
        output: &Path,
        base_dir: &Path,
        filepath: &str,
        timestamp: u64,
    ) -> Result<()> {
        let source = fs::read_to_string(input)
            .with_context(|| format!("Failed to read input file: {}", input.display()))?;

        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(FileName::Real(input.to_path_buf()).into(), source);

        let globals = Globals::new();
        let output_code = GLOBALS.set(&globals, || -> Result<String> {
            let mut parser = Parser::new(
                Syntax::Typescript(TsSyntax {
                    tsx: true,
                    decorators: true,
                    dts: false,
                    ..Default::default()
                }),
                StringInput::from(&*fm),
                None,
            );

            let mut program = Program::Module(
                parser
                    .parse_module()
                    .map_err(|err| anyhow!("Failed to parse {}: {err:?}", input.display()))?,
            );

            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            {
                let mut pass = resolver(unresolved_mark, top_level_mark, false);
                pass.process(&mut program);
            }
            let mut rewriter = ModuleSpecifierRewriter::new(
                build_rules(timestamp, self.dev)?,
                self.dev,
                self.timestamp_resolver.as_ref(),
            );
            program.visit_mut_with(&mut rewriter);

            {
                let mut pass = strip(unresolved_mark, top_level_mark);
                pass.process(&mut program);
            }
            {
                let mut pass = react::<swc_core::common::comments::NoopComments>(
                    cm.clone(),
                    None,
                    ReactOptions {
                        runtime: Some(Runtime::Automatic),
                        import_source: Some("/modules/stdlib/src/expose".into()),
                        ..Default::default()
                    },
                    top_level_mark,
                    unresolved_mark,
                );
                pass.process(&mut program);
            }
            {
                let mut pass = fixer(None);
                pass.process(&mut program);
            }

            let mut buf = Vec::new();
            {
                let mut emitter = Emitter {
                    cfg: CodegenConfig::default().with_minify(false),
                    comments: None,
                    cm: cm.clone(),
                    wr: JsWriter::new(cm.clone(), "\n", &mut buf, None),
                };
                program.emit_with(&mut emitter)?;
            }

            let mut code = String::from_utf8(buf)
                .map_err(|err| anyhow!("Failed to encode output for {}: {err}", input.display()))?;

            code = code
                .replace(
                    "from \"/modules/stdlib/src/expose/jsx-runtime\"",
                    "from \"/modules/stdlib/src/expose/jsx-runtime.js\"",
                )
                .replace(
                    "from \"react/jsx-runtime\"",
                    "from \"/modules/stdlib/src/expose/jsx-runtime.js\"",
                );

            code = remap_js_classmap_expressions(&code, &self.classmap)?;

            Ok(code)
        })?;

        let _ = base_dir;
        let _ = filepath;
        ensure_parent(output)?;
        fs::write(output, output_code)
            .with_context(|| format!("Failed to write output file: {}", output.display()))?;
        Ok(())
    }

    pub fn css(&self, input: &Path, output: &Path, files: &[PathBuf]) -> Result<()> {
        let mut css = grass::from_path(
            input,
            &grass::Options::default().style(grass::OutputStyle::Expanded),
        )
        .with_context(|| format!("Failed to compile scss: {}", input.display()))?;

        if css.contains("@tailwind") {
            css = run_tailwind(&css, files)?;
        }

        css = autoprefix_css(&css)?;
        css = remap_css_selectors(&css, &self.css_mapping)?;
        css = remap_css_classmap_expressions(&css, &self.css_mapping)?;

        ensure_parent(output)?;
        fs::write(output, css)
            .with_context(|| format!("Failed to write css file: {}", output.display()))?;
        Ok(())
    }
}

struct RegexRule {
    regex: Regex,
    replacement: String,
}

fn build_rules(timestamp: u64, dev: bool) -> Result<Vec<RegexRule>> {
    let mut rules = vec![
        RegexRule {
            regex: Regex::new(r"\.js(\?.*)?$")?,
            replacement: ".js$1".to_string(),
        },
        RegexRule {
            regex: Regex::new(r"\.ts(\?.*)?$")?,
            replacement: ".js$1".to_string(),
        },
        RegexRule {
            regex: Regex::new(r"\.mjs(\?.*)?$")?,
            replacement: ".js$1".to_string(),
        },
        RegexRule {
            regex: Regex::new(r"\.mts(\?.*)?$")?,
            replacement: ".js$1".to_string(),
        },
        RegexRule {
            regex: Regex::new(r"\.jsx(\?.*)?$")?,
            replacement: ".js$1".to_string(),
        },
        RegexRule {
            regex: Regex::new(r"\.tsx(\?.*)?$")?,
            replacement: ".js$1".to_string(),
        },
    ];

    if dev {
        rules.push(RegexRule {
            regex: Regex::new(r"^(\.?\.\/.*)$")?,
            replacement: format!("$1?t={timestamp}"),
        });
    }

    Ok(rules)
}

struct ModuleSpecifierRewriter<'a> {
    rules: Vec<RegexRule>,
    dev: bool,
    timestamp_resolver: Option<&'a TimestampResolver>,
}

impl<'a> ModuleSpecifierRewriter<'a> {
    fn new(
        rules: Vec<RegexRule>,
        dev: bool,
        timestamp_resolver: Option<&'a TimestampResolver>,
    ) -> Self {
        Self {
            rules,
            dev,
            timestamp_resolver,
        }
    }

    fn rewrite_import_specifier(&self, specifier: &str) -> Result<Option<String>> {
        if specifier.starts_with("http://") || specifier.starts_with("https://") {
            return Ok(None);
        }

        let mut rewritten = specifier.to_string();
        for rule in &self.rules {
            rewritten = rule
                .regex
                .replace_all(&rewritten, rule.replacement.as_str())
                .into_owned();
        }

        if self.dev && rewritten.starts_with("/modules/") {
            let import_path = trim_query_and_fragment(&rewritten);
            if let Some(resolver) = self.timestamp_resolver {
                if let Some(ts) = resolver.resolve(import_path)? {
                    rewritten = format!("{import_path}?t={ts}");
                }
            }
        }

        if rewritten == specifier {
            Ok(None)
        } else {
            Ok(Some(rewritten))
        }
    }
}

fn trim_query_and_fragment(specifier: &str) -> &str {
    let query_index = specifier.find('?').unwrap_or(specifier.len());
    let fragment_index = specifier.find('#').unwrap_or(specifier.len());
    let end = query_index.min(fragment_index);
    &specifier[..end]
}

impl VisitMut for ModuleSpecifierRewriter<'_> {
    fn visit_mut_import_decl(&mut self, n: &mut ImportDecl) {
        n.visit_mut_children_with(self);
        let specifier = n.src.value.as_str().map_or_else(
            || n.src.value.to_string_lossy().into_owned(),
            ToString::to_string,
        );
        if let Ok(Some(remapped)) = self.rewrite_import_specifier(&specifier) {
            n.src = Box::new(remapped.into());
        }
    }

    fn visit_mut_named_export(&mut self, n: &mut NamedExport) {
        n.visit_mut_children_with(self);
        if let Some(src) = &n.src {
            let specifier = src.value.as_str().map_or_else(
                || src.value.to_string_lossy().into_owned(),
                ToString::to_string,
            );
            if let Ok(Some(remapped)) = self.rewrite_import_specifier(&specifier) {
                n.src = Some(Box::new(remapped.into()));
            }
        }
    }

    fn visit_mut_export_all(&mut self, n: &mut ExportAll) {
        n.visit_mut_children_with(self);
        let specifier = n.src.value.as_str().map_or_else(
            || n.src.value.to_string_lossy().into_owned(),
            ToString::to_string,
        );
        if let Ok(Some(remapped)) = self.rewrite_import_specifier(&specifier) {
            n.src = Box::new(remapped.into());
        }
    }

    fn visit_mut_call_expr(&mut self, n: &mut CallExpr) {
        n.visit_mut_children_with(self);
        if let Callee::Import(_) = n.callee {
            if let Some(arg) = n.args.first() {
                if let Expr::Lit(Lit::Str(lit_str)) = *arg.expr.clone() {
                    let specifier = lit_str.value.as_str().map_or_else(
                        || lit_str.value.to_string_lossy().into_owned(),
                        ToString::to_string,
                    );
                    if let Ok(Some(remapped)) = self.rewrite_import_specifier(&specifier) {
                        let replacer = Expr::Lit(Lit::Str(Str {
                            span: lit_str.span,
                            value: remapped.into(),
                            raw: None,
                        }));
                        n.args[0] = ExprOrSpread {
                            spread: None,
                            expr: Box::new(replacer),
                        };
                    }
                }
            }
        }
    }
}

fn remap_css_selectors(css: &str, mapping: &Mapping) -> Result<String> {
    let mut out = String::with_capacity(css.len());
    let bytes = css.as_bytes();
    let mut i = 0;
    let mut cursor = 0;
    let mut in_comment = false;
    let mut in_string: Option<u8> = None;
    let mut stack: Vec<bool> = Vec::new();

    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();

        if in_comment {
            if c == b'*' && next == Some(b'/') {
                in_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if let Some(quote) = in_string {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if c == b'/' && next == Some(b'*') {
            in_comment = true;
            i += 2;
            continue;
        }

        if c == b'\'' || c == b'"' {
            in_string = Some(c);
            i += 1;
            continue;
        }

        if c == b'{' {
            let prelude = &css[cursor..i];
            let trimmed = prelude.trim_start();
            let inside_keyframes = stack.iter().any(|keyframes| *keyframes);
            if trimmed.starts_with('@') {
                out.push_str(prelude);
                out.push('{');
                stack.push(is_keyframes_prelude(trimmed));
            } else if inside_keyframes {
                out.push_str(prelude);
                out.push('{');
                stack.push(false);
            } else {
                out.push_str(&remap_selector_segment(prelude, mapping)?);
                out.push('{');
                stack.push(false);
            }
            i += 1;
            cursor = i;
            continue;
        }

        if c == b'}' {
            out.push_str(&css[cursor..i]);
            out.push('}');
            if !stack.is_empty() {
                stack.pop();
            }
            i += 1;
            cursor = i;
            continue;
        }

        if c == b';' {
            out.push_str(&css[cursor..i]);
            out.push(';');
            i += 1;
            cursor = i;
            continue;
        }

        i += 1;
    }

    if cursor < css.len() {
        out.push_str(&css[cursor..]);
    }

    Ok(out)
}

fn is_keyframes_prelude(prelude: &str) -> bool {
    let prelude = prelude.trim_start();
    let lower = prelude.to_ascii_lowercase();
    lower.starts_with("@keyframes")
        || lower.starts_with("@-webkit-keyframes")
        || lower.starts_with("@-moz-keyframes")
        || lower.starts_with("@-ms-keyframes")
}

fn remap_selector_segment(segment: &str, mapping: &Mapping) -> Result<String> {
    let mut out = String::with_capacity(segment.len());
    let bytes = segment.as_bytes();
    let mut i = 0;
    let mut in_string: Option<u8> = None;
    let mut attr_depth = 0;

    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_string {
            if c == b'\\' {
                out.push(c as char);
                if let Some(next) = bytes.get(i + 1) {
                    out.push(*next as char);
                }
                i += 2;
                continue;
            }
            out.push(c as char);
            if c == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if c == b'\'' || c == b'"' {
            in_string = Some(c);
            out.push(c as char);
            i += 1;
            continue;
        }

        if c == b'[' {
            attr_depth += 1;
            out.push('[');
            i += 1;
            continue;
        }

        if c == b']' {
            if attr_depth > 0 {
                attr_depth -= 1;
            }
            out.push(']');
            i += 1;
            continue;
        }

        if c == b'.' && attr_depth == 0 {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() {
                let ch = bytes[end] as char;
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    end += 1;
                } else {
                    break;
                }
            }

            if end == start {
                out.push('.');
                i += 1;
                continue;
            }

            let name = &segment[start..end];
            let remapped = remap_class_name(name, mapping)?;
            out.push('.');
            out.push_str(remapped.as_deref().unwrap_or(name));
            i = end;
            continue;
        }

        out.push(c as char);
        i += 1;
    }

    Ok(out)
}

fn remap_class_name(name: &str, mapping: &Mapping) -> Result<Option<String>> {
    let idents: Vec<&str> = name.split("__").collect();
    let mut current = mapping;
    let mut last_ident = 0;

    for ident in &idents {
        match current {
            Mapping::Map(map) => match map.get(*ident) {
                Some(next) => {
                    current = next;
                    last_ident += 1;
                }
                None => break,
            },
            _ => break,
        }
    }

    if last_ident == 0 {
        return Ok(None);
    }

    if last_ident != idents.len() {
        let problematic = idents[..=last_ident].join("__");
        return Err(anyhow!(
            "{} isn't a node of the provided mapping",
            problematic
        ));
    }

    match current {
        Mapping::Str(value) => Ok(Some(value.clone())),
        _ => Err(anyhow!(
            "{} isn't an ending node (leaf) of the provided mapping",
            name
        )),
    }
}

fn autoprefix_css(css: &str) -> Result<String> {
    let stylesheet = lightningcss::stylesheet::StyleSheet::parse(
        css,
        lightningcss::stylesheet::ParserOptions::default(),
    )
    .map_err(|err| anyhow!("Failed to parse CSS: {err}"))?;

    let result = stylesheet
        .to_css(lightningcss::stylesheet::PrinterOptions {
            minify: false,
            targets: lightningcss::targets::Targets::default(),
            ..Default::default()
        })
        .map_err(|err| anyhow!("Failed to serialize CSS: {err}"))?;

    Ok(result.code)
}

fn run_tailwind(css: &str, files: &[PathBuf]) -> Result<String> {
    let bin = std::env::var("TAILWINDCSS_BIN").unwrap_or_else(|_| "tailwindcss".to_string());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let tmp_dir = std::env::temp_dir();
    let input_path = tmp_dir.join(format!("spicetify-tailwind-{stamp}-input.css"));
    let output_path = tmp_dir.join(format!("spicetify-tailwind-{stamp}-output.css"));

    fs::write(&input_path, css)
        .with_context(|| format!("Failed to write temp css: {}", input_path.display()))?;

    let mut cmd = Command::new(&bin);
    cmd.arg("--input")
        .arg(&input_path)
        .arg("--output")
        .arg(&output_path);

    if !files.is_empty() {
        let content = files
            .iter()
            .map(|path| normalize_slashes(path))
            .collect::<Vec<_>>()
            .join(",");
        cmd.arg("--content").arg(content);
    }

    let status = cmd.status().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow!("tailwindcss CLI not found. Set TAILWINDCSS_BIN or install tailwindcss")
        } else {
            anyhow!("Failed to run tailwindcss: {err}")
        }
    })?;

    if !status.success() {
        return Err(anyhow!("tailwindcss failed with status: {status}"));
    }

    let output_css = fs::read_to_string(&output_path)
        .with_context(|| format!("Failed to read temp css: {}", output_path.display()))?;

    let _ = fs::remove_file(&input_path);
    let _ = fs::remove_file(&output_path);

    Ok(output_css)
}

fn lookup_mapping_path(mapping: &Mapping, path: &[&str], sep: &str) -> Result<Option<String>> {
    let mut current = mapping;
    let mut last_ident = 0;

    for ident in path {
        match current {
            Mapping::Map(map) => match map.get(*ident) {
                Some(next) => {
                    current = next;
                    last_ident += 1;
                }
                None => break,
            },
            _ => break,
        }
    }

    if last_ident == 0 {
        return Ok(None);
    }

    if last_ident != path.len() {
        let problematic = path[..=last_ident].join(sep);
        return Err(anyhow!(
            "{} isn't a node of the provided mapping",
            problematic
        ));
    }

    match current {
        Mapping::Str(value) => Ok(Some(value.clone())),
        _ => Err(anyhow!(
            "{} isn't an ending node (leaf) of the provided mapping",
            path.join(sep)
        )),
    }
}

fn remap_js_classmap_expressions(code: &str, mapping: &Mapping) -> Result<String> {
    let re = Regex::new(r"\bMAP(?:\.[A-Za-z_][A-Za-z0-9_]*)+")?;
    let mut out = String::with_capacity(code.len());
    let mut cursor = 0;

    for m in re.find_iter(code) {
        out.push_str(&code[cursor..m.start()]);
        let matched = m.as_str();
        let path: Vec<&str> = matched.split('.').collect();
        if let Some(value) = lookup_mapping_path(mapping, &path, ".")? {
            out.push_str(&serde_json::to_string(&value)?);
        } else {
            out.push_str(matched);
        }
        cursor = m.end();
    }

    out.push_str(&code[cursor..]);
    Ok(out)
}

fn remap_css_classmap_expressions(css: &str, mapping: &Mapping) -> Result<String> {
    let re = Regex::new(r"\.MAP(?:__[A-Za-z0-9_-]+)+")?;
    let mut out = String::with_capacity(css.len());
    let mut cursor = 0;

    for m in re.find_iter(css) {
        out.push_str(&css[cursor..m.start()]);
        let matched = m.as_str();
        let path_text = &matched[1..];
        let path: Vec<&str> = path_text.split("__").collect();
        if let Some(value) = lookup_mapping_path(mapping, &path, "__")? {
            out.push('.');
            out.push_str(&value);
        } else {
            out.push_str(matched);
        }
        cursor = m.end();
    }

    out.push_str(&css[cursor..]);
    Ok(out)
}
