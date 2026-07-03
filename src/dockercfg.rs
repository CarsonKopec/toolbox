//! Credential lookup from the Docker config (`~/.docker/config.json`, or the
//! directory named by `$DOCKER_CONFIG`). Supports `auths` entries (base64
//! `user:pass`, or plain username/password) and credential helpers
//! (`credHelpers` / `credsStore`, via the `docker-credential-<name>` binaries),
//! so credentials saved by `docker login` work for ToolBox pulls and pushes.

use base64::Engine;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize)]
struct DockerConfig {
    #[serde(default)]
    auths: HashMap<String, AuthEntry>,
    #[serde(default, rename = "credHelpers")]
    cred_helpers: HashMap<String, String>,
    #[serde(default, rename = "credsStore")]
    creds_store: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct AuthEntry {
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

impl AuthEntry {
    fn to_creds(&self) -> Option<(String, String)> {
        if let Some(b64) = &self.auth {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                if let Ok(s) = String::from_utf8(bytes) {
                    if let Some((u, p)) = s.split_once(':') {
                        return Some((u.to_string(), p.to_string()));
                    }
                }
            }
        }
        match (&self.username, &self.password) {
            (Some(u), Some(p)) => Some((u.clone(), p.clone())),
            _ => None,
        }
    }
}

/// Look up `(username, password)` for `registry` from the Docker config, or
/// `None` if there's no config or no matching credential.
pub fn auth_for(registry: &str) -> Option<(String, String)> {
    let path = config_dir()?.join("config.json");
    let data = std::fs::read_to_string(path).ok()?;
    let cfg: DockerConfig = serde_json::from_str(&data).ok()?;
    lookup(&cfg, registry)
}

fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DOCKER_CONFIG") {
        return Some(PathBuf::from(d));
    }
    dirs::home_dir().map(|h| h.join(".docker"))
}

/// Registry names to try, since Docker keys vary (host, `https://host`, and the
/// legacy Docker Hub URL).
fn candidate_keys(registry: &str) -> Vec<String> {
    let mut keys = vec![registry.to_string(), format!("https://{registry}")];
    if matches!(
        registry,
        "docker.io" | "registry-1.docker.io" | "index.docker.io"
    ) {
        keys.push("https://index.docker.io/v1/".to_string());
    }
    keys
}

fn lookup(cfg: &DockerConfig, registry: &str) -> Option<(String, String)> {
    let candidates = candidate_keys(registry);

    // Per-registry credential helper takes precedence.
    for key in &candidates {
        if let Some(helper) = cfg.cred_helpers.get(key) {
            if let Some(creds) = cred_helper_get(helper, key) {
                return Some(creds);
            }
        }
    }
    // Then the global credential store.
    if let Some(store) = &cfg.creds_store {
        for key in &candidates {
            if let Some(creds) = cred_helper_get(store, key) {
                return Some(creds);
            }
        }
    }
    // Finally, inline `auths` entries.
    for key in &candidates {
        if let Some(entry) = cfg.auths.get(key) {
            if let Some(creds) = entry.to_creds() {
                return Some(creds);
            }
        }
    }
    None
}

/// Invoke `docker-credential-<helper> get` with `server` on stdin, parsing the
/// `{"Username","Secret"}` response. Returns `None` if the helper is missing,
/// errors, or has no credential for the server.
fn cred_helper_get(helper: &str, server: &str) -> Option<(String, String)> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new(format!("docker-credential-{helper}"))
        .arg("get")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(server.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }

    #[derive(Deserialize)]
    struct Response {
        #[serde(rename = "Username", default)]
        username: String,
        #[serde(rename = "Secret", default)]
        secret: String,
    }
    let r: Response = serde_json::from_slice(&out.stdout).ok()?;
    if r.secret.is_empty() {
        None
    } else {
        Some((r.username, r.secret))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> DockerConfig {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn auths_base64_is_decoded() {
        // base64("carson:secret-token") = "Y2Fyc29uOnNlY3JldC10b2tlbg=="
        let cfg =
            parse(r#"{ "auths": { "ghcr.io": { "auth": "Y2Fyc29uOnNlY3JldC10b2tlbg==" } } }"#);
        assert_eq!(
            lookup(&cfg, "ghcr.io"),
            Some(("carson".into(), "secret-token".into()))
        );
    }

    #[test]
    fn auths_plain_username_password() {
        let cfg = parse(r#"{ "auths": { "ghcr.io": { "username": "me", "password": "pw" } } }"#);
        assert_eq!(lookup(&cfg, "ghcr.io"), Some(("me".into(), "pw".into())));
    }

    #[test]
    fn docker_hub_key_variants_match() {
        let cfg = parse(r#"{ "auths": { "https://index.docker.io/v1/": { "auth": "YTpi" } } }"#);
        // base64("a:b") = "YTpi"
        assert_eq!(lookup(&cfg, "docker.io"), Some(("a".into(), "b".into())));
    }

    #[test]
    fn unknown_registry_is_none() {
        let cfg = parse(r#"{ "auths": { "ghcr.io": { "auth": "YTpi" } } }"#);
        assert_eq!(lookup(&cfg, "quay.io"), None);
    }

    #[test]
    fn empty_marker_entry_is_none() {
        // Docker leaves an empty entry when the real cred is in a store.
        let cfg = parse(r#"{ "auths": { "ghcr.io": {} } }"#);
        assert_eq!(lookup(&cfg, "ghcr.io"), None);
    }
}
