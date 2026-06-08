use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Root of the per-machine ToolBox installation.
///
/// Windows: %LOCALAPPDATA%\ToolBox
/// Linux:   $XDG_DATA_HOME/toolbox  (or ~/.local/share/toolbox)
/// macOS:   ~/Library/Application Support/ToolBox
pub fn install_root() -> Result<PathBuf> {
    if let Ok(over) = std::env::var("TOOLBOX_HOME") {
        return Ok(PathBuf::from(over));
    }
    #[cfg(windows)]
    {
        dirs::data_local_dir()
            .map(|d| d.join("ToolBox"))
            .ok_or_else(|| anyhow!("could not resolve %LOCALAPPDATA%"))
    }
    #[cfg(not(windows))]
    {
        dirs::data_dir()
            .map(|d| d.join("toolbox"))
            .ok_or_else(|| anyhow!("could not resolve data dir"))
    }
}

pub fn registry_path() -> Result<PathBuf> {
    Ok(install_root()?.join("registry.tomlp"))
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(install_root()?.join("cache"))
}

/// Subdir within an env that holds per-OS binaries.
pub const fn os_subdir() -> &'static str {
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
}
