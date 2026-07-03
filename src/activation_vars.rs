//! Resolution of activation value templates.
//!
//! Activation env values (and `$TOOLBOX_PREFIX` references) are treated as
//! **render-time templates**: they are stored in the manifest as ordinary
//! quoted strings and only interpreted when an env is activated. Two forms are
//! supported in a value:
//!
//!   * `$TOOLBOX_PREFIX` — the env's current mount path (legacy sentinel).
//!   * `$ENV.VAR ?? fallback` — read `VAR` from the host environment, with an
//!     optional default — direnv/conda-style. (Full TOML+ value-expression
//!     syntax is available, e.g. `$ENV.TOOLBOX_PREFIX + "/share"`.)
//!
//! ## Why render-time, not parse-time
//!
//! TOML+ resolves variables at *parse* time. Resolving on manifest load and
//! re-serializing would bake the *resolved* value into the file, destroying the
//! original `$ENV.EDITOR ?? "vim"` on the next save. Keeping values as
//! render-time templates — strings that round-trip untouched and are evaluated
//! only when emitting the activation script — sidesteps that entirely. The
//! `parse_time_resolution_would_lose_the_expression` test pins down the failure
//! mode we're avoiding. The trade: a value referencing a variable must be valid
//! TOML+ expression syntax, and we pay a tiny parse per value at activate time.

use anyhow::{anyhow, Result};
use std::path::Path;

use tomlplus_syntax::error::Severity;
use tomlplus_syntax::parser::parse_with_env;
use tomlplus_syntax::value::Value;

/// Resolve an activation value for the env mounted at `env_root`.
///
/// 1. The legacy bare `$TOOLBOX_PREFIX` sentinel is replaced first, so existing
///    manifests behave exactly as before.
/// 2. If the result still references a TOML+ variable (`$ENV.`), it is evaluated
///    through the TOML+ engine with the host environment available (and
///    `TOOLBOX_PREFIX` injected as the env root).
/// 3. Plain literal values are returned unchanged.
pub fn resolve(value: &str, env_root: &Path) -> Result<String> {
    let prefix = env_root.to_string_lossy();
    let legacy = value.replace("$TOOLBOX_PREFIX", &prefix);
    if !legacy.contains("$ENV.") {
        return Ok(legacy);
    }
    eval_expr(&legacy, env_root)
}

/// Evaluate `expr` as a single TOML+ value expression. `$ENV.X` resolves from
/// the host environment, except `$ENV.TOOLBOX_PREFIX`, which is injected as the
/// env root. Unset host vars resolve to null (so `?? fallback` kicks in).
fn eval_expr(expr: &str, env_root: &Path) -> Result<String> {
    let prefix = env_root.to_string_lossy().into_owned();
    let resolver = move |key: &str| -> Option<String> {
        if key == "TOOLBOX_PREFIX" {
            Some(prefix.clone())
        } else {
            std::env::var(key).ok()
        }
    };

    let source = format!("__v = {expr}");
    let doc = parse_with_env(&source, &resolver);

    if let Some(d) = doc
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
    {
        return Err(anyhow!(
            "evaluating activation value `{expr}`: {}",
            d.message
        ));
    }

    match doc.config.get("__v") {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Integer(n)) => Ok(n.to_string()),
        Some(Value::Float(f)) => Ok(f.to_string()),
        Some(Value::Bool(b)) => Ok(b.to_string()),
        Some(Value::Null) | None => Ok(String::new()),
        Some(other) => Err(anyhow!(
            "activation value `{expr}` resolved to a {}, expected a scalar",
            other.type_name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_prefix_sentinel_unchanged() {
        // The existing mechanism keeps working byte-for-byte.
        let r = resolve("$TOOLBOX_PREFIX/bin", Path::new("/opt/env")).unwrap();
        assert_eq!(r, "/opt/env/bin");
    }

    #[test]
    fn plain_literal_passes_through() {
        let r = resolve("just a value", Path::new("/x")).unwrap();
        assert_eq!(r, "just a value");
    }

    #[test]
    fn env_var_falls_back_to_default_when_unset() {
        // `$ENV.<unset>` -> null, then `?? "vim"` -> "vim". No host mutation,
        // so this is safe under parallel test execution.
        let r = resolve(
            "$ENV.TBX_DEFINITELY_UNSET_XYZ123 ?? \"vim\"",
            Path::new("/x"),
        )
        .unwrap();
        assert_eq!(r, "vim");
    }

    #[test]
    fn toolbox_prefix_available_as_env_var_with_concat() {
        let r = resolve("$ENV.TOOLBOX_PREFIX + \"/share\"", Path::new("/opt/env")).unwrap();
        assert_eq!(r, "/opt/env/share");
    }

    #[test]
    fn parse_time_resolution_would_lose_the_expression() {
        // Rationale guard for the render-time design: resolving at parse time
        // and re-serializing silently bakes the value down, dropping the
        // original `$ENV...?? ...`. This is the behavior we deliberately avoid.
        use tomlplus_syntax::dumper::dumps;
        use tomlplus_syntax::parser::parse;

        let src = "tool = $ENV.TBX_DEFINITELY_UNSET_XYZ123 ?? \"vim\"\n";
        let doc = parse(src);
        let dumped = dumps(&doc);

        assert!(dumped.contains("\"vim\""), "value resolved at parse time");
        assert!(
            !dumped.contains("$ENV"),
            "the `$ENV...?? ...` expression did not survive the round trip"
        );
    }
}
