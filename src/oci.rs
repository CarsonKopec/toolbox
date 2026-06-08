//! OCI artifact distribution for ToolBox packages.
//!
//! Wire format:
//! - The manifest is a standard OCI image manifest.
//! - The config blob has `mediaType: application/vnd.toolbox.package.config.v1+json`
//!   and its payload is a small JSON descriptor (name, version, platforms).
//! - Each layer has `mediaType: application/vnd.toolbox.package.layer.v1.tar+zstd`
//!   and its payload is a zstd-compressed tarball whose entries overlay onto
//!   the env root (`windows/...`, `linux/...`, `macos/...`, `share/...`).
//! - Files inside the layer have their build-time prefix replaced by
//!   `__TOOLBOX_PREFIX__` (see relocate.rs) so they are position-independent.
//!
//! Caching:
//! - Pulled blobs are content-addressed and cached under
//!   `<install_root>/cache/oci/blobs/sha256/<hex>`. Repeat pulls are offline.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::paths;

pub const CONFIG_MEDIA_TYPE: &str = "application/vnd.toolbox.package.config.v1+json";
pub const LAYER_MEDIA_TYPE: &str = "application/vnd.toolbox.package.layer.v1.tar+zstd";

/// The JSON payload of the config blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageConfig {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Activation contributions the package declares (PATH additions, env vars),
    /// keyed by "all"/"windows"/"linux"/"macos". Merged into the env manifest on
    /// install.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub activation: std::collections::BTreeMap<String, crate::manifest::ActivationBlock>,
    /// Runnable tools the package ships, merged into the env manifest on install.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tools: std::collections::BTreeMap<String, crate::manifest::Tool>,
}

/// Summary of what a pull produced — used to update the env manifest.
#[derive(Debug, Clone)]
pub struct PullSummary {
    pub name: String,
    pub version: String,
    pub layer_count: usize,
    /// Files extracted across all layers, relative to the env root, forward
    /// slashes. Directory entries are not included.
    pub files: Vec<String>,
    /// Activation contributions declared by the package's config blob.
    pub activation: std::collections::BTreeMap<String, crate::manifest::ActivationBlock>,
    /// Tools declared by the package's config blob.
    pub tools: std::collections::BTreeMap<String, crate::manifest::Tool>,
}

/// Pull a ToolBox OCI artifact and extract its layers into `dest`.
/// Anonymous auth only for now.
pub fn pull(reference: &str, dest: &Path) -> Result<PullSummary> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("creating tokio runtime")?;
    rt.block_on(pull_async(reference, dest))
}

async fn pull_async(reference: &str, dest: &Path) -> Result<PullSummary> {
    use oci_client::client::ClientConfig;
    use oci_client::secrets::RegistryAuth;
    use oci_client::{Client, Reference};

    let r: Reference = reference
        .parse()
        .with_context(|| format!("parsing OCI reference {reference:?}"))?;
    let client = Client::new(ClientConfig::default());
    let auth = RegistryAuth::Anonymous;

    let (manifest, _digest) = client
        .pull_image_manifest(&r, &auth)
        .await
        .with_context(|| format!("pulling manifest for {reference}"))?;

    if manifest.config.media_type != CONFIG_MEDIA_TYPE {
        return Err(anyhow!(
            "artifact at {reference} has config mediaType {:?}, expected {:?}",
            manifest.config.media_type,
            CONFIG_MEDIA_TYPE
        ));
    }

    let cache = oci_cache_dir()?;
    std::fs::create_dir_all(&cache)?;
    std::fs::create_dir_all(dest)?;

    // Pull config blob and parse it for name/version.
    let config_bytes = pull_blob_cached(&client, &r, &manifest.config, &cache).await?;
    let config: PackageConfig = serde_json::from_slice(&config_bytes)
        .context("parsing ToolBox package config blob")?;

    let mut layer_count = 0;
    let mut files: Vec<String> = Vec::new();
    for layer in &manifest.layers {
        if layer.media_type != LAYER_MEDIA_TYPE {
            eprintln!(
                "toolbox: warning: skipping layer with unexpected mediaType {:?}",
                layer.media_type
            );
            continue;
        }
        let blob_path = pull_blob_to_cache(&client, &r, layer, &cache).await?;
        let layer_files = extract_layer(&blob_path, dest)
            .with_context(|| format!("extracting layer {}", layer.digest))?;
        files.extend(layer_files);
        layer_count += 1;
    }

    Ok(PullSummary {
        name: config.name,
        version: config.version,
        layer_count,
        files,
        activation: config.activation,
        tools: config.tools,
    })
}

/// Options controlling how a package tree is packaged into an OCI artifact.
#[derive(Debug, Default, Clone)]
pub struct PushOptions {
    /// Package name. Defaults to the last path segment of the reference repo.
    pub name: Option<String>,
    /// Package version. Defaults to the reference tag.
    pub version: Option<String>,
    /// Platforms recorded in the config. Defaults to the per-OS dirs present.
    pub platforms: Vec<String>,
    /// Optional human description recorded in the config.
    pub description: Option<String>,
}

/// Result of a successful push.
#[derive(Debug, Clone)]
pub struct PushSummary {
    pub name: String,
    pub version: String,
    pub manifest_url: String,
}

/// Package the tree at `src` into a single tar+zstd layer plus a ToolBox config
/// blob, and push it to `reference`. The author is responsible for having baked
/// the `__TOOLBOX_PREFIX__` sentinel into relocatable files beforehand.
///
/// Auth: if `TOOLBOX_REGISTRY_USERNAME` and `TOOLBOX_REGISTRY_PASSWORD` are set,
/// basic auth is used; otherwise the push is anonymous (most registries reject
/// anonymous pushes, so credentials are usually required).
pub fn push(src: &Path, reference: &str, opts: &PushOptions) -> Result<PushSummary> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("creating tokio runtime")?;
    rt.block_on(push_async(src, reference, opts))
}

async fn push_async(src: &Path, reference: &str, opts: &PushOptions) -> Result<PushSummary> {
    use oci_client::client::{ClientConfig, Config, ImageLayer};
    use oci_client::{Client, Reference};

    let r: Reference = reference
        .parse()
        .with_context(|| format!("parsing OCI reference {reference:?}"))?;

    // An optional package manifest in the tree supplies activation and default
    // metadata; explicit push options/reference still take precedence.
    let pkg_manifest = {
        let p = src.join(crate::manifest::PACKAGE_MANIFEST_FILE);
        if p.exists() {
            Some(crate::manifest::Manifest::from_path(&p)?)
        } else {
            None
        }
    };

    let name = opts
        .name
        .clone()
        .or_else(|| pkg_manifest.as_ref().map(|m| m.name.clone()))
        .unwrap_or_else(|| {
            r.repository()
                .rsplit('/')
                .next()
                .unwrap_or_else(|| r.repository())
                .to_string()
        });
    let version = opts
        .version
        .clone()
        .or_else(|| pkg_manifest.as_ref().map(|m| m.version.clone()))
        .or_else(|| r.tag().map(str::to_string))
        .ok_or_else(|| {
            anyhow!("no version: pass a tagged reference (e.g. name:1.2.3) or use a version override")
        })?;
    let description = opts
        .description
        .clone()
        .or_else(|| pkg_manifest.as_ref().and_then(|m| m.description.clone()));
    let activation = pkg_manifest
        .as_ref()
        .map(|m| m.activation.clone())
        .unwrap_or_default();
    let tools = pkg_manifest.map(|m| m.tools).unwrap_or_default();

    let platforms = if opts.platforms.is_empty() {
        detect_platforms(src)
    } else {
        opts.platforms.clone()
    };

    let layer_data = build_layer(src)?;
    eprintln!(
        "toolbox: packaged {} ({} bytes compressed)",
        name,
        layer_data.len()
    );
    let layer = ImageLayer::new(layer_data, LAYER_MEDIA_TYPE.to_string(), None);

    let config = PackageConfig {
        name: name.clone(),
        version: version.clone(),
        platforms,
        description,
        activation,
        tools,
    };
    let config_data = serde_json::to_vec(&config).context("serializing package config")?;
    let config = Config::new(config_data, CONFIG_MEDIA_TYPE.to_string(), None);

    let client = Client::new(ClientConfig::default());
    let auth = push_auth();

    let resp = client
        .push(&r, &[layer], config, &auth, None)
        .await
        .with_context(|| format!("pushing {reference}"))?;

    Ok(PushSummary {
        name,
        version,
        manifest_url: resp.manifest_url,
    })
}

/// Which of the per-OS directories exist under `src`.
fn detect_platforms(src: &Path) -> Vec<String> {
    ["windows", "linux", "macos"]
        .into_iter()
        .filter(|os| src.join(os).is_dir())
        .map(str::to_string)
        .collect()
}

/// The top-level directories a package may overlay onto an env root. Env-local
/// metadata (`.toolbox/`, `toolbox-env.tomlp`) is intentionally not in this set,
/// so it is never packaged or copied and can't clobber the consuming env.
const OVERLAY_DIRS: [&str; 4] = ["windows", "linux", "macos", "share"];

/// The overlay directories actually present under `src`.
fn overlay_dirs(src: &Path) -> Vec<&'static str> {
    OVERLAY_DIRS
        .into_iter()
        .filter(|t| src.join(t).is_dir())
        .collect()
}

/// Overlay a local package tree directly into `dest` (no tar/registry round
/// trip), returning the relative paths of the regular files copied. Used by
/// `install --from <dir>`.
pub fn extract_dir(src: &Path, dest: &Path) -> Result<Vec<String>> {
    let tops = overlay_dirs(src);
    if tops.is_empty() {
        return Err(anyhow!(
            "no package contents under {} (expected windows/, linux/, macos/, or share/)",
            src.display()
        ));
    }

    let mut files = Vec::new();
    for top in tops {
        for entry in walkdir::WalkDir::new(src.join(top)) {
            let entry = entry.with_context(|| format!("walking {top}/"))?;
            let rel = entry
                .path()
                .strip_prefix(src)
                .expect("walked path is under src");
            let target = dest.join(rel);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&target)
                    .with_context(|| format!("creating {}", target.display()))?;
            } else if entry.file_type().is_file() {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(entry.path(), &target)
                    .with_context(|| format!("copying to {}", target.display()))?;
                files.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(files)
}

/// Build the layer payload: a zstd-compressed tarball of the package's overlay
/// directories (`windows/`, `linux/`, `macos/`, `share/`). Env-local metadata
/// (`.toolbox/`, `toolbox-env.tomlp`) is deliberately excluded so it can't
/// clobber the consuming env on extraction.
fn build_layer(src: &Path) -> Result<Vec<u8>> {
    let tops = overlay_dirs(src);
    if tops.is_empty() {
        return Err(anyhow!(
            "no package contents under {} (expected windows/, linux/, macos/, or share/)",
            src.display()
        ));
    }

    let mut encoder = zstd::Encoder::new(Vec::new(), 19).context("initializing zstd encoder")?;
    {
        let mut builder = tar::Builder::new(&mut encoder);
        builder.follow_symlinks(false);
        for top in &tops {
            builder
                .append_dir_all(top, src.join(top))
                .with_context(|| format!("adding {top}/ to layer"))?;
        }
        builder.finish().context("finalizing tar")?;
    }
    encoder.finish().context("finalizing zstd stream")
}

fn push_auth() -> oci_client::secrets::RegistryAuth {
    use oci_client::secrets::RegistryAuth;
    match (
        std::env::var("TOOLBOX_REGISTRY_USERNAME"),
        std::env::var("TOOLBOX_REGISTRY_PASSWORD"),
    ) {
        (Ok(u), Ok(p)) if !u.is_empty() => RegistryAuth::Basic(u, p),
        _ => RegistryAuth::Anonymous,
    }
}

// --- helpers ---

async fn pull_blob_cached(
    client: &oci_client::Client,
    reference: &oci_client::Reference,
    descriptor: &oci_client::manifest::OciDescriptor,
    cache: &Path,
) -> Result<Vec<u8>> {
    let blob_path = pull_blob_to_cache(client, reference, descriptor, cache).await?;
    Ok(std::fs::read(&blob_path)?)
}

async fn pull_blob_to_cache(
    client: &oci_client::Client,
    reference: &oci_client::Reference,
    descriptor: &oci_client::manifest::OciDescriptor,
    cache: &Path,
) -> Result<PathBuf> {
    let blob_path = cached_blob_path(cache, &descriptor.digest);
    if blob_path.exists() {
        return Ok(blob_path);
    }
    eprintln!(
        "toolbox: downloading {} ({} bytes)",
        descriptor.digest, descriptor.size
    );
    let mut buf: Vec<u8> = Vec::with_capacity(descriptor.size.max(0) as usize);
    client
        .pull_blob(reference, descriptor, &mut buf)
        .await
        .with_context(|| format!("downloading blob {}", descriptor.digest))?;
    verify_digest(&buf, &descriptor.digest)?;
    let tmp = blob_path.with_extension("part");
    std::fs::write(&tmp, &buf)?;
    std::fs::rename(&tmp, &blob_path)?;
    Ok(blob_path)
}

/// Extract every entry of a zstd+tar layer into `dest`, returning the relative
/// paths (forward slashes) of the regular files that were written. Iterates
/// entries individually rather than `Archive::unpack` so we can record the file
/// list for later uninstall; entries inherit the archive's preserve-permissions
/// flag, so behavior matches the bulk unpack.
fn extract_layer(blob: &Path, dest: &Path) -> Result<Vec<String>> {
    // `unpack_in` canonicalizes `dest`, so it must exist first. (`Archive::unpack`
    // used to create it for us.)
    std::fs::create_dir_all(dest)
        .with_context(|| format!("creating {}", dest.display()))?;
    let f = std::fs::File::open(blob)?;
    let decoder = zstd::Decoder::new(f).context("opening zstd decoder")?;
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(true);

    let mut files = Vec::new();
    for entry in archive.entries().context("reading tar entries")? {
        let mut entry = entry.context("reading tar entry")?;
        let rel = entry
            .path()
            .context("reading tar entry path")?
            .to_string_lossy()
            .replace('\\', "/");
        let is_file = entry.header().entry_type().is_file();
        let unpacked = entry
            .unpack_in(dest)
            .with_context(|| format!("unpacking {rel} into {}", dest.display()))?;
        // `unpack_in` returns false when it refuses an entry (e.g. an absolute
        // or `..`-escaping path); don't record those.
        if unpacked && is_file {
            files.push(rel);
        }
    }
    Ok(files)
}

fn verify_digest(data: &[u8], expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let (alg, want_hex) = expected
        .split_once(':')
        .ok_or_else(|| anyhow!("malformed digest: {expected}"))?;
    if alg != "sha256" {
        return Err(anyhow!("unsupported digest algorithm: {alg}"));
    }
    let got = Sha256::digest(data);
    let got_hex = hex::encode(got);
    if got_hex != want_hex {
        return Err(anyhow!(
            "digest mismatch: expected sha256:{want_hex}, got sha256:{got_hex}"
        ));
    }
    Ok(())
}

fn oci_cache_dir() -> Result<PathBuf> {
    Ok(paths::cache_dir()?.join("oci").join("blobs").join("sha256"))
}

fn cached_blob_path(cache: &Path, digest: &str) -> PathBuf {
    let (_, hex) = digest.split_once(':').unwrap_or(("sha256", digest));
    cache.join(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_verify_ok() {
        let data = b"hello toolbox";
        use sha2::{Digest, Sha256};
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(data)));
        verify_digest(data, &digest).unwrap();
    }

    #[test]
    fn digest_verify_mismatch() {
        verify_digest(b"x", "sha256:0000000000000000000000000000000000000000000000000000000000000000")
            .unwrap_err();
    }

    #[test]
    fn digest_verify_bad_algorithm() {
        verify_digest(b"x", "md5:00").unwrap_err();
    }

    #[test]
    fn extract_layer_round_trip() {
        // Build a tar with one file, zstd-compress it, then extract it.
        let workdir = tempfile::tempdir().unwrap();
        let blob_path = workdir.path().join("layer.tar.zst");
        {
            let f = std::fs::File::create(&blob_path).unwrap();
            let encoder = zstd::Encoder::new(f, 3).unwrap().auto_finish();
            let mut builder = tar::Builder::new(encoder);
            let payload = b"#!__TOOLBOX_PREFIX__/bin/python\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "windows/bin/hello.py", &payload[..])
                .unwrap();
            builder.finish().unwrap();
        }

        let dest = workdir.path().join("env");
        let files = extract_layer(&blob_path, &dest).unwrap();
        assert_eq!(files, vec!["windows/bin/hello.py".to_string()]);

        let extracted = std::fs::read_to_string(dest.join("windows").join("bin").join("hello.py")).unwrap();
        assert_eq!(extracted, "#!__TOOLBOX_PREFIX__/bin/python\n");
    }

    #[test]
    fn cached_blob_path_strips_algorithm() {
        let p = cached_blob_path(Path::new("/c"), "sha256:abc123");
        assert_eq!(p, PathBuf::from("/c/abc123"));
    }

    #[test]
    fn build_layer_round_trips_through_extract() {
        // Lay down a small package tree, package it, extract it, and confirm
        // the overlay dirs survive while env-local metadata is excluded.
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("pkg");
        std::fs::create_dir_all(src.join("windows").join("bin")).unwrap();
        std::fs::create_dir_all(src.join("share")).unwrap();
        std::fs::create_dir_all(src.join(".toolbox")).unwrap();
        std::fs::write(src.join("windows").join("bin").join("rg.exe"), b"binary").unwrap();
        std::fs::write(src.join("share").join("readme.txt"), b"hi").unwrap();
        // Should NOT be packaged:
        std::fs::write(src.join(".toolbox").join("last-prefix"), b"C:/old").unwrap();
        std::fs::write(src.join("toolbox-env.tomlp"), b"name='x'").unwrap();

        let data = build_layer(&src).unwrap();

        let dest = work.path().join("out");
        let blob = work.path().join("layer.tar.zst");
        std::fs::write(&blob, &data).unwrap();
        let mut files = extract_layer(&blob, &dest).unwrap();
        files.sort();
        assert_eq!(files, vec!["share/readme.txt", "windows/bin/rg.exe"]);
        assert!(!dest.join(".toolbox").exists());
        assert!(!dest.join("toolbox-env.tomlp").exists());
    }

    #[test]
    fn extract_dir_copies_overlay_and_skips_metadata() {
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("pkg");
        std::fs::create_dir_all(src.join("windows").join("bin")).unwrap();
        std::fs::create_dir_all(src.join("share")).unwrap();
        std::fs::create_dir_all(src.join(".toolbox")).unwrap();
        std::fs::write(src.join("windows").join("bin").join("rg.exe"), b"bin").unwrap();
        std::fs::write(src.join("share").join("readme.txt"), b"hi").unwrap();
        // Env-local metadata that must NOT be copied:
        std::fs::write(src.join(".toolbox").join("last-prefix"), b"old").unwrap();
        std::fs::write(src.join("toolbox-env.tomlp"), b"name = \"x\"").unwrap();

        let dest = work.path().join("env");
        let mut files = extract_dir(&src, &dest).unwrap();
        files.sort();
        assert_eq!(files, vec!["share/readme.txt", "windows/bin/rg.exe"]);
        assert_eq!(
            std::fs::read(dest.join("windows").join("bin").join("rg.exe")).unwrap(),
            b"bin"
        );
        assert!(!dest.join(".toolbox").exists());
        assert!(!dest.join("toolbox-env.tomlp").exists());
    }

    #[test]
    fn extract_dir_errors_on_empty_tree() {
        let work = tempfile::tempdir().unwrap();
        extract_dir(work.path(), &work.path().join("env")).unwrap_err();
    }

    #[test]
    fn detect_platforms_lists_present_os_dirs() {
        let work = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("windows")).unwrap();
        std::fs::create_dir_all(work.path().join("linux")).unwrap();
        let mut got = detect_platforms(work.path());
        got.sort();
        assert_eq!(got, vec!["linux".to_string(), "windows".to_string()]);
    }

    #[test]
    fn build_layer_errors_on_empty_tree() {
        let work = tempfile::tempdir().unwrap();
        build_layer(work.path()).unwrap_err();
    }
}
