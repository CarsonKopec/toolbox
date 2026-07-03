//! `toolbox update` re-installs a package from its recorded source. Verified by
//! changing a local package's contents + version, then updating.
//!
//! NB: this file is named `repull`, not `update` — on Windows, Cargo's test
//! binary inherits the file name, and the UAC installer-detection heuristic
//! refuses to run any executable whose name contains "update"/"install"/"setup"
//! without elevation (os error 740).

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

fn write_package(pkg: &Path, version: &str, payload: &str) {
    std::fs::create_dir_all(pkg.join("share")).unwrap();
    std::fs::write(pkg.join("share").join("data.txt"), payload).unwrap();
    std::fs::write(
        pkg.join("toolbox-package.tomlp"),
        format!("name = \"mypkg\"\nversion = \"{version}\"\n"),
    )
    .unwrap();
}

#[test]
fn update_repulls_from_local_source() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let env_dir = tmp.path().join("env");
    let pkg = tmp.path().join("pkg");

    write_package(&pkg, "1.0.0", "version one");

    run(toolbox(&home).args(["init", env_dir.to_str().unwrap(), "--name", "dev"]));
    run(toolbox(&home).args(["register", env_dir.to_str().unwrap()]));
    run(toolbox(&home).args(["install", "--from", pkg.to_str().unwrap(), "-e", "dev"]));

    let data = env_dir.join("share").join("data.txt");
    assert_eq!(std::fs::read_to_string(&data).unwrap(), "version one");

    // v1 also ships a file that v2 will drop, to exercise pruning.
    std::fs::write(pkg.join("share").join("gone.txt"), "obsolete").unwrap();
    run(toolbox(&home).args(["update", "mypkg", "-e", "dev"])); // re-record with gone.txt
    let stale = env_dir.join("share").join("gone.txt");
    assert!(stale.exists(), "setup: gone.txt should be installed");

    // Change the package on disk (new payload + version, and drop gone.txt).
    write_package(&pkg, "2.0.0", "version two");
    std::fs::remove_file(pkg.join("share").join("gone.txt")).unwrap();
    let out = run(toolbox(&home).args(["update", "mypkg", "-e", "dev"]));
    assert!(out.contains("Updated 1 package"), "{out}");

    // The new payload and version landed in the env.
    assert_eq!(std::fs::read_to_string(&data).unwrap(), "version two");
    let manifest = std::fs::read_to_string(env_dir.join("toolbox-env.tomlp")).unwrap();
    assert!(manifest.contains("2.0.0"), "version not updated: {manifest}");
    // The file dropped between versions was pruned.
    assert!(!stale.exists(), "gone.txt should have been pruned by update");
}
