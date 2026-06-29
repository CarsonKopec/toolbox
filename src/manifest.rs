use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use tomlplus_syntax::value::Value;
use tomlplus_syntax::Document;

use crate::tomlp;

pub const MANIFEST_FILE: &str = "toolbox-env.tomlp";

/// Optional manifest a package author places at the root of a package tree to
/// declare metadata and activation contributions. Read by `push` (transcribed
/// into the OCI config blob) and by `install --from`.
pub const PACKAGE_MANIFEST_FILE: &str = "toolbox-package.tomlp";

/// The manifest stored at the root of every env directory.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub packages: Vec<PackageRef>,
    /// Activation contributions. Keys: "all", "windows", "linux", "macos".
    pub activation: BTreeMap<String, ActivationBlock>,
    /// Named, runnable tools, keyed by tool name. Invoked via `toolbox run`.
    pub tools: BTreeMap<String, Tool>,
}

/// A declared, runnable tool. `run`, `args`, and `env` values are render-time
/// templates (see `activation_vars`): they may use `$TOOLBOX_PREFIX` and
/// `$ENV.VAR ?? fallback`. Serde derives are for the OCI package config blob.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// Program to run: an interpreter/command resolved via the env's PATH, or a
    /// path (env-relative, or absolute after template resolution).
    pub run: String,
    /// Arguments passed before any caller-supplied arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Extra environment variables, layered on top of activation.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PackageRef {
    pub name: String,
    pub version: String,
    /// OCI reference, e.g. "ghcr.io/me/ripgrep:14.1.0".
    pub source: String,
}

/// Serde derives here are for the OCI package config blob (JSON); the env
/// manifest's TOML+ form is mapped by hand in `to_tomlp`/`from_tomlp`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActivationBlock {
    /// Paths to prepend to PATH, relative to the env root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_prepend: Vec<String>,
    /// Environment variables to set. Values are render-time templates resolved
    /// on activate (see `activation_vars`): they may reference `$TOOLBOX_PREFIX`
    /// (the env's mount path) and `$ENV.VAR ?? fallback` (host env with default).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl Manifest {
    /// Read and parse the manifest at `<root>/toolbox-env.tomlp`.
    pub fn load(root: &Path) -> Result<Self> {
        Self::from_path(&root.join(MANIFEST_FILE))
    }

    /// Read and parse a manifest from an explicit file path (e.g. a package
    /// manifest, which lives outside an env root).
    pub fn from_path(path: &Path) -> Result<Self> {
        let source =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_tomlp(&source, path)
    }

    /// Write the manifest to `<root>/toolbox-env.tomlp`.
    ///
    /// Self-validating: the rendered text is parsed back (running the TOML+
    /// annotation validator) before anything is written, so a value that
    /// violates its own annotations — e.g. an env name with illegal characters —
    /// fails here at write time instead of confusingly on the next load.
    pub fn save(&self, root: &Path) -> Result<()> {
        let path = root.join(MANIFEST_FILE);
        let text = self.rendered_and_checked(&path)?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Render to TOML+ text and verify the result re-parses and satisfies its
    /// own annotations. Returns the validated text. `path` is used only for
    /// error messages, so callers can run this before the file exists.
    pub fn rendered_and_checked(&self, path: &Path) -> Result<String> {
        let text = self.to_tomlp();
        Self::from_tomlp(&text, path)
            .with_context(|| format!("refusing to write invalid manifest to {}", path.display()))?;
        Ok(text)
    }

    /// Render the manifest as annotated TOML+ text.
    pub fn to_tomlp(&self) -> String {
        let mut doc = Document::default();

        doc.config
            .insert("name".into(), Value::String(self.name.clone()));
        // The name becomes a registry key and a directory name, so constrain it
        // to filesystem/shell-safe characters. (Top-level key: annotations here
        // round-trip; annotations inside block dicts do not.)
        doc.meta.insert(
            "name".into(),
            vec![
                tomlp::required(),
                tomlp::typed("string"),
                tomlp::minlen(1),
                tomlp::pattern("[A-Za-z0-9._-]+"),
            ],
        );

        doc.config
            .insert("version".into(), Value::String(self.version.clone()));
        doc.meta
            .insert("version".into(), vec![tomlp::required(), tomlp::typed("string")]);

        if let Some(desc) = &self.description {
            doc.config
                .insert("description".into(), Value::String(desc.clone()));
        }

        if !self.packages.is_empty() {
            let items = self
                .packages
                .iter()
                .map(|p| {
                    let mut d = BTreeMap::new();
                    d.insert("name".into(), Value::String(p.name.clone()));
                    d.insert("version".into(), Value::String(p.version.clone()));
                    d.insert("source".into(), Value::String(p.source.clone()));
                    Value::Dict(d)
                })
                .collect();
            doc.config.insert("packages".into(), Value::Array(items));
        }

        if !self.activation.is_empty() {
            let mut blocks = BTreeMap::new();
            for (os, block) in &self.activation {
                let mut b = BTreeMap::new();
                if !block.path_prepend.is_empty() {
                    b.insert(
                        "path_prepend".into(),
                        Value::Array(
                            block
                                .path_prepend
                                .iter()
                                .map(|s| Value::String(s.clone()))
                                .collect(),
                        ),
                    );
                }
                if !block.env.is_empty() {
                    let env = block
                        .env
                        .iter()
                        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                        .collect();
                    b.insert("env".into(), Value::Dict(env));
                }
                blocks.insert(os.clone(), Value::Dict(b));
            }
            doc.config.insert("activation".into(), Value::Dict(blocks));
        }

        if !self.tools.is_empty() {
            let mut tools = BTreeMap::new();
            for (tname, tool) in &self.tools {
                let mut t = BTreeMap::new();
                t.insert("run".into(), Value::String(tool.run.clone()));
                if !tool.args.is_empty() {
                    t.insert(
                        "args".into(),
                        Value::Array(
                            tool.args.iter().map(|s| Value::String(s.clone())).collect(),
                        ),
                    );
                }
                if !tool.env.is_empty() {
                    let env = tool
                        .env
                        .iter()
                        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                        .collect();
                    t.insert("env".into(), Value::Dict(env));
                }
                tools.insert(tname.clone(), Value::Dict(t));
            }
            doc.config.insert("tools".into(), Value::Dict(tools));
        }

        tomlp::dump(&doc)
    }

    /// Parse a manifest from TOML+ text. `path` is used for error messages only.
    pub fn from_tomlp(source: &str, path: &Path) -> Result<Self> {
        let doc = tomlp::parse_strict(source, path)?;
        let config = &doc.config;

        let name = tomlp::req_str(config, "name", path)?;
        let version = tomlp::req_str(config, "version", path)?;
        let description = tomlp::opt_str(config, "description", path)?;

        let mut packages = Vec::new();
        if let Some(value) = config.get("packages") {
            let arr = value.as_array().ok_or_else(|| {
                anyhow!("{}: `packages` must be an array", path.display())
            })?;
            for (i, item) in arr.iter().enumerate() {
                let d = item.as_dict().ok_or_else(|| {
                    anyhow!("{}: packages[{i}] must be a table", path.display())
                })?;
                packages.push(PackageRef {
                    name: dict_req_str(d, "name", i, path)?,
                    version: dict_req_str(d, "version", i, path)?,
                    source: dict_req_str(d, "source", i, path)?,
                });
            }
        }

        let mut activation = BTreeMap::new();
        if let Some(value) = config.get("activation") {
            let blocks = value.as_dict().ok_or_else(|| {
                anyhow!("{}: `activation` must be a table", path.display())
            })?;
            for (os, raw) in blocks {
                let b = raw.as_dict().ok_or_else(|| {
                    anyhow!("{}: activation.{os} must be a table", path.display())
                })?;
                let mut block = ActivationBlock::default();
                if let Some(pp) = b.get("path_prepend") {
                    let arr = pp.as_array().ok_or_else(|| {
                        anyhow!("{}: activation.{os}.path_prepend must be an array", path.display())
                    })?;
                    for s in arr {
                        block.path_prepend.push(
                            s.as_str()
                                .ok_or_else(|| {
                                    anyhow!(
                                        "{}: activation.{os}.path_prepend entries must be strings",
                                        path.display()
                                    )
                                })?
                                .to_string(),
                        );
                    }
                }
                if let Some(env) = b.get("env") {
                    let env = env.as_dict().ok_or_else(|| {
                        anyhow!("{}: activation.{os}.env must be a table", path.display())
                    })?;
                    for (k, v) in env {
                        let v = v.as_str().ok_or_else(|| {
                            anyhow!(
                                "{}: activation.{os}.env.{k} must be a string",
                                path.display()
                            )
                        })?;
                        block.env.insert(k.clone(), v.to_string());
                    }
                }
                activation.insert(os.clone(), block);
            }
        }

        let mut tools = BTreeMap::new();
        if let Some(value) = config.get("tools") {
            let map = value
                .as_dict()
                .ok_or_else(|| anyhow!("{}: `tools` must be a table", path.display()))?;
            for (tname, raw) in map {
                let t = raw.as_dict().ok_or_else(|| {
                    anyhow!("{}: tools.{tname} must be a table", path.display())
                })?;
                let run = t.get("run").and_then(Value::as_str).ok_or_else(|| {
                    anyhow!("{}: tools.{tname}.run must be a string", path.display())
                })?;
                let mut tool = Tool {
                    run: run.to_string(),
                    ..Default::default()
                };
                if let Some(a) = t.get("args") {
                    let arr = a.as_array().ok_or_else(|| {
                        anyhow!("{}: tools.{tname}.args must be an array", path.display())
                    })?;
                    for s in arr {
                        tool.args.push(
                            s.as_str()
                                .ok_or_else(|| {
                                    anyhow!(
                                        "{}: tools.{tname}.args entries must be strings",
                                        path.display()
                                    )
                                })?
                                .to_string(),
                        );
                    }
                }
                if let Some(e) = t.get("env") {
                    let d = e.as_dict().ok_or_else(|| {
                        anyhow!("{}: tools.{tname}.env must be a table", path.display())
                    })?;
                    for (k, v) in d {
                        let v = v.as_str().ok_or_else(|| {
                            anyhow!("{}: tools.{tname}.env.{k} must be a string", path.display())
                        })?;
                        tool.env.insert(k.clone(), v.to_string());
                    }
                }
                tools.insert(tname.clone(), tool);
            }
        }

        Ok(Manifest {
            name,
            version,
            description,
            packages,
            activation,
            tools,
        })
    }
}

fn dict_req_str(
    d: &BTreeMap<String, Value>,
    key: &str,
    idx: usize,
    path: &Path,
) -> Result<String> {
    d.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{}: packages[{idx}].{key} must be a string", path.display()))
}

/// Small accessor extensions used while walking the parsed tree.
trait ValueExt {
    fn as_array(&self) -> Option<&[Value]>;
    fn as_dict(&self) -> Option<&BTreeMap<String, Value>>;
}

impl ValueExt for Value {
    fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    fn as_dict(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        let mut env = BTreeMap::new();
        env.insert("DATA_DIR".to_string(), "$TOOLBOX_PREFIX/share/data".to_string());
        let mut activation = BTreeMap::new();
        activation.insert(
            "all".to_string(),
            ActivationBlock {
                path_prepend: vec!["share/bin".into()],
                env,
            },
        );
        let mut tools = BTreeMap::new();
        tools.insert(
            "fmt".to_string(),
            Tool {
                run: "python".into(),
                args: vec!["$TOOLBOX_PREFIX/share/scripts/fmt.py".into()],
                env: BTreeMap::new(),
            },
        );
        Manifest {
            name: "myenv".into(),
            version: "0.1.0".into(),
            description: Some("a test env".into()),
            packages: vec![PackageRef {
                name: "ripgrep".into(),
                version: "14.1.0".into(),
                source: "ghcr.io/me/ripgrep:14.1.0".into(),
            }],
            activation,
            tools,
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let m = sample();
        let text = m.to_tomlp();
        let back = Manifest::from_tomlp(&text, Path::new("test.tomlp")).unwrap();

        assert_eq!(back.name, "myenv");
        assert_eq!(back.version, "0.1.0");
        assert_eq!(back.description.as_deref(), Some("a test env"));
        assert_eq!(back.packages.len(), 1);
        assert_eq!(back.packages[0].source, "ghcr.io/me/ripgrep:14.1.0");
        // The $TOOLBOX_PREFIX sentinel must survive as a literal string.
        assert_eq!(
            back.activation["all"].env["DATA_DIR"],
            "$TOOLBOX_PREFIX/share/data"
        );
        assert_eq!(back.activation["all"].path_prepend, vec!["share/bin"]);
        // Tools round-trip, including templated args.
        assert_eq!(back.tools["fmt"].run, "python");
        assert_eq!(
            back.tools["fmt"].args,
            vec!["$TOOLBOX_PREFIX/share/scripts/fmt.py"]
        );
    }

    #[test]
    fn activation_value_with_embedded_quotes_round_trips() {
        // Guards the manifest round trip for values containing double quotes
        // (e.g. `$ENV.EDITOR ?? "vim"`). Passed directly, bypassing any shell.
        let mut env = BTreeMap::new();
        env.insert("EDITOR".to_string(), "$ENV.EDITOR ?? \"vim\"".to_string());
        let mut activation = BTreeMap::new();
        activation.insert("all".to_string(), ActivationBlock { path_prepend: vec![], env });

        let m = Manifest {
            name: "q".into(),
            version: "0.1.0".into(),
            description: None,
            packages: vec![],
            activation,
            tools: BTreeMap::new(),
        };
        let back = Manifest::from_tomlp(&m.to_tomlp(), Path::new("m.tomlp")).unwrap();
        assert_eq!(back.activation["all"].env["EDITOR"], "$ENV.EDITOR ?? \"vim\"");
    }

    #[test]
    fn emits_required_annotations() {
        let text = sample().to_tomlp();
        assert!(text.contains("@required"));
        assert!(text.contains("@type: string"));
    }

    #[test]
    fn minimal_manifest_round_trips() {
        let m = Manifest {
            name: "bare".into(),
            version: "1.0.0".into(),
            description: None,
            packages: vec![],
            activation: BTreeMap::new(),
            tools: BTreeMap::new(),
        };
        let text = m.to_tomlp();
        assert!(!text.contains("packages"));
        assert!(!text.contains("[activation]"));
        assert!(!text.contains("[tools]"));
        let back = Manifest::from_tomlp(&text, Path::new("m.tomlp")).unwrap();
        assert_eq!(back.name, "bare");
        assert!(back.packages.is_empty());
        assert!(back.activation.is_empty());
        assert!(back.tools.is_empty());
    }

    #[test]
    fn manifest_with_utf8_bom_parses() {
        // Windows editors prepend a BOM; it must not fold into the first key.
        let src = "\u{feff}name = \"q\"\nversion = \"1.0.0\"\n";
        let m = Manifest::from_tomlp(src, Path::new("m.tomlp")).unwrap();
        assert_eq!(m.name, "q");
        assert_eq!(m.version, "1.0.0");
    }

    #[test]
    fn missing_required_key_errors() {
        let err = Manifest::from_tomlp("version = \"1.0\"\n", Path::new("m.tomlp")).unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn bad_name_rejected_by_self_validation() {
        // A name with a space violates the emitted @pattern, so rendering +
        // checking must fail (this is what `save` runs before writing).
        let m = Manifest {
            name: "bad name".into(),
            version: "0.1.0".into(),
            description: None,
            packages: vec![],
            activation: BTreeMap::new(),
            tools: BTreeMap::new(),
        };
        let err = m.rendered_and_checked(Path::new("m.tomlp")).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("invalid"));
    }

    #[test]
    fn good_name_passes_self_validation() {
        let m = Manifest {
            name: "my-env_2.0".into(),
            version: "0.1.0".into(),
            description: None,
            packages: vec![],
            activation: BTreeMap::new(),
            tools: BTreeMap::new(),
        };
        assert!(m.rendered_and_checked(Path::new("m.tomlp")).is_ok());
    }
}
