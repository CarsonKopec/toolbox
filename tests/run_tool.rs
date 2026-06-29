//! `toolbox run <env> <tool>` runs a declared tool; unknown tools error; raw
//! `run <env> -- <cmd>` still works. Uses the toolbox binary itself as a
//! portable, deterministic tool program (`toolbox --version`).

use std::path::Path;
use std::process::Command;

fn toolbox(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_toolbox"));
    c.env("TOOLBOX_HOME", home);
    c
}

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().expect("spawn toolbox");
    assert!(
        out.status.success(),
        "command {:?} failed (status {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        cmd,
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn runs_declared_tool_raw_and_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "tenv"]));

    // Declare a tool that runs `toolbox --version`. Forward slashes avoid TOML+
    // escaping of Windows backslashes.
    let exe = env!("CARGO_BIN_EXE_toolbox").replace('\\', "/");
    let manifest_path = env_dir.join("toolbox-env.tomlp");
    let mut manifest = std::fs::read_to_string(&manifest_path).unwrap();
    manifest.push_str(&format!(
        "\n[tools]\nver = #{{ run = \"{exe}\", args = [\"--version\"] }}#\n"
    ));
    std::fs::write(&manifest_path, manifest).unwrap();

    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));

    // Declared tool runs and produces the version banner.
    let out = run(toolbox(&home).args(["run", "tenv", "ver"]));
    assert!(out.contains("toolbox"), "tool output: {out}");

    // A non-tool first arg is run as a raw program (here the binary directly).
    let raw = run(toolbox(&home).args(["run", "tenv", &exe, "--version"]));
    assert!(raw.contains("toolbox"), "raw output: {raw}");

    // An unknown name that's neither a tool nor on PATH is a clear error that
    // lists the declared tools.
    let bad = toolbox(&home)
        .args(["run", "tenv", "nope-not-a-real-thing-xyz"])
        .output()
        .unwrap();
    assert!(!bad.status.success(), "unknown command should fail");
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(stderr.contains("not a declared tool"), "stderr: {stderr}");
    assert!(stderr.contains("ver"), "error should list declared tools: {stderr}");
}

#[test]
fn tool_lifecycle_via_cli() {
    // Declare, run, and remove a tool entirely through the CLI — no manifest
    // editing. The tool runs `toolbox --version`.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");
    let exe = env!("CARGO_BIN_EXE_toolbox");

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "tenv"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));

    // Add a tool from the CLI.
    run(toolbox(&home).args([
        "config", "add-tool", "tenv", "ver", "--run", exe, "--arg", "--version",
    ]));

    // It shows up and runs.
    let shown = run(toolbox(&home).args(["config", "show", "tenv"]));
    assert!(shown.contains("ver:"), "tool not shown: {shown}");
    let out = run(toolbox(&home).args(["run", "tenv", "ver"]));
    assert!(out.contains("toolbox"), "tool output: {out}");

    // Remove it; it's gone.
    run(toolbox(&home).args(["config", "remove-tool", "tenv", "ver"]));
    let gone = toolbox(&home).args(["run", "tenv", "ver"]).output().unwrap();
    assert!(!gone.status.success(), "removed tool should not run");
}
