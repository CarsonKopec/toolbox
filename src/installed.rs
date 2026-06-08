//! Per-package install records.
//!
//! When `install` extracts a package's layers into an env, the list of files it
//! laid down is recorded at `.toolbox/installed/<package>.json`. `uninstall`
//! reads that record to know exactly which files to remove, so removing one
//! package never disturbs files owned by another.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const INSTALLED_DIR: &str = ".toolbox/installed";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledFiles {
    pub package: String,
    pub version: String,
    /// The OCI reference this package was installed from.
    pub source: String,
    /// Files this package extracted, relative to the env root, forward slashes.
    /// Directory entries are not recorded.
    pub files: Vec<String>,
}

impl InstalledFiles {
    fn record_path(env_root: &Path, package: &str) -> PathBuf {
        env_root
            .join(INSTALLED_DIR)
            .join(format!("{}.json", sanitize(package)))
    }

    pub fn save(&self, env_root: &Path) -> Result<()> {
        let p = Self::record_path(env_root, &self.package);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&p, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", p.display()))?;
        Ok(())
    }

    /// Load the record for `package`, or `None` if it isn't installed.
    pub fn load(env_root: &Path, package: &str) -> Result<Option<Self>> {
        let p = Self::record_path(env_root, package);
        if !p.exists() {
            return Ok(None);
        }
        let s = fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Ok(Some(
            serde_json::from_str(&s).with_context(|| format!("parsing {}", p.display()))?,
        ))
    }

    /// Delete the record file. No-op if it doesn't exist.
    pub fn remove(env_root: &Path, package: &str) -> Result<()> {
        let p = Self::record_path(env_root, package);
        match fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", p.display())),
        }
    }
}

/// Make a package name safe to use as a single filename component.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_remove_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let rec = InstalledFiles {
            package: "ripgrep".into(),
            version: "14.1.0".into(),
            source: "ghcr.io/me/ripgrep:14.1.0".into(),
            files: vec!["windows/bin/rg.exe".into()],
        };
        rec.save(root).unwrap();

        let loaded = InstalledFiles::load(root, "ripgrep").unwrap().unwrap();
        assert_eq!(loaded.version, "14.1.0");
        assert_eq!(loaded.files, vec!["windows/bin/rg.exe".to_string()]);

        InstalledFiles::remove(root, "ripgrep").unwrap();
        assert!(InstalledFiles::load(root, "ripgrep").unwrap().is_none());
        // Removing again is a no-op.
        InstalledFiles::remove(root, "ripgrep").unwrap();
    }

    #[test]
    fn load_missing_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(InstalledFiles::load(tmp.path(), "nope").unwrap().is_none());
    }

    #[test]
    fn sanitize_replaces_path_separators() {
        assert_eq!(sanitize("ghcr.io/me/rg"), "ghcr.io_me_rg");
        assert_eq!(sanitize("a:b*c?"), "a_b_c_");
    }
}
