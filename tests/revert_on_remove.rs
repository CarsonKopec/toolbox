//! Uninstall reverts the activation + tools a package contributed, but leaves
//! anything another installed package still relies on.
//!
//! NB: named `revert_on_remove`, not `uninstall_*` — Windows UAC refuses to run
//! a test binary whose name contains "install"/"update"/"setup" (os error 740).

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

/// A package tree with a share/ file (so it has overlay content) and a manifest.
fn make_pkg(dir: &Path, manifest: &str) {
    std::fs::create_dir_all(dir.join("share").join("common")).unwrap();
    std::fs::write(dir.join("share").join("common").join("f.txt"), "x").unwrap();
    std::fs::write(dir.join("toolbox-package.tomlp"), manifest).unwrap();
}

#[test]
fn uninstall_reverts_contributions_but_keeps_shared() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");
    let pkg_a = tmp.path().join("pkg_a");
    let pkg_b = tmp.path().join("pkg_b");

    // A contributes an env var, a tool, and a shared PATH entry.
    make_pkg(
        &pkg_a,
        "name = \"pkg-a\"\nversion = \"1.0.0\"\n\n\
         [activation]\nall = #{ path_prepend = [\"share/common\"], env = #{ A_VAR = \"1\" }# }#\n\n\
         [tools]\natool = #{ run = \"echo\" }#\n",
    );
    // B contributes only the same shared PATH entry.
    make_pkg(
        &pkg_b,
        "name = \"pkg-b\"\nversion = \"1.0.0\"\n\n\
         [activation]\nall = #{ path_prepend = [\"share/common\"] }#\n",
    );

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "dev"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));
    run(toolbox(&home).args(["install", "--from", pkg_a.to_str().unwrap(), "-e", "dev"]));
    run(toolbox(&home).args(["install", "--from", pkg_b.to_str().unwrap(), "-e", "dev"]));

    let manifest = || std::fs::read_to_string(env_dir.join("toolbox-env.tomlp")).unwrap();
    let m = manifest();
    assert!(
        m.contains("A_VAR") && m.contains("atool") && m.contains("share/common"),
        "{m}"
    );

    // Uninstall A: its env var + tool go away, but the shared PATH entry stays
    // (B still needs it).
    run(toolbox(&home).args(["uninstall", "pkg-a", "-e", "dev"]));
    let m = manifest();
    assert!(!m.contains("A_VAR"), "A_VAR should be reverted: {m}");
    assert!(!m.contains("atool"), "atool should be reverted: {m}");
    assert!(
        m.contains("share/common"),
        "shared PATH entry must remain: {m}"
    );

    // Uninstall B: now nothing needs the shared entry, so activation is empty.
    run(toolbox(&home).args(["uninstall", "pkg-b", "-e", "dev"]));
    let m = manifest();
    assert!(
        !m.contains("share/common"),
        "shared PATH entry should be gone: {m}"
    );
    assert!(
        !m.contains("[activation]"),
        "activation should be empty: {m}"
    );
}
