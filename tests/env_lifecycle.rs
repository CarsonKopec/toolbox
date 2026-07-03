//! `toolbox info` summarizes an env; `toolbox remove` deletes its directory and
//! unregisters it (refusing to delete a directory that isn't a toolbox env).

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
fn info_summarizes_and_remove_deletes() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");
    let pkg = tmp.path().join("pkg");

    // A package that contributes a tool + an activation var.
    std::fs::create_dir_all(pkg.join("share")).unwrap();
    std::fs::write(pkg.join("share").join("f.txt"), "x").unwrap();
    std::fs::write(
        pkg.join("toolbox-package.tomlp"),
        "name = \"demo\"\nversion = \"2.0.0\"\n\n\
         [activation]\nall = #{ env = #{ FOO = \"bar\" }# }#\n\n\
         [tools]\nhi = #{ run = \"echo\" }#\n",
    )
    .unwrap();

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "dev"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));
    run(toolbox(&home).args(["install", "--from", pkg.to_str().unwrap(), "-e", "dev"]));

    // info surfaces packages, tools, and activation.
    let out = run(toolbox(&home).args(["info", "dev"]));
    assert!(out.contains("demo 2.0.0"), "package missing: {out}");
    assert!(out.contains("hi:"), "tool missing: {out}");
    assert!(out.contains("env var"), "activation missing: {out}");

    // remove deletes the directory and drops it from the registry.
    run(toolbox(&home).args(["remove", "dev"]));
    assert!(!env_dir.exists(), "env directory should be deleted");
    let list = run(toolbox(&home).args(["list"]));
    assert!(!list.contains("dev"), "env should be unregistered: {list}");

    // removing again fails (no longer registered).
    let again = toolbox(&home).args(["remove", "dev"]).output().unwrap();
    assert!(!again.status.success(), "removing an unregistered env should fail");
}

#[test]
fn remove_refuses_non_env_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "dev"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));

    // Delete the manifest so the directory no longer looks like an env, then try
    // to remove — it must refuse rather than delete an unrelated directory.
    std::fs::remove_file(env_dir.join("toolbox-env.tomlp")).unwrap();
    let out = toolbox(&home).args(["remove", "dev"]).output().unwrap();
    assert!(!out.status.success(), "remove should refuse a non-env dir");
    assert!(String::from_utf8_lossy(&out.stderr).contains("refusing to delete"));
    assert!(env_dir.exists(), "directory must not be deleted");
}
