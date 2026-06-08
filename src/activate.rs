//! Generate shell snippets for `eval $(toolbox activate <name>)`.
//!
//! On activate we:
//!   1. Save the current PATH and any vars we'll overwrite into
//!      _TOOLBOX_STASH_<name> so deactivate can restore them.
//!   2. Prepend per-OS bin dirs to PATH.
//!   3. Export TOOLBOX_PREFIX, TOOLBOX_ACTIVE_ENV.
//!   4. Apply manifest [activation.all] and [activation.<os>] env vars,
//!      substituting $TOOLBOX_PREFIX in their values.
//!
//! Shell detection: emit POSIX for bash/zsh/sh, PowerShell syntax when invoked
//! from pwsh/powershell. cmd.exe is out-of-scope for v1.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::env::Env;
use crate::manifest::ActivationBlock;
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Posix,
    PowerShell,
}

impl Shell {
    pub fn detect() -> Self {
        // Heuristics; can be overridden by `--shell` on the activate command later.
        if std::env::var_os("PSModulePath").is_some() && cfg!(windows) {
            Shell::PowerShell
        } else {
            Shell::Posix
        }
    }
}

pub fn render_activate(env: &Env, shell: Shell) -> Result<String> {
    let prefix = env.root.to_string_lossy().to_string();
    let os_dir = paths::os_subdir();
    let mut out = String::new();

    // Collected PATH additions: per-OS first, then [activation.all].
    let mut path_adds: Vec<String> = Vec::new();
    path_adds.push(format!("{prefix}/{os_dir}/bin"));
    if let Some(b) = env.manifest.activation.get(os_dir) {
        for p in &b.path_prepend {
            path_adds.push(format!("{prefix}/{p}"));
        }
    }
    if let Some(b) = env.manifest.activation.get("all") {
        for p in &b.path_prepend {
            path_adds.push(format!("{prefix}/{p}"));
        }
    }

    match shell {
        Shell::Posix => render_posix(&mut out, &env.manifest.name, &prefix, &env.root, &path_adds, &env.manifest.activation)?,
        Shell::PowerShell => render_powershell(&mut out, &env.manifest.name, &prefix, &env.root, &path_adds, &env.manifest.activation)?,
    }
    Ok(out)
}

fn render_posix(
    out: &mut String,
    name: &str,
    prefix: &str,
    env_root: &Path,
    path_adds: &[String],
    activation: &std::collections::BTreeMap<String, ActivationBlock>,
) -> Result<()> {
    out.push_str(&format!("export _TOOLBOX_OLD_PATH=\"$PATH\"\n"));
    let joined = path_adds.join(":");
    out.push_str(&format!("export PATH=\"{joined}:$PATH\"\n"));
    out.push_str(&format!("export TOOLBOX_PREFIX=\"{prefix}\"\n"));
    out.push_str(&format!("export TOOLBOX_ACTIVE_ENV=\"{name}\"\n"));
    for key in ["all", paths::os_subdir()] {
        if let Some(b) = activation.get(key) {
            for (k, v) in &b.env {
                let v = crate::activation_vars::resolve(v, env_root)?;
                out.push_str(&format!("export {k}=\"{v}\"\n"));
            }
        }
    }
    Ok(())
}

fn render_powershell(
    out: &mut String,
    name: &str,
    prefix: &str,
    env_root: &Path,
    path_adds: &[String],
    activation: &std::collections::BTreeMap<String, ActivationBlock>,
) -> Result<()> {
    out.push_str("$env:_TOOLBOX_OLD_PATH = $env:PATH\n");
    let joined = path_adds.join(";");
    out.push_str(&format!("$env:PATH = \"{joined};$env:PATH\"\n"));
    out.push_str(&format!("$env:TOOLBOX_PREFIX = \"{prefix}\"\n"));
    out.push_str(&format!("$env:TOOLBOX_ACTIVE_ENV = \"{name}\"\n"));
    for key in ["all", paths::os_subdir()] {
        if let Some(b) = activation.get(key) {
            for (k, v) in &b.env {
                let v = crate::activation_vars::resolve(v, env_root)?;
                out.push_str(&format!("$env:{k} = \"{v}\"\n"));
            }
        }
    }
    Ok(())
}

pub fn render_deactivate(shell: Shell) -> String {
    match shell {
        Shell::Posix => "\
if [ -n \"$_TOOLBOX_OLD_PATH\" ]; then
  export PATH=\"$_TOOLBOX_OLD_PATH\"
  unset _TOOLBOX_OLD_PATH
fi
unset TOOLBOX_PREFIX
unset TOOLBOX_ACTIVE_ENV
"
        .into(),
        Shell::PowerShell => "\
if ($env:_TOOLBOX_OLD_PATH) {
  $env:PATH = $env:_TOOLBOX_OLD_PATH
  Remove-Item Env:_TOOLBOX_OLD_PATH
}
Remove-Item Env:TOOLBOX_PREFIX -ErrorAction SilentlyContinue
Remove-Item Env:TOOLBOX_ACTIVE_ENV -ErrorAction SilentlyContinue
"
        .into(),
    }
}

/// Mutate `cmd`'s environment so that spawning it behaves as if the env were
/// activated in the current shell. Used by `toolbox run`.
pub fn apply_to_command(env: &Env, cmd: &mut std::process::Command) -> Result<()> {
    let prefix = env.root.to_string_lossy().to_string();
    let os_dir = paths::os_subdir();

    let mut all_paths = path_adds(env);
    if let Some(current) = std::env::var_os("PATH") {
        all_paths.extend(std::env::split_paths(&current));
    }
    let new_path = std::env::join_paths(&all_paths).context("joining PATH entries")?;
    cmd.env("PATH", new_path);

    cmd.env("TOOLBOX_PREFIX", &prefix);
    cmd.env("TOOLBOX_ACTIVE_ENV", &env.manifest.name);

    for key in ["all", os_dir] {
        if let Some(b) = env.manifest.activation.get(key) {
            for (k, v) in &b.env {
                cmd.env(k, crate::activation_vars::resolve(v, &env.root)?);
            }
        }
    }
    Ok(())
}

/// Directories the env contributes to PATH, in prepend order:
/// per-OS `bin`, then `[activation.<os>].path_prepend`, then
/// `[activation.all].path_prepend`.
fn path_adds(env: &Env) -> Vec<PathBuf> {
    let os_dir = paths::os_subdir();
    let mut out = vec![env.root.join(os_dir).join("bin")];
    if let Some(b) = env.manifest.activation.get(os_dir) {
        for p in &b.path_prepend {
            out.push(env.root.join(p));
        }
    }
    if let Some(b) = env.manifest.activation.get("all") {
        for p in &b.path_prepend {
            out.push(env.root.join(p));
        }
    }
    out
}

/// Resolve `program` to an absolute path the way the shell would, but using
/// the env's prepended PATH first. Returns `None` if not found.
///
/// On Windows, also tries each `PATHEXT` extension. If `program` already
/// contains a path separator, it is treated as a path and returned as-is when
/// the file exists.
pub fn resolve_program(env: &Env, program: &str) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.is_absolute() || program.contains('/') || program.contains('\\') {
        return if p.is_file() { Some(p.to_path_buf()) } else { None };
    }

    let mut search = path_adds(env);
    if let Some(current) = std::env::var_os("PATH") {
        search.extend(std::env::split_paths(&current));
    }

    #[cfg(windows)]
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .map(|s| s.to_string())
        .collect();
    #[cfg(not(windows))]
    let exts: Vec<String> = vec![String::new()];

    let prog_lower = program.to_lowercase();
    for dir in &search {
        for ext in &exts {
            let ext_lower = ext.to_lowercase();
            let cand = if ext_lower.is_empty() || prog_lower.ends_with(&ext_lower) {
                dir.join(program)
            } else {
                dir.join(format!("{program}{ext_lower}"))
            };
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Shell init snippet that defines a `toolbox` shell function which calls the
/// binary and `eval`s its output for `activate`/`deactivate` subcommands.
pub fn shell_init(shell: Shell, toolbox_bin: &Path) -> String {
    let bin = toolbox_bin.to_string_lossy();
    match shell {
        Shell::Posix => format!(
            "toolbox() {{
  case \"$1\" in
    activate|deactivate)
      eval \"$('{bin}' \"$@\")\"
      ;;
    *)
      '{bin}' \"$@\"
      ;;
  esac
}}
"
        ),
        Shell::PowerShell => format!(
            "function toolbox {{
  param([Parameter(ValueFromRemainingArguments=$true)] $args)
  if ($args.Count -gt 0 -and ($args[0] -eq 'activate' -or $args[0] -eq 'deactivate')) {{
    $script = & '{bin}' @args
    Invoke-Expression ($script -join \"`n\")
  }} else {{
    & '{bin}' @args
  }}
}}
"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    fn fake_env(root: PathBuf, name: &str) -> Env {
        Env {
            root,
            manifest: Manifest {
                name: name.into(),
                version: "0.0.0".into(),
                description: None,
                packages: vec![],
                activation: BTreeMap::new(),
                tools: BTreeMap::new(),
            },
        }
    }

    #[test]
    fn apply_to_command_sets_prefix_and_active_env() {
        let tmp = tempfile::tempdir().unwrap();
        let env = fake_env(tmp.path().to_path_buf(), "myenv");
        let mut cmd = std::process::Command::new("true");
        apply_to_command(&env, &mut cmd).unwrap();

        let envs: BTreeMap<OsString, Option<OsString>> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|s| s.to_os_string())))
            .collect();

        assert_eq!(
            envs.get(OsString::from("TOOLBOX_PREFIX").as_os_str())
                .and_then(|v| v.clone()),
            Some(OsString::from(tmp.path().to_string_lossy().to_string()))
        );
        assert_eq!(
            envs.get(OsString::from("TOOLBOX_ACTIVE_ENV").as_os_str())
                .and_then(|v| v.clone()),
            Some(OsString::from("myenv"))
        );
        let new_path = envs
            .get(OsString::from("PATH").as_os_str())
            .and_then(|v| v.clone())
            .expect("PATH should be set");
        let first_dir = std::env::split_paths(&new_path).next().unwrap();
        assert_eq!(first_dir, tmp.path().join(paths::os_subdir()).join("bin"));
    }

    #[test]
    fn apply_to_command_substitutes_prefix_in_manifest_env() {
        let tmp = tempfile::tempdir().unwrap();
        let mut env = fake_env(tmp.path().to_path_buf(), "e");
        let mut block = ActivationBlock::default();
        block
            .env
            .insert("DATA_DIR".into(), "$TOOLBOX_PREFIX/share/data".into());
        env.manifest.activation.insert("all".into(), block);

        let mut cmd = std::process::Command::new("true");
        apply_to_command(&env, &mut cmd).unwrap();

        let data_dir = cmd
            .get_envs()
            .find(|(k, _)| k == &std::ffi::OsStr::new("DATA_DIR"))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().to_string())
            .unwrap();
        let want = format!("{}/share/data", tmp.path().to_string_lossy());
        assert_eq!(data_dir, want);
    }

    #[test]
    fn resolve_program_finds_executable_in_env_bin() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join(paths::os_subdir()).join("bin");
        std::fs::create_dir_all(&bin).unwrap();

        #[cfg(windows)]
        let (file, query) = ("tool.exe", "tool");
        #[cfg(not(windows))]
        let (file, query) = ("tool", "tool");

        std::fs::write(bin.join(file), b"").unwrap();

        let env = fake_env(tmp.path().to_path_buf(), "e");
        let resolved = resolve_program(&env, query).expect("should resolve");
        assert_eq!(resolved, bin.join(file));
    }

    #[test]
    fn resolve_program_returns_none_for_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(paths::os_subdir()).join("bin")).unwrap();
        let env = fake_env(tmp.path().to_path_buf(), "e");
        assert!(resolve_program(&env, "definitely-not-a-real-binary-xyz123").is_none());
    }
}
