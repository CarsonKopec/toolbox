//! Exercises the `toolbox config` activation-editing surface end to end:
//! set/unset env vars, add/remove PATH entries, show, and confirm the edits
//! flow through to the rendered activation script.

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
fn config_edits_activation_and_renders() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "cfgenv"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));

    // Set a plain env var, a template env var, and a PATH addition.
    run(toolbox(&home).args(["config", "set-env", "cfgenv", "GREETING", "hello"]));
    run(toolbox(&home).args([
        "config",
        "set-env",
        "cfgenv",
        "EDITOR",
        "$ENV.EDITOR ?? \"vim\"",
    ]));
    run(toolbox(&home).args(["config", "add-path", "cfgenv", "share/bin"]));

    // `show` reflects all three; the template value keeps its inner quotes
    // (proves the round trip preserves embedded `"`).
    let shown = run(toolbox(&home).args(["config", "show", "cfgenv"]));
    assert!(shown.contains("GREETING = hello"), "{shown}");
    assert!(
        shown.contains("EDITOR = $ENV.EDITOR ?? \"vim\""),
        "embedded quotes should round-trip: {shown}"
    );
    assert!(shown.contains("PATH += share/bin"), "{shown}");

    // The rendered activation script reflects the edits, with the template
    // resolving against the host environment (EDITOR=nano here).
    let script = run(toolbox(&home)
        .env("EDITOR", "nano")
        .args(["activate", "cfgenv", "--shell", "posix"]));
    assert!(script.contains("GREETING=\"hello\""), "{script}");
    assert!(script.contains("EDITOR=\"nano\""), "{script}");
    assert!(script.contains("share/bin"), "{script}");

    // Remove the plain var and the PATH entry; the template var stays.
    run(toolbox(&home).args(["config", "unset-env", "cfgenv", "GREETING"]));
    run(toolbox(&home).args(["config", "remove-path", "cfgenv", "share/bin"]));

    let shown = run(toolbox(&home).args(["config", "show", "cfgenv"]));
    assert!(!shown.contains("GREETING"), "{shown}");
    assert!(!shown.contains("share/bin"), "{shown}");
    assert!(shown.contains("EDITOR ="), "template var should remain: {shown}");

    // Removing something that isn't there is an error.
    let out = toolbox(&home)
        .args(["config", "unset-env", "cfgenv", "NOPE"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "unset of missing key should fail");
}
