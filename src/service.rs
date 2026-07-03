//! Background services: state tracking and the cross-platform process
//! primitives (detached spawn, liveness check, tree kill) behind `start` /
//! `stop` / `status` / `logs`.
//!
//! There is no central daemon: `start` spawns the tool detached, records a small
//! state file under `<install_root>/run/<env>/<tool>.json`, and streams the
//! process's output to a sibling `.log`. `stop`/`status` operate on that state.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths;

/// Recorded state for one running (or last-known) service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceState {
    pub env: String,
    pub tool: String,
    pub pid: u32,
    pub log: PathBuf,
    /// Seconds since the Unix epoch when the service was started.
    pub started_at: u64,
}

impl ServiceState {
    fn state_path(env: &str, tool: &str) -> Result<PathBuf> {
        Ok(paths::run_dir()?
            .join(sanitize(env))
            .join(format!("{}.json", sanitize(tool))))
    }

    /// Path of the log file a service writes to (independent of whether it's
    /// currently running).
    pub fn log_path(env: &str, tool: &str) -> Result<PathBuf> {
        Ok(paths::run_dir()?
            .join(sanitize(env))
            .join(format!("{}.log", sanitize(tool))))
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::state_path(&self.env, &self.tool)?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", p.display()))?;
        Ok(())
    }

    pub fn load(env: &str, tool: &str) -> Result<Option<Self>> {
        let p = Self::state_path(env, tool)?;
        if !p.exists() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Ok(Some(
            serde_json::from_str(&s).with_context(|| format!("parsing {}", p.display()))?,
        ))
    }

    pub fn remove(env: &str, tool: &str) -> Result<()> {
        let p = Self::state_path(env, tool)?;
        match std::fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", p.display())),
        }
    }

    /// All recorded services for an env (running or stale).
    pub fn all_for_env(env: &str) -> Result<Vec<Self>> {
        let dir = paths::run_dir()?.join(sanitize(env));
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(s) = std::fs::read_to_string(&path) {
                    if let Ok(st) = serde_json::from_str(&s) {
                        out.push(st);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.tool.cmp(&b.tool));
        Ok(out)
    }

    /// Uptime in seconds, or 0 if the clock appears to have gone backwards.
    pub fn uptime_secs(&self) -> u64 {
        now_secs().saturating_sub(self.started_at)
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Spawn `cmd` detached from this process so it keeps running after `toolbox`
/// exits. On Windows this uses `DETACHED_PROCESS`; on Unix the child becomes a
/// new process-group leader (so its whole tree can be signalled on stop).
pub fn spawn_detached(cmd: &mut Command) -> Result<Child> {
    // Windows: we intentionally set no custom creation flags. Overriding them
    // disables std's restricted handle inheritance (the STARTUPINFOEX handle
    // list), which would then leak a parent's captured stdout/stderr pipe into
    // the service and hold it open for the service's whole lifetime. A plain
    // spawn with redirected stdio still outlives `toolbox` (Windows does not
    // cascade-kill child processes), and `taskkill /T` stops the tree on stop.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group, so the child survives `toolbox` exiting and its
        // whole tree can be signalled on stop.
        cmd.process_group(0);
    }
    Ok(cmd.spawn()?)
}

/// Whether a process with `pid` is currently alive.
pub fn is_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        // `tasklist` prints "No tasks..." to stdout when nothing matches.
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
    #[cfg(unix)]
    {
        // `kill -0` succeeds iff the process exists and is signalable.
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Terminate a process and its children.
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(unix)]
    {
        // The service is its own process-group leader, so a negative pid targets
        // the whole group. TERM first, then KILL after a short grace period.
        let group = format!("-{pid}");
        let _ = Command::new("kill").args(["-TERM", &group]).output();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = Command::new("kill").args(["-KILL", &group]).output();
    }
}

/// Make an env/tool name safe as a single path component.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect()
}

/// Read the last `lines` lines of a file (or the whole file if `lines` is None).
pub fn tail(path: &Path, lines: Option<usize>) -> Result<String> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    match lines {
        Some(n) => {
            let all: Vec<&str> = content.lines().collect();
            let start = all.len().saturating_sub(n);
            Ok(all[start..].join("\n"))
        }
        None => Ok(content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_returns_last_lines() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), "a\nb\nc\nd\n").unwrap();
        assert_eq!(tail(f.path(), Some(2)).unwrap(), "c\nd");
        assert_eq!(tail(f.path(), Some(10)).unwrap(), "a\nb\nc\nd");
        assert_eq!(tail(f.path(), None).unwrap(), "a\nb\nc\nd\n");
    }
}
