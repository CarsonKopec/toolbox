//! `toolbox which` resolves a declared tool or a program to its path within an
//! env. The env defaults to TOOLBOX_ACTIVE_ENV, which is the callback path a
//! running tool would use to find a sibling.

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
fn which_resolves_tool_via_flag_and_active_env() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");
    let exe = env!("CARGO_BIN_EXE_toolbox");

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "dev"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));
    run(toolbox(&home).args(["config", "add-tool", "dev", "ver", "--run", exe]));

    // Explicit --env: prints the resolved program path (just the path).
    let out = run(toolbox(&home).args(["which", "--env", "dev", "ver"]));
    assert_eq!(out.trim(), exe, "which should print the exact program path");

    // Callback path: env defaults to TOOLBOX_ACTIVE_ENV (as set while a tool runs).
    let out2 = run(toolbox(&home)
        .env("TOOLBOX_ACTIVE_ENV", "dev")
        .args(["which", "ver"]));
    assert_eq!(out2.trim(), exe);

    // Unknown name errors.
    let bad = toolbox(&home)
        .args(["which", "--env", "dev", "nope-not-real-xyz"])
        .output()
        .unwrap();
    assert!(!bad.status.success(), "unknown name should fail");

    // No env and no TOOLBOX_ACTIVE_ENV errors clearly.
    let no_env = toolbox(&home)
        .env_remove("TOOLBOX_ACTIVE_ENV")
        .args(["which", "ver"])
        .output()
        .unwrap();
    assert!(!no_env.status.success());
    assert!(String::from_utf8_lossy(&no_env.stderr).contains("TOOLBOX_ACTIVE_ENV"));
}
