//! Thin adapter over `tomlplus-syntax` for ToolBox's config files.
//!
//! The TOML+ core has no serde integration: parsing yields a `Document` whose
//! `config` is a `BTreeMap<String, Value>` tree, and writing goes through
//! `dumps`. This module centralizes the glue — strict parsing (diagnostics
//! become hard errors), annotation construction for generated files, and a few
//! typed accessors — so `manifest.rs` and `registry.rs` only deal in their own
//! structs.

use anyhow::{anyhow, Result};
use std::collections::BTreeMap;
use std::path::Path;

use tomlplus_syntax::annotation::{Annotation, AnnotationArg};
use tomlplus_syntax::dumper::dumps;
use tomlplus_syntax::error::Severity;
use tomlplus_syntax::span::Span;
use tomlplus_syntax::value::Value;
use tomlplus_syntax::{parse, validate, Document};

/// Parse TOML+ source, treating any error-severity diagnostic (from either the
/// parser or the annotation validator) as a hard failure. `path` is only used
/// for error messages.
pub fn parse_strict(source: &str, path: &Path) -> Result<Document> {
    // Tolerate a leading UTF-8 BOM: Windows editors (Notepad, PowerShell's
    // `Set-Content -Encoding utf8`) prepend one, which would otherwise fold into
    // the first key name (e.g. `\u{feff}name`) and break parsing.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let doc = parse(source);

    let mut errors: Vec<String> = doc
        .diagnostics
        .iter()
        .chain(validate(&doc).iter())
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect();

    if !errors.is_empty() {
        errors.dedup();
        return Err(anyhow!(
            "invalid TOML+ in {}:\n  - {}",
            path.display(),
            errors.join("\n  - ")
        ));
    }
    Ok(doc)
}

/// Serialize a document (config + annotations) to TOML+ text.
pub fn dump(doc: &Document) -> String {
    dumps(doc)
}

/// Build an annotation with the given name/argument. Spans are `DUMMY` — the
/// dumper only reads the name and argument when emitting `@`-lines.
pub fn ann(name: &str, arg: AnnotationArg) -> Annotation {
    Annotation {
        name: name.to_string(),
        arg,
        span: Span::DUMMY,
        name_span: Span::DUMMY,
        arg_span: None,
        list_item_spans: Vec::new(),
    }
}

/// `@required` flag annotation.
pub fn required() -> Annotation {
    ann("required", AnnotationArg::None)
}

/// `@type: <name>` annotation.
pub fn typed(type_name: &str) -> Annotation {
    ann("type", AnnotationArg::String(type_name.to_string()))
}

/// `@minlen: <n>` annotation.
pub fn minlen(n: i64) -> Annotation {
    ann("minlen", AnnotationArg::Int(n))
}

/// `@pattern: <regex>` annotation. The regex must not start with `[` *and* end
/// with `]`, or the dumper's unquoted emission would be re-read as an `@enum`
/// list on the next parse.
pub fn pattern(regex: &str) -> Annotation {
    ann("pattern", AnnotationArg::String(regex.to_string()))
}

// --- typed accessors over the parsed config tree ---

/// A required string field; errors if absent or not a string.
pub fn req_str(config: &BTreeMap<String, Value>, key: &str, ctx: &Path) -> Result<String> {
    match config.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(anyhow!(
            "{}: key `{key}` must be a string, found {}",
            ctx.display(),
            other.type_name()
        )),
        None => Err(anyhow!("{}: missing required key `{key}`", ctx.display())),
    }
}

/// An optional string field; errors if present but not a string.
pub fn opt_str(config: &BTreeMap<String, Value>, key: &str, ctx: &Path) -> Result<Option<String>> {
    match config.get(key) {
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(other) => Err(anyhow!(
            "{}: key `{key}` must be a string, found {}",
            ctx.display(),
            other.type_name()
        )),
    }
}
