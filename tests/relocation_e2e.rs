//! End-to-end proof of the core thesis: an env whose files carry the
//! `__TOOLBOX_PREFIX__` sentinel keeps working after the env directory is moved
//! to a new path. Drives the real `toolbox` binary through the full loop:
//! init → register → install (local) → move dir → re-register → activate.

use std::path::Path;
use std::process::Command;

/// A `toolbox` invocation with an isolated per-machine home.
fn toolbox(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_toolbox"));
    c.env("TOOLBOX_HOME", home);
    c
}

/// Run a command, asserting success and returning stdout.
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
fn env_relocates_after_directory_move() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let alpha = tmp.path().join("alpha"); // first mount point
    let beta = tmp.path().join("beta"); // mount point after the move
    let pkg = tmp.path().join("pkg");

    // A package tree with the relocation sentinel baked into a shared file.
    // `share/` is OS-independent, so this test behaves identically everywhere.
    std::fs::create_dir_all(pkg.join("share")).unwrap();
    std::fs::write(
        pkg.join("share").join("config.txt"),
        "data_dir=__TOOLBOX_PREFIX__/share\n",
    )
    .unwrap();

    let config_rel = Path::new("share").join("config.txt");

    // init → register → install from the local package tree.
    run(toolbox(&home).args(["init", alpha.to_str().unwrap(), "--name", "demo"]));
    run(toolbox(&home).args(["register", alpha.to_str().unwrap()]));
    run(toolbox(&home).args(["install", "--from", pkg.to_str().unwrap(), "-e", "demo"]));

    // After install the sentinel is patched to the current mount (alpha).
    let after_install = std::fs::read_to_string(alpha.join(&config_rel)).unwrap();
    assert!(
        !after_install.contains("__TOOLBOX_PREFIX__"),
        "sentinel should be patched away on install, got: {after_install}"
    );
    assert!(
        after_install.contains("alpha"),
        "expected the alpha path, got: {after_install}"
    );

    // Move the entire env directory to a new location and tell toolbox about it.
    std::fs::rename(&alpha, &beta).unwrap();
    run(toolbox(&home).args(["unregister", "demo"]));
    run(toolbox(&home).args(["register", beta.to_str().unwrap()]));

    // Activating auto-relocates: last-prefix (alpha) != current mount (beta).
    run(toolbox(&home).args(["activate", "demo", "--shell", "posix"]));

    // The file is now patched to beta, with no trace of the old alpha path.
    let after_move = std::fs::read_to_string(beta.join(&config_rel)).unwrap();
    assert!(
        after_move.contains("beta"),
        "expected the beta path after relocation, got: {after_move}"
    );
    assert!(
        !after_move.contains("alpha"),
        "old alpha path should be gone after relocation, got: {after_move}"
    );
    assert!(
        !after_move.contains("__TOOLBOX_PREFIX__"),
        "sentinel should remain patched, got: {after_move}"
    );
}
