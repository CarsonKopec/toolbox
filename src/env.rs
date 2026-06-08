use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{Manifest, MANIFEST_FILE};

/// A loaded env: its root path on disk and parsed manifest.
#[derive(Debug)]
pub struct Env {
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Env {
    /// Load an env from a directory on disk.
    pub fn load(root: &Path) -> Result<Self> {
        if !root.exists() {
            return Err(anyhow!("env path does not exist: {}", root.display()));
        }
        let manifest = Manifest::load(root)?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
        })
    }

    /// Create a new empty env at `root` with the given name.
    pub fn create(root: &Path, name: &str) -> Result<Self> {
        if root.exists() && root.read_dir()?.next().is_some() {
            return Err(anyhow!(
                "refusing to init non-empty directory: {}",
                root.display()
            ));
        }

        let manifest = Manifest {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            description: None,
            packages: vec![],
            activation: Default::default(),
            tools: Default::default(),
        };
        // Validate the manifest (and thus the name) before creating anything on
        // disk, so a bad name doesn't leave an orphan directory skeleton behind.
        let manifest_text = manifest.rendered_and_checked(&root.join(MANIFEST_FILE))?;

        fs::create_dir_all(root)?;
        for os in ["windows", "linux", "macos"] {
            for sub in ["bin", "lib", "share"] {
                fs::create_dir_all(root.join(os).join(sub))?;
            }
        }
        fs::create_dir_all(root.join("share"))?;
        fs::create_dir_all(root.join(".toolbox"))?;
        fs::write(root.join(MANIFEST_FILE), manifest_text)
            .with_context(|| format!("writing {}", root.join(MANIFEST_FILE).display()))?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
        })
    }
}
