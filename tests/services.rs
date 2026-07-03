//! Background services: `start` a declared tool, see it `running` in `status`,
//! then `stop` it. The service is a long-running `toolbox __sleep`, so this
//! exercises the detached spawn, liveness check, and tree kill on each OS.

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
fn service_start_status_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");
    let exe = env!("CARGO_BIN_EXE_toolbox");

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "dev"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));
    // A service that runs `toolbox __sleep 60000` in the background.
    run(toolbox(&home).args([
        "config", "add-tool", "dev", "sleeper", "--run", exe, "--arg", "__sleep", "--arg", "60000",
    ]));

    // Don't capture `start`'s stdout via a pipe: on Windows the detached child
    // inherits the parent's captured pipe handle, which would keep it open for
    // the service's lifetime. Interactive/console use is unaffected.
    let started = toolbox(&home)
        .args(["start", "dev", "sleeper"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn start");
    assert!(started.success(), "start should succeed");

    let status = run(toolbox(&home).args(["status", "dev"]));
    assert!(status.contains("sleeper"), "status: {status}");
    assert!(
        status.contains("running"),
        "service should be running: {status}"
    );

    // Starting again while running is refused.
    let dup = toolbox(&home)
        .args(["start", "dev", "sleeper"])
        .output()
        .unwrap();
    assert!(!dup.status.success(), "second start should fail");

    run(toolbox(&home).args(["stop", "dev", "sleeper"]));

    let after = run(toolbox(&home).args(["status", "dev"]));
    assert!(
        !after.contains("running"),
        "service should be stopped: {after}"
    );
}

#[test]
fn service_restart_always_respawns() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");
    let exe = env!("CARGO_BIN_EXE_toolbox");

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "dev"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));
    // A tool that exits every 300ms, with restart=always.
    run(toolbox(&home).args([
        "config",
        "add-tool",
        "dev",
        "flaky",
        "--run",
        exe,
        "--arg",
        "__sleep",
        "--arg",
        "300",
        "--restart",
        "always",
    ]));

    toolbox(&home)
        .args(["start", "dev", "flaky"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn start");

    // After more than one child lifetime, the supervisor should still be alive
    // (it keeps respawning), and the log should record restarts.
    std::thread::sleep(std::time::Duration::from_millis(1500));

    let status = run(toolbox(&home).args(["status", "dev"]));
    assert!(
        status.contains("running"),
        "restart=always service should still be running: {status}"
    );
    let logs = run(toolbox(&home).args(["logs", "dev", "flaky"]));
    assert!(
        logs.contains("restarting"),
        "log should show restarts: {logs}"
    );

    run(toolbox(&home).args(["stop", "dev", "flaky"]));
    let after = run(toolbox(&home).args(["status", "dev"]));
    assert!(!after.contains("running"), "should be stopped: {after}");
}
