use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use tomlplus_syntax::value::Value;
use tomlplus_syntax::Document;

use crate::paths;
use crate::tomlp;

/// Per-machine registry mapping env name → absolute path on this machine.
/// Lives at `<install_root>/registry.tomlp`.
#[derive(Debug, Default)]
pub struct Registry {
    pub envs: BTreeMap<String, EnvEntry>,
}

#[derive(Debug, Clone)]
pub struct EnvEntry {
    pub path: PathBuf,
}

impl Registry {
    pub fn load() -> Result<Self> {
        let p = paths::registry_path()?;
        if !p.exists() {
            return Ok(Self::default());
        }
        let s = fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Self::from_tomlp(&s, &p)
    }

    pub fn save(&self) -> Result<()> {
        let p = paths::registry_path()?;
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&p, self.to_tomlp()).with_context(|| format!("writing {}", p.display()))?;
        Ok(())
    }

    fn to_tomlp(&self) -> String {
        let mut doc = Document::default();
        if !self.envs.is_empty() {
            let mut envs = BTreeMap::new();
            for (name, entry) in &self.envs {
                let mut d = BTreeMap::new();
                d.insert(
                    "path".into(),
                    Value::String(entry.path.to_string_lossy().into_owned()),
                );
                envs.insert(name.clone(), Value::Dict(d));
            }
            doc.config.insert("envs".into(), Value::Dict(envs));
        }
        tomlp::dump(&doc)
    }

    fn from_tomlp(source: &str, path: &Path) -> Result<Self> {
        let doc = tomlp::parse_strict(source, path)?;
        let mut envs = BTreeMap::new();
        if let Some(Value::Dict(map)) = doc.config.get("envs") {
            for (name, raw) in map {
                let entry = match raw {
                    Value::Dict(d) => d,
                    other => {
                        return Err(anyhow!(
                            "{}: envs.{name} must be a table, found {}",
                            path.display(),
                            other.type_name()
                        ))
                    }
                };
                let p = entry.get("path").and_then(Value::as_str).ok_or_else(|| {
                    anyhow!("{}: envs.{name} is missing a string `path`", path.display())
                })?;
                envs.insert(
                    name.clone(),
                    EnvEntry {
                        path: PathBuf::from(p),
                    },
                );
            }
        }
        Ok(Self { envs })
    }

    pub fn insert(&mut self, name: &str, path: &Path) -> Result<()> {
        if self.envs.contains_key(name) {
            return Err(anyhow!("env '{}' already registered", name));
        }
        self.envs.insert(
            name.to_string(),
            EnvEntry {
                path: path.to_path_buf(),
            },
        );
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        self.envs
            .remove(name)
            .ok_or_else(|| anyhow!("env '{}' not registered", name))?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<&EnvEntry> {
        self.envs
            .get(name)
            .ok_or_else(|| anyhow!("env '{}' not registered", name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_tomlp() {
        let mut reg = Registry::default();
        // A Windows-style path with backslashes must survive escaping.
        reg.insert("tools", Path::new(r"C:\Users\me\envs\tools")).unwrap();
        reg.insert("dev", Path::new("/home/me/dev")).unwrap();

        let text = reg.to_tomlp();
        let back = Registry::from_tomlp(&text, Path::new("registry.tomlp")).unwrap();

        assert_eq!(back.envs.len(), 2);
        assert_eq!(
            back.get("tools").unwrap().path,
            PathBuf::from(r"C:\Users\me\envs\tools")
        );
        assert_eq!(back.get("dev").unwrap().path, PathBuf::from("/home/me/dev"));
    }

    #[test]
    fn empty_registry_round_trips() {
        let reg = Registry::default();
        let text = reg.to_tomlp();
        let back = Registry::from_tomlp(&text, Path::new("registry.tomlp")).unwrap();
        assert!(back.envs.is_empty());
    }
}
