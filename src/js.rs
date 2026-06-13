use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use oxc::allocator::Allocator;
use oxc::ast::ast::{ExportAllDeclaration, ExportNamedDeclaration, Expression, ImportDeclaration};
use oxc::ast_visit::{VisitMut, walk_mut};
use oxc::codegen::{Codegen, CodegenOptions};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{JsxOptions, JsxRuntime, TransformOptions, Transformer, TypeScriptOptions};
use regex::Regex;

use crate::classmap::Mapping;
use crate::timestamp::TimestampResolver;
use crate::util::ensure_parent;

pub struct JsTranspiler {
    classmap: Mapping,
    ext_rules: Vec<RegexRule>,
    timestamp_resolver: Option<TimestampResolver>,
    sourcemap: bool,
    dev: bool,
}

struct RegexRule {
    regex: Regex,
    replacement: String,
}

impl JsTranspiler {
    pub fn new(classmap: Mapping, sourcemap: bool, dev: bool) -> Self {
        let timestamp_resolver = TimestampResolver::new();
        let ext_rules = vec![
            RegexRule {
                regex: Regex::new(r"\.tsx?(\?.*)?$").unwrap(),
                replacement: ".js$1".to_string(),
            },
            RegexRule {
                regex: Regex::new(r"\.mjs(\?.*)?$").unwrap(),
                replacement: ".js$1".to_string(),
            },
            RegexRule {
                regex: Regex::new(r"\.mts(\?.*)?$").unwrap(),
                replacement: ".js$1".to_string(),
            },
            RegexRule {
                regex: Regex::new(r"\.jsx(\?.*)?$").unwrap(),
                replacement: ".js$1".to_string(),
            },
        ];
        Self {
            classmap,
            ext_rules,
            timestamp_resolver,
            sourcemap,
            dev,
        }
    }

    pub fn transpile(
        &self,
        input: &Path,
        output: &Path,
        _filepath: &str,
        timestamp: u64,
    ) -> Result<()> {
        let source = fs::read_to_string(input)
            .with_context(|| format!("Failed to read input file: {}", input.display()))?;

        let is_plain_js = input.extension().is_some_and(|e| e == "js");
        if is_plain_js {
            if self.dev {
                let allocator = Allocator::default();
                let source_type = SourceType::mjs();
                let parser_ret = Parser::new(&allocator, &source, source_type).parse();

                if !parser_ret.errors.is_empty() {
                    return Err(anyhow!(
                        "Failed to parse {}: {:?}",
                        input.display(),
                        parser_ret.errors
                    ));
                }

                let mut program = parser_ret.program;
                let mut ts_rewriter = TimestampRewriter::new(
                    &allocator,
                    timestamp,
                    self.dev,
                    self.timestamp_resolver.as_ref(),
                );
                ts_rewriter.visit_program(&mut program);

                let codegen_ret = Codegen::new()
                    .with_options(CodegenOptions::default())
                    .build(&program);

                let code = remap_js_classmap_expressions(&codegen_ret.code, &self.classmap)?;
                ensure_parent(output)?;
                fs::write(output, code).with_context(|| {
                    format!("Failed to write output file: {}", output.display())
                })?;
            } else {
                let code = remap_js_classmap_expressions(&source, &self.classmap)?;
                ensure_parent(output)?;
                fs::write(output, code).with_context(|| {
                    format!("Failed to write output file: {}", output.display())
                })?;
            }
            return Ok(());
        }

        let allocator = Allocator::default();

        let source_type = SourceType::tsx();
        let parser_ret = Parser::new(&allocator, &source, source_type).parse();

        if !parser_ret.errors.is_empty() {
            return Err(anyhow!(
                "Failed to parse {}: {:?}",
                input.display(),
                parser_ret.errors
            ));
        }

        let mut program = parser_ret.program;

        let semantic_ret = SemanticBuilder::new()
            .with_check_syntax_error(true)
            .build(&program);

        if !semantic_ret.errors.is_empty() {
            return Err(anyhow!(
                "Semantic analysis failed for {}: {:?}",
                input.display(),
                semantic_ret.errors
            ));
        }

        let scoping = semantic_ret.semantic.into_scoping();

        let mut ext_rewriter = ExtensionRewriter::new(&allocator, &self.ext_rules);
        ext_rewriter.visit_program(&mut program);

        let transform_options = TransformOptions {
            typescript: TypeScriptOptions {
                rewrite_import_extensions: None,
                ..TypeScriptOptions::default()
            },
            jsx: JsxOptions {
                runtime: JsxRuntime::Automatic,
                import_source: Some("/modules/stdlib/src/expose".into()),
                ..JsxOptions::default()
            },
            ..TransformOptions::default()
        };

        let ret = Transformer::new(&allocator, input, &transform_options)
            .build_with_scoping(scoping, &mut program);

        if !ret.errors.is_empty() {
            return Err(anyhow!(
                "Transform failed for {}: {:?}",
                input.display(),
                ret.errors
            ));
        }

        let mut ts_rewriter = TimestampRewriter::new(
            &allocator,
            timestamp,
            self.dev,
            self.timestamp_resolver.as_ref(),
        );
        ts_rewriter.visit_program(&mut program);

        let codegen_opts = if self.sourcemap {
            CodegenOptions {
                source_map_path: Some(input.to_path_buf()),
                ..CodegenOptions::default()
            }
        } else {
            CodegenOptions::default()
        };

        let codegen_ret = Codegen::new().with_options(codegen_opts).build(&program);

        let mut code = codegen_ret.code;

        if let Some(map) = codegen_ret.map {
            let map_path = input.with_extension("js.map");
            let _ = fs::remove_file(&map_path);

            let j = map.to_json();
            let value = serde_json::json!({
                "version": j.version,
                "file": j.file,
                "sourceRoot": j.source_root,
                "sources": j.sources,
                "sourcesContent": j.sources_content,
                "names": j.names,
                "mappings": j.mappings,
            });
            let json = serde_json::to_string(&value).map_err(|e| {
                anyhow!(
                    "Failed to serialize source map for {}: {e}",
                    input.display()
                )
            })?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
            code.push_str("\n//# sourceMappingURL=data:application/json;charset=utf-8;base64,");
            code.push_str(&encoded);
            code.push('\n');
        }

        let code = remap_js_classmap_expressions(&code, &self.classmap)?;

        ensure_parent(output)?;
        fs::write(output, code)
            .with_context(|| format!("Failed to write output file: {}", output.display()))?;
        Ok(())
    }
}

fn rewrite_specifier(specifier: &str, rules: &[RegexRule]) -> Option<String> {
    if specifier.starts_with("http://") || specifier.starts_with("https://") {
        return None;
    }

    let mut rewritten = specifier.to_string();
    for rule in rules {
        rewritten = rule
            .regex
            .replace_all(&rewritten, rule.replacement.as_str())
            .into_owned();
    }

    if rewritten == specifier {
        None
    } else {
        Some(rewritten)
    }
}

struct ExtensionRewriter<'a> {
    allocator: &'a Allocator,
    rules: &'a [RegexRule],
}

impl<'a> ExtensionRewriter<'a> {
    fn new(allocator: &'a Allocator, rules: &'a [RegexRule]) -> Self {
        Self { allocator, rules }
    }
}

impl<'a> VisitMut<'a> for ExtensionRewriter<'a> {
    fn visit_import_declaration(&mut self, it: &mut ImportDeclaration<'a>) {
        let specifier = it.source.value.as_str();
        if let Some(remapped) = rewrite_specifier(specifier, self.rules) {
            it.source.value = self.allocator.alloc_str(&remapped).into();
        }
        walk_mut::walk_import_declaration(self, it);
    }

    fn visit_export_named_declaration(&mut self, it: &mut ExportNamedDeclaration<'a>) {
        if let Some(ref mut source) = it.source {
            let specifier = source.value.as_str();
            if let Some(remapped) = rewrite_specifier(specifier, self.rules) {
                source.value = self.allocator.alloc_str(&remapped).into();
            }
        }
        walk_mut::walk_export_named_declaration(self, it);
    }

    fn visit_export_all_declaration(&mut self, it: &mut ExportAllDeclaration<'a>) {
        let specifier = it.source.value.as_str();
        if let Some(remapped) = rewrite_specifier(specifier, self.rules) {
            it.source.value = self.allocator.alloc_str(&remapped).into();
        }
        walk_mut::walk_export_all_declaration(self, it);
    }

    fn visit_expression(&mut self, it: &mut Expression<'a>) {
        if let Expression::ImportExpression(import_expr) = it
            && let Expression::StringLiteral(s) = &mut import_expr.source
        {
            let specifier = s.value.as_str();
            if let Some(remapped) = rewrite_specifier(specifier, self.rules) {
                s.value = self.allocator.alloc_str(&remapped).into();
            }
        }
        walk_mut::walk_expression(self, it);
    }
}
struct TimestampRewriter<'a> {
    allocator: &'a Allocator,
    timestamp: u64,
    dev: bool,
    timestamp_resolver: Option<&'a TimestampResolver>,
}

impl<'a> TimestampRewriter<'a> {
    fn new(
        allocator: &'a Allocator,
        timestamp: u64,
        dev: bool,
        timestamp_resolver: Option<&'a TimestampResolver>,
    ) -> Self {
        Self {
            allocator,
            timestamp,
            dev,
            timestamp_resolver,
        }
    }

    fn rewrite(&self, specifier: &str) -> Option<String> {
        if specifier.starts_with("http://") || specifier.starts_with("https://") {
            return None;
        }

        if self.dev {
            if specifier.starts_with("./") || specifier.starts_with("../") {
                return Some(format!("{specifier}?t={}", self.timestamp));
            }

            if specifier.starts_with("/modules/") {
                let import_path = trim_query_and_fragment(specifier);
                if let Some(resolver) = self.timestamp_resolver
                    && let Some(ts) = resolver.resolve(import_path).unwrap_or(None)
                {
                    return Some(format!("{import_path}?t={ts}"));
                }
            }
        }

        if specifier == "/modules/stdlib/src/expose/jsx-runtime" || specifier == "react/jsx-runtime"
        {
            return Some("/modules/stdlib/src/expose/jsx-runtime.js".to_string());
        }

        None
    }
}

fn trim_query_and_fragment(specifier: &str) -> &str {
    let query_index = specifier.find('?').unwrap_or(specifier.len());
    let fragment_index = specifier.find('#').unwrap_or(specifier.len());
    let end = query_index.min(fragment_index);
    &specifier[..end]
}

impl<'a> VisitMut<'a> for TimestampRewriter<'a> {
    fn visit_import_declaration(&mut self, it: &mut ImportDeclaration<'a>) {
        let specifier = it.source.value.as_str();
        if let Some(remapped) = self.rewrite(specifier) {
            it.source.value = self.allocator.alloc_str(&remapped).into();
        }
        walk_mut::walk_import_declaration(self, it);
    }

    fn visit_export_named_declaration(&mut self, it: &mut ExportNamedDeclaration<'a>) {
        if let Some(ref mut source) = it.source {
            let specifier = source.value.as_str();
            if let Some(remapped) = self.rewrite(specifier) {
                source.value = self.allocator.alloc_str(&remapped).into();
            }
        }
        walk_mut::walk_export_named_declaration(self, it);
    }

    fn visit_export_all_declaration(&mut self, it: &mut ExportAllDeclaration<'a>) {
        let specifier = it.source.value.as_str();
        if let Some(remapped) = self.rewrite(specifier) {
            it.source.value = self.allocator.alloc_str(&remapped).into();
        }
        walk_mut::walk_export_all_declaration(self, it);
    }

    fn visit_expression(&mut self, it: &mut Expression<'a>) {
        if let Expression::ImportExpression(import_expr) = it
            && let Expression::StringLiteral(s) = &mut import_expr.source
        {
            let specifier = s.value.as_str();
            if let Some(remapped) = self.rewrite(specifier) {
                s.value = self.allocator.alloc_str(&remapped).into();
            }
        }
        walk_mut::walk_expression(self, it);
    }
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
    let re = Regex::new(r"\bMAP(?:\.[A-Za-z_][A-Za-z0-9_]*)+").unwrap();
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
