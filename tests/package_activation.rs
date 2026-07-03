//! End-to-end: a package that declares activation via `toolbox-package.tomlp`
//! gets those contributions merged into the env manifest on `install --from`,
//! and they show up (with `$TOOLBOX_PREFIX` resolved) in the activation script.

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
fn package_declared_activation_is_merged_and_rendered() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");
    let pkg = tmp.path().join("pkg");

    // A package tree with overlay content plus a package manifest declaring
    // activation contributions (a PATH addition and an env var).
    std::fs::create_dir_all(pkg.join("share").join("scripts")).unwrap();
    std::fs::write(
        pkg.join("share").join("scripts").join("tool.sh"),
        "#!/bin/sh\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("toolbox-package.tomlp"),
        r#"name = "pytools"
version = "1.0.0"

[activation]
all = #{ path_prepend = ["share/scripts"], env = #{ PYTHONHOME = "$TOOLBOX_PREFIX/share/py" }# }#

[tools]
greet = #{ run = "python", args = ["$TOOLBOX_PREFIX/share/scripts/greet.py"] }#
"#,
    )
    .unwrap();

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "pyenv"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));
    run(toolbox(&home).args(["install", "--from", pkg.to_str().unwrap(), "-e", "pyenv"]));

    // The env manifest absorbed the package's activation and its tool.
    let manifest = std::fs::read_to_string(env_dir.join("toolbox-env.tomlp")).unwrap();
    assert!(manifest.contains("PYTHONHOME"), "manifest: {manifest}");
    assert!(manifest.contains("share/scripts"), "manifest: {manifest}");
    assert!(manifest.contains("[tools]"), "tool not merged: {manifest}");
    assert!(manifest.contains("greet"), "tool not merged: {manifest}");
    assert!(
        manifest.contains("greet.py"),
        "tool args not merged: {manifest}"
    );

    // The activation script renders the env var with $TOOLBOX_PREFIX resolved,
    // and adds the declared dir to PATH.
    let script = run(toolbox(&home).args(["activate", "pyenv", "--shell", "posix"]));
    assert!(script.contains("PYTHONHOME"), "script: {script}");
    assert!(script.contains("share/py"), "script: {script}");
    assert!(
        !script.contains("$TOOLBOX_PREFIX/share/py"),
        "the sentinel should be resolved in the rendered script: {script}"
    );
    assert!(
        script.contains("share/scripts"),
        "PATH addition missing: {script}"
    );
}
