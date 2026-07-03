use anyhow::{anyhow, Context, Result};
use std::path::Path;

use crate::activate::{self, Shell};
use crate::cli::{Command, ConfigAction};
use crate::env::Env;
use crate::paths;
use crate::registry::Registry;

pub fn dispatch(cmd: Command) -> Result<()> {
    match cmd {
        Command::Init { path, name } => init(&path, name.as_deref()),
        Command::Install {
            package,
            env,
            from,
            name,
            version,
        } => match (package, from) {
            (Some(_), Some(_)) => Err(anyhow!(
                "pass either a package reference or --from, not both"
            )),
            (None, None) => Err(anyhow!("provide a package reference or --from <dir>")),
            (Some(pkg), None) => install(&pkg, &env),
            (None, Some(dir)) => install_from(&dir, &env, name.as_deref(), version.as_deref()),
        },
        Command::Uninstall { package, env } => uninstall(&package, &env),
        Command::Update { package, env } => update(package.as_deref(), &env),
        Command::Register { path, name } => register(&path, name.as_deref()),
        Command::Unregister { name } => unregister(&name),
        Command::Remove { name } => remove(&name),
        Command::List => list(),
        Command::Info { name } => info(&name),
        Command::Activate { name, shell } => activate_cmd(&name, &shell),
        Command::Deactivate { shell } => deactivate_cmd(&shell),
        Command::Run { name, cmd, args } => run_cmd(&name, &cmd, &args),
        Command::Which { name, env } => which(&name, env.as_deref()),
        Command::Start { env, tool } => start_service(&env, &tool),
        Command::Stop { env, tool } => stop_service(&env, &tool),
        Command::Status { env } => status_services(&env),
        Command::Logs { env, tool, lines } => logs_service(&env, &tool, lines),
        Command::Sleep { millis } => {
            std::thread::sleep(std::time::Duration::from_millis(millis));
            Ok(())
        }
        Command::Verify { name } => verify(&name),
        Command::Relocate { name } => relocate_cmd(&name),
        Command::PackIndex { path } => pack_index(&path),
        Command::Push {
            path,
            reference,
            name,
            version,
            platforms,
        } => push_cmd(&path, &reference, name, version, platforms),
        Command::Shellenv { shell } => shellenv(&shell),
        Command::Config { action } => config_dispatch(action),
    }
}

/// Load a registered env by name (registry lookup + manifest parse).
fn load_registered_env(name: &str) -> Result<Env> {
    let reg = Registry::load()?;
    let entry = reg.get(name)?;
    Env::load(&entry.path)
}

fn init(path: &Path, name: Option<&str>) -> Result<()> {
    let n = name
        .map(str::to_string)
        .or_else(|| path.file_name().map(|s| s.to_string_lossy().into_owned()))
        .ok_or_else(|| anyhow!("could not infer env name from path; pass --name"))?;
    let env = Env::create(path, &n)?;
    println!(
        "Initialized env '{}' at {}",
        env.manifest.name,
        env.root.display()
    );
    Ok(())
}

fn install(package: &str, env_name: &str) -> Result<()> {
    let reg = Registry::load()?;
    let entry = reg.get(env_name)?;
    let env = Env::load(&entry.path)?;

    eprintln!("toolbox: pulling {package}");
    let summary = crate::oci::pull(package, &env.root)?;
    eprintln!(
        "toolbox: extracted {} ({}) — {} layer(s)",
        summary.name, summary.version, summary.layer_count
    );

    finish_install(
        &env,
        env_name,
        &summary.name,
        &summary.version,
        package,
        &summary.files,
        &summary.activation,
        &summary.tools,
    )
}

fn install_from(
    dir: &Path,
    env_name: &str,
    name: Option<&str>,
    version: Option<&str>,
) -> Result<()> {
    let reg = Registry::load()?;
    let entry = reg.get(env_name)?;
    let env = Env::load(&entry.path)?;

    let abs = dunce::canonicalize(dir)?;

    // An optional package manifest in the tree supplies activation and defaults
    // for name/version; CLI flags override.
    let pkg_manifest = {
        let p = abs.join(crate::manifest::PACKAGE_MANIFEST_FILE);
        if p.exists() {
            Some(crate::manifest::Manifest::from_path(&p)?)
        } else {
            None
        }
    };

    let pkg_name = name
        .map(str::to_string)
        .or_else(|| pkg_manifest.as_ref().map(|m| m.name.clone()))
        .or_else(|| abs.file_name().map(|s| s.to_string_lossy().into_owned()))
        .ok_or_else(|| anyhow!("could not infer package name from path; pass --name"))?;
    let pkg_version = version
        .map(str::to_string)
        .or_else(|| pkg_manifest.as_ref().map(|m| m.version.clone()))
        .unwrap_or_else(|| "0.0.0".to_string());
    let activation = pkg_manifest
        .as_ref()
        .map(|m| m.activation.clone())
        .unwrap_or_default();
    let tools = pkg_manifest.map(|m| m.tools).unwrap_or_default();

    eprintln!("toolbox: installing from {}", abs.display());
    let files = crate::oci::extract_dir(&abs, &env.root)?;
    eprintln!("toolbox: copied {} file(s)", files.len());

    let source = format!("file://{}", abs.display());
    finish_install(
        &env,
        env_name,
        &pkg_name,
        &pkg_version,
        &source,
        &files,
        &activation,
        &tools,
    )
}

/// Shared tail of every install path: record the package in the manifest and
/// the per-package file list, merge the package's activation contributions,
/// then re-scan and patch relocation sentinels.
#[allow(clippy::too_many_arguments)]
fn finish_install(
    env: &Env,
    env_name: &str,
    name: &str,
    version: &str,
    source: &str,
    files: &[String],
    activation: &std::collections::BTreeMap<String, crate::manifest::ActivationBlock>,
    tools: &std::collections::BTreeMap<String, crate::manifest::Tool>,
) -> Result<()> {
    use crate::manifest::PackageRef;

    // Update the env manifest with this package reference.
    let mut manifest = env.manifest.clone();
    match manifest.packages.iter_mut().find(|p| p.name == name) {
        Some(p) => {
            p.version = version.to_string();
            p.source = source.to_string();
        }
        None => manifest.packages.push(PackageRef {
            name: name.to_string(),
            version: version.to_string(),
            source: source.to_string(),
        }),
    }
    merge_activation(&mut manifest.activation, activation);
    // Tools merge by name; a re-install overwrites the same tool.
    for (tname, tool) in tools {
        manifest.tools.insert(tname.clone(), tool.clone());
    }
    manifest.save(&env.root)?;

    // Record the files this package laid down (so `uninstall` removes exactly
    // those) plus the activation/tools it contributed (so uninstall can revert
    // them without disturbing what other packages still rely on).
    crate::installed::InstalledFiles {
        package: name.to_string(),
        version: version.to_string(),
        source: source.to_string(),
        files: files.to_vec(),
        activation: activation.clone(),
        tools: tools.clone(),
    }
    .save(&env.root)?;

    // Re-scan the env tree so newly-extracted files are in the relocate index.
    let idx = crate::relocate::scan_for_sentinel(&env.root)?;
    idx.save(&env.root)?;

    // Patch sentinels in the just-extracted files to the current mount path.
    // Use apply_with_prev(SENTINEL, ...) so already-relocated files are
    // left alone (they no longer contain the sentinel).
    if !idx.entries.is_empty() {
        crate::relocate::apply_with_prev(
            &env.root,
            &idx,
            crate::relocate::PREFIX_SENTINEL,
            &env.root,
        )?;
    }

    println!("Installed {name} {version} into {env_name}");
    Ok(())
}

/// Merge a package's activation contributions into the env's. `path_prepend`
/// entries are appended (de-duplicated); env vars are inserted, last writer
/// winning. Idempotent, so re-installing the same package is a no-op.
fn merge_activation(
    base: &mut std::collections::BTreeMap<String, crate::manifest::ActivationBlock>,
    add: &std::collections::BTreeMap<String, crate::manifest::ActivationBlock>,
) {
    for (os, block) in add {
        let entry = base.entry(os.clone()).or_default();
        for p in &block.path_prepend {
            if !entry.path_prepend.contains(p) {
                entry.path_prepend.push(p.clone());
            }
        }
        for (k, v) in &block.env {
            entry.env.insert(k.clone(), v.clone());
        }
    }
}

/// Undo a package's activation contributions, leaving anything another installed
/// package still provides (or that the user has since changed) in place.
fn revert_activation(
    base: &mut std::collections::BTreeMap<String, crate::manifest::ActivationBlock>,
    contributed: &std::collections::BTreeMap<String, crate::manifest::ActivationBlock>,
    others: &[crate::installed::InstalledFiles],
) {
    for (os, block) in contributed {
        let now_empty = {
            let cur = match base.get_mut(os) {
                Some(c) => c,
                None => continue,
            };
            // env vars: drop a key only if no other package set it and the value
            // is still the one this package contributed (don't clobber user edits).
            for (k, v) in &block.env {
                let kept_by_other = others
                    .iter()
                    .any(|o| o.activation.get(os).and_then(|b| b.env.get(k)).is_some());
                if !kept_by_other && cur.env.get(k) == Some(v) {
                    cur.env.remove(k);
                }
            }
            // path_prepend: drop a path only if no other package also adds it.
            for p in &block.path_prepend {
                let kept_by_other = others.iter().any(|o| {
                    o.activation
                        .get(os)
                        .is_some_and(|b| b.path_prepend.contains(p))
                });
                if !kept_by_other {
                    cur.path_prepend.retain(|x| x != p);
                }
            }
            cur.path_prepend.is_empty() && cur.env.is_empty()
        };
        if now_empty {
            base.remove(os);
        }
    }
}

/// Undo a package's tools, keeping any a different package also contributes or
/// the user has since redefined.
fn revert_tools(
    base: &mut std::collections::BTreeMap<String, crate::manifest::Tool>,
    contributed: &std::collections::BTreeMap<String, crate::manifest::Tool>,
    others: &[crate::installed::InstalledFiles],
) {
    for (tname, tool) in contributed {
        let kept_by_other = others.iter().any(|o| o.tools.contains_key(tname));
        if !kept_by_other && base.get(tname) == Some(tool) {
            base.remove(tname);
        }
    }
}

/// Re-install one or all packages from the source recorded in the manifest.
/// A `file://` source re-installs from that local directory; anything else is
/// re-pulled from the registry. Files present in the old version but not the new
/// one are pruned (by diffing the install records).
fn update(package: Option<&str>, env_name: &str) -> Result<()> {
    let env = load_registered_env(env_name)?;

    let targets: Vec<(String, String)> = match package {
        Some(p) => {
            let pk = env
                .manifest
                .packages
                .iter()
                .find(|x| x.name == p)
                .ok_or_else(|| anyhow!("'{p}' is not installed in '{env_name}'"))?;
            vec![(pk.name.clone(), pk.source.clone())]
        }
        None => env
            .manifest
            .packages
            .iter()
            .map(|p| (p.name.clone(), p.source.clone()))
            .collect(),
    };

    if targets.is_empty() {
        println!("No packages installed in '{env_name}'.");
        return Ok(());
    }

    for (pname, source) in &targets {
        // Snapshot the old file list, re-install, then remove files that the new
        // version no longer ships.
        let old_files: Vec<String> = crate::installed::InstalledFiles::load(&env.root, pname)?
            .map(|r| r.files)
            .unwrap_or_default();

        eprintln!("toolbox: updating {pname} from {source}");
        match source.strip_prefix("file://") {
            Some(dir) => install_from(Path::new(dir), env_name, None, None)?,
            None => install(source, env_name)?,
        }

        let new_files: std::collections::HashSet<String> =
            crate::installed::InstalledFiles::load(&env.root, pname)?
                .map(|r| r.files.into_iter().collect())
                .unwrap_or_default();

        let mut stale = Vec::new();
        for f in old_files {
            if !new_files.contains(&f) {
                let abs = env.root.join(f.replace('/', std::path::MAIN_SEPARATOR_STR));
                match std::fs::remove_file(&abs) {
                    Ok(()) => stale.push(f),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e).with_context(|| format!("pruning {}", abs.display())),
                }
            }
        }
        if !stale.is_empty() {
            prune_empty_dirs(&env.root, &stale);
            eprintln!("toolbox: pruned {} stale file(s) from {pname}", stale.len());
        }
    }
    println!("Updated {} package(s) in '{env_name}'", targets.len());
    Ok(())
}

fn uninstall(package: &str, env_name: &str) -> Result<()> {
    let reg = Registry::load()?;
    let entry = reg.get(env_name)?;
    let env = Env::load(&entry.path)?;

    let record = crate::installed::InstalledFiles::load(&env.root, package)?
        .ok_or_else(|| anyhow!("package '{package}' is not installed in env '{env_name}'"))?;

    // Remove each recorded file. Missing files are tolerated (already gone),
    // but other I/O errors are surfaced.
    let mut removed = 0usize;
    for rel in &record.files {
        let abs = env
            .root
            .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        match std::fs::remove_file(&abs) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing {}", abs.display())),
        }
    }
    prune_empty_dirs(&env.root, &record.files);

    // What every *other* installed package still contributes, so we don't revert
    // activation/tools they rely on.
    let others: Vec<crate::installed::InstalledFiles> =
        crate::installed::InstalledFiles::all(&env.root)?
            .into_iter()
            .filter(|r| r.package != package)
            .collect();

    // Drop the package reference and revert the activation/tools it merged in.
    let mut manifest = env.manifest.clone();
    manifest.packages.retain(|p| p.name != package);
    revert_activation(&mut manifest.activation, &record.activation, &others);
    revert_tools(&mut manifest.tools, &record.tools, &others);
    manifest.save(&env.root)?;

    crate::installed::InstalledFiles::remove(&env.root, package)?;

    // Rebuild the relocate index now that those files are gone.
    let idx = crate::relocate::scan_for_sentinel(&env.root)?;
    idx.save(&env.root)?;

    println!("Uninstalled {package} ({removed} files) from {env_name}");
    Ok(())
}

/// Remove directories left empty by an uninstall, walking from each removed
/// file up toward the env root. Never removes the env root or the skeleton
/// directories created by `init`; `remove_dir` only succeeds on empty dirs, so
/// directories still holding other packages' files are left untouched.
fn prune_empty_dirs(env_root: &Path, files: &[String]) {
    use std::collections::BTreeSet;

    let mut protected: BTreeSet<String> = BTreeSet::new();
    protected.insert("share".into());
    protected.insert(".toolbox".into());
    for os in ["windows", "linux", "macos"] {
        protected.insert(os.into());
        for sub in ["bin", "lib", "share"] {
            protected.insert(format!("{os}/{sub}"));
        }
    }

    // Collect candidate dirs (ancestors of removed files), deepest first.
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for rel in files {
        let mut cur = rel.as_str();
        while let Some(idx) = cur.rfind('/') {
            cur = &cur[..idx];
            dirs.insert(cur.to_string());
        }
    }
    let mut ordered: Vec<&String> = dirs.iter().collect();
    ordered.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));

    for rel in ordered {
        if protected.contains(rel) {
            continue;
        }
        let abs = env_root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        // Ignore errors: a non-empty dir simply stays.
        let _ = std::fs::remove_dir(&abs);
    }
}

const OS_SCOPES: [&str; 4] = ["all", "windows", "linux", "macos"];

fn check_os(os: &str) -> Result<()> {
    if OS_SCOPES.contains(&os) {
        Ok(())
    } else {
        Err(anyhow!(
            "invalid --os '{os}'; expected one of: {}",
            OS_SCOPES.join(", ")
        ))
    }
}

/// Drop an activation block that has become empty, so the manifest stays tidy.
fn prune_empty_block(
    activation: &mut std::collections::BTreeMap<String, crate::manifest::ActivationBlock>,
    os: &str,
) {
    if activation
        .get(os)
        .is_some_and(|b| b.path_prepend.is_empty() && b.env.is_empty())
    {
        activation.remove(os);
    }
}

fn config_dispatch(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::SetEnv {
            env,
            key,
            value,
            os,
        } => config_set_env(&env, &key, &value, &os),
        ConfigAction::UnsetEnv { env, key, os } => config_unset_env(&env, &key, &os),
        ConfigAction::AddPath { env, path, os } => config_add_path(&env, &path, &os),
        ConfigAction::RemovePath { env, path, os } => config_remove_path(&env, &path, &os),
        ConfigAction::AddTool {
            env,
            name,
            run,
            args,
            env_vars,
        } => config_add_tool(&env, &name, &run, &args, &env_vars),
        ConfigAction::RemoveTool { env, name } => config_remove_tool(&env, &name),
        ConfigAction::Show { env } => config_show(&env),
    }
}

fn config_set_env(name: &str, key: &str, value: &str, os: &str) -> Result<()> {
    check_os(os)?;
    let env = load_registered_env(name)?;
    let mut manifest = env.manifest.clone();
    manifest
        .activation
        .entry(os.to_string())
        .or_default()
        .env
        .insert(key.to_string(), value.to_string());
    manifest.save(&env.root)?;
    println!("Set {key} in [{os}] activation of '{name}'");
    Ok(())
}

fn config_unset_env(name: &str, key: &str, os: &str) -> Result<()> {
    check_os(os)?;
    let env = load_registered_env(name)?;
    let mut manifest = env.manifest.clone();
    let removed = match manifest.activation.get_mut(os) {
        Some(block) => block.env.remove(key).is_some(),
        None => false,
    };
    if !removed {
        anyhow::bail!("'{key}' is not set in [{os}] activation of '{name}'");
    }
    prune_empty_block(&mut manifest.activation, os);
    manifest.save(&env.root)?;
    println!("Unset {key} from [{os}] activation of '{name}'");
    Ok(())
}

fn config_add_path(name: &str, path: &str, os: &str) -> Result<()> {
    check_os(os)?;
    let env = load_registered_env(name)?;
    let mut manifest = env.manifest.clone();
    let block = manifest.activation.entry(os.to_string()).or_default();
    if block.path_prepend.iter().any(|p| p == path) {
        println!("'{path}' is already in [{os}] PATH of '{name}'");
        return Ok(());
    }
    block.path_prepend.push(path.to_string());
    manifest.save(&env.root)?;
    println!("Added {path} to [{os}] PATH of '{name}'");
    Ok(())
}

fn config_remove_path(name: &str, path: &str, os: &str) -> Result<()> {
    check_os(os)?;
    let env = load_registered_env(name)?;
    let mut manifest = env.manifest.clone();
    let removed = match manifest.activation.get_mut(os) {
        Some(block) => {
            let before = block.path_prepend.len();
            block.path_prepend.retain(|p| p != path);
            block.path_prepend.len() != before
        }
        None => false,
    };
    if !removed {
        anyhow::bail!("'{path}' is not in [{os}] PATH of '{name}'");
    }
    prune_empty_block(&mut manifest.activation, os);
    manifest.save(&env.root)?;
    println!("Removed {path} from [{os}] PATH of '{name}'");
    Ok(())
}

fn config_add_tool(
    name: &str,
    tool_name: &str,
    run: &str,
    args: &[String],
    env_vars: &[String],
) -> Result<()> {
    let env = load_registered_env(name)?;
    let mut tool_env = std::collections::BTreeMap::new();
    for kv in env_vars {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| anyhow!("--env-var must be KEY=VALUE, got '{kv}'"))?;
        tool_env.insert(k.to_string(), v.to_string());
    }
    let mut manifest = env.manifest.clone();
    manifest.tools.insert(
        tool_name.to_string(),
        crate::manifest::Tool {
            run: run.to_string(),
            args: args.to_vec(),
            env: tool_env,
        },
    );
    manifest.save(&env.root)?;
    println!("Added tool '{tool_name}' to '{name}' (run: {run})");
    Ok(())
}

fn config_remove_tool(name: &str, tool_name: &str) -> Result<()> {
    let env = load_registered_env(name)?;
    let mut manifest = env.manifest.clone();
    if manifest.tools.remove(tool_name).is_none() {
        anyhow::bail!("'{tool_name}' is not a declared tool of '{name}'");
    }
    manifest.save(&env.root)?;
    println!("Removed tool '{tool_name}' from '{name}'");
    Ok(())
}

fn config_show(name: &str) -> Result<()> {
    let env = load_registered_env(name)?;
    let m = &env.manifest;
    if m.activation.is_empty() && m.tools.is_empty() {
        println!("No activation or tools configured for '{name}'.");
        return Ok(());
    }
    if !m.activation.is_empty() {
        println!("Activation for '{name}':");
        for (os, block) in &m.activation {
            println!("  [{os}]");
            for p in &block.path_prepend {
                println!("    PATH += {p}");
            }
            for (k, v) in &block.env {
                println!("    {k} = {v}");
            }
        }
    }
    if !m.tools.is_empty() {
        println!("Tools for '{name}':");
        for (tname, tool) in &m.tools {
            let args = if tool.args.is_empty() {
                String::new()
            } else {
                format!(" {}", tool.args.join(" "))
            };
            println!("  {tname}: {}{args}", tool.run);
            for (k, v) in &tool.env {
                println!("    env: {k} = {v}");
            }
        }
    }
    Ok(())
}

fn register(path: &Path, name: Option<&str>) -> Result<()> {
    let abs = dunce::canonicalize(path)?;
    let env = Env::load(&abs)?;
    let n = name.unwrap_or(&env.manifest.name).to_string();
    let mut reg = Registry::load()?;
    reg.insert(&n, &abs)?;
    reg.save()?;
    println!("Registered '{}' -> {}", n, abs.display());
    Ok(())
}

fn unregister(name: &str) -> Result<()> {
    let mut reg = Registry::load()?;
    reg.remove(name)?;
    reg.save()?;
    println!("Unregistered '{}'", name);
    Ok(())
}

fn remove(name: &str) -> Result<()> {
    let mut reg = Registry::load()?;
    let path = reg.get(name)?.path.clone();

    // Safety: only delete a directory that actually looks like a toolbox env, so
    // a stale/misconfigured registry entry can't nuke an unrelated directory.
    if !path.join(crate::manifest::MANIFEST_FILE).exists() {
        return Err(anyhow!(
            "refusing to delete {}: no {} found (not a toolbox env). \
             Use `unregister` to drop it from the registry instead.",
            path.display(),
            crate::manifest::MANIFEST_FILE
        ));
    }

    std::fs::remove_dir_all(&path).with_context(|| format!("deleting {}", path.display()))?;
    reg.remove(name)?;
    reg.save()?;
    println!("Removed env '{name}' and deleted {}", path.display());
    Ok(())
}

fn info(name: &str) -> Result<()> {
    let env = load_registered_env(name)?;
    let m = &env.manifest;

    println!("{} {}", m.name, m.version);
    if let Some(d) = &m.description {
        println!("  {d}");
    }
    println!("  path: {}", env.root.display());

    let idx = crate::relocate::RelocateIndex::load(&env.root)?;
    let reloc = match crate::relocate::last_prefix(&env.root) {
        Some(last) if last != env.root => {
            format!("needs relocation (last mounted at {})", last.display())
        }
        Some(_) => "up to date".to_string(),
        None => "never activated on this machine".to_string(),
    };
    println!(
        "  relocation: {reloc} ({} indexed file(s))",
        idx.entries.len()
    );

    if m.packages.is_empty() {
        println!("  packages: (none)");
    } else {
        println!("  packages:");
        for p in &m.packages {
            println!("    {} {} — {}", p.name, p.version, p.source);
        }
    }

    if !m.tools.is_empty() {
        println!("  tools:");
        for (tname, tool) in &m.tools {
            let args = if tool.args.is_empty() {
                String::new()
            } else {
                format!(" {}", tool.args.join(" "))
            };
            println!("    {tname}: {}{args}", tool.run);
        }
    }

    if !m.activation.is_empty() {
        println!("  activation:");
        for (os, block) in &m.activation {
            println!(
                "    [{os}] {} path add(s), {} env var(s)",
                block.path_prepend.len(),
                block.env.len()
            );
        }
    }
    Ok(())
}

fn list() -> Result<()> {
    let reg = Registry::load()?;
    if reg.envs.is_empty() {
        println!("No envs registered. Use `toolbox register <path>`.");
        return Ok(());
    }
    println!("{:<24} {:<8} PATH", "NAME", "STATUS");
    for (name, entry) in &reg.envs {
        let status = if entry.path.join(crate::manifest::MANIFEST_FILE).exists() {
            "ok"
        } else {
            "missing"
        };
        println!("{:<24} {:<8} {}", name, status, entry.path.display());
    }
    Ok(())
}

fn activate_cmd(name: &str, shell_arg: &str) -> Result<()> {
    let reg = Registry::load()?;
    let entry = reg.get(name)?;
    let env = Env::load(&entry.path)?;
    auto_relocate(&env.root)?;
    let shell = parse_shell(shell_arg)?;
    let script = activate::render_activate(&env, shell)?;
    print!("{script}");
    Ok(())
}

fn auto_relocate(env_root: &Path) -> Result<()> {
    let idx = crate::relocate::RelocateIndex::load(env_root)?;
    if idx.entries.is_empty() {
        return Ok(());
    }
    let last = crate::relocate::last_prefix(env_root);
    if last.as_deref() == Some(env_root) {
        return Ok(());
    }
    eprintln!("toolbox: relocating env to {}", env_root.display());
    crate::relocate::apply(env_root, &idx, env_root)?;
    Ok(())
}

fn relocate_cmd(name: &str) -> Result<()> {
    let reg = Registry::load()?;
    let entry = reg.get(name)?;
    let env = Env::load(&entry.path)?;
    let idx = crate::relocate::RelocateIndex::load(&env.root)?;
    if idx.entries.is_empty() {
        println!("Env '{}' has no relocate index — nothing to do.", name);
        return Ok(());
    }
    crate::relocate::apply(&env.root, &idx, &env.root)?;
    println!("Relocated '{}' to {}", name, env.root.display());
    Ok(())
}

fn pack_index(path: &Path) -> Result<()> {
    let abs = dunce::canonicalize(path)?;
    let idx = crate::relocate::scan_for_sentinel(&abs)?;
    let n = idx.entries.len();
    idx.save(&abs)?;
    println!(
        "Wrote {} with {} entries.",
        abs.join(crate::relocate::RELOCATE_FILE).display(),
        n
    );
    Ok(())
}

fn push_cmd(
    path: &Path,
    reference: &str,
    name: Option<String>,
    version: Option<String>,
    platforms: Vec<String>,
) -> Result<()> {
    let abs = dunce::canonicalize(path)?;
    let opts = crate::oci::PushOptions {
        name,
        version,
        platforms,
        description: None,
    };
    eprintln!("toolbox: pushing {} to {reference}", abs.display());
    let summary = crate::oci::push(&abs, reference, &opts)?;
    println!(
        "Pushed {} {} -> {}",
        summary.name, summary.version, summary.manifest_url
    );
    Ok(())
}

fn deactivate_cmd(shell_arg: &str) -> Result<()> {
    let shell = parse_shell(shell_arg)?;
    print!("{}", activate::render_deactivate(shell));
    Ok(())
}

fn parse_shell(s: &str) -> Result<Shell> {
    Ok(match s {
        "posix" | "bash" | "zsh" | "sh" => Shell::Posix,
        "pwsh" | "powershell" => Shell::PowerShell,
        "auto" => Shell::detect(),
        other => return Err(anyhow!("unknown shell: {other}")),
    })
}

fn run_cmd(name: &str, cmd: &str, args: &[String]) -> Result<()> {
    let env = load_registered_env(name)?;
    auto_relocate(&env.root)?;

    let mut command = if let Some(spec) = env.manifest.tools.get(cmd) {
        // Declared tool: build its command, then append the caller's args.
        let mut command = build_tool_command(&env, spec)?;
        command.args(args);
        command
    } else {
        // Not a declared tool: treat `cmd` as a program to execute.
        let program = activate::resolve_program(&env, cmd).ok_or_else(|| {
            let declared: Vec<&str> = env.manifest.tools.keys().map(String::as_str).collect();
            anyhow!(
                "'{cmd}' is not a declared tool of env '{name}' and was not found on PATH; \
                 declared tools: {}",
                if declared.is_empty() {
                    "(none)".into()
                } else {
                    declared.join(", ")
                }
            )
        })?;
        let mut command = std::process::Command::new(&program);
        command.args(args);
        activate::apply_to_command(&env, &mut command)?;
        command
    };

    let status = command.status().context("spawning child process")?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Resolve a tool's `run` field to a program path. The value is template-
/// resolved first (`$TOOLBOX_PREFIX`, `$ENV...`); a path-like result is taken
/// relative to the env root (unless absolute), otherwise it is looked up on the
/// env's PATH.
fn resolve_tool_program(env: &Env, run: &str) -> Result<std::path::PathBuf> {
    let resolved = crate::activation_vars::resolve(run, &env.root)?;
    if resolved.contains('/') || resolved.contains('\\') {
        let p = std::path::Path::new(&resolved);
        let path = if p.is_absolute() {
            p.to_path_buf()
        } else {
            env.root.join(&resolved)
        };
        if path.is_file() {
            return Ok(path);
        }
        return Err(anyhow!("tool program not found: {}", path.display()));
    }
    activate::resolve_program(env, &resolved).ok_or_else(|| {
        anyhow!(
            "could not find tool program '{resolved}' in env '{}' or PATH",
            env.manifest.name
        )
    })
}

/// Build a `Command` for a declared tool: resolved program, template-resolved
/// declared args, activation env, then the tool's own env on top. No stdio or
/// caller args are set — the caller decides those.
fn build_tool_command(env: &Env, spec: &crate::manifest::Tool) -> Result<std::process::Command> {
    let program = resolve_tool_program(env, &spec.run)?;
    let mut command = std::process::Command::new(&program);
    for a in &spec.args {
        command.arg(crate::activation_vars::resolve(a, &env.root)?);
    }
    activate::apply_to_command(env, &mut command)?;
    for (k, v) in &spec.env {
        command.env(k, crate::activation_vars::resolve(v, &env.root)?);
    }
    Ok(command)
}

fn start_service(env_name: &str, tool: &str) -> Result<()> {
    use crate::service::ServiceState;

    let env = load_registered_env(env_name)?;
    let spec = env.manifest.tools.get(tool).ok_or_else(|| {
        anyhow!("no tool '{tool}' declared in env '{env_name}'; declare one with `config add-tool`")
    })?;

    // Refuse to start a second copy of an already-running service.
    if let Some(existing) = ServiceState::load(env_name, tool)? {
        if crate::service::is_alive(existing.pid) {
            anyhow::bail!(
                "service '{tool}' is already running in '{env_name}' (pid {})",
                existing.pid
            );
        }
    }

    auto_relocate(&env.root)?;
    let mut command = build_tool_command(&env, spec)?;

    // Stream the service's output to a log file.
    let log = ServiceState::log_path(env_name, tool)?;
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out = std::fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::from(out.try_clone()?));
    command.stderr(std::process::Stdio::from(out));

    let child = crate::service::spawn_detached(&mut command)
        .with_context(|| format!("starting service '{tool}'"))?;
    let pid = child.id();

    ServiceState {
        env: env_name.to_string(),
        tool: tool.to_string(),
        pid,
        log,
        started_at: crate::service::now_secs(),
    }
    .save()?;

    println!("Started service '{tool}' in '{env_name}' (pid {pid})");
    Ok(())
}

fn stop_service(env_name: &str, tool: &str) -> Result<()> {
    use crate::service::ServiceState;

    let state = ServiceState::load(env_name, tool)?
        .ok_or_else(|| anyhow!("service '{tool}' is not running in '{env_name}'"))?;

    if crate::service::is_alive(state.pid) {
        crate::service::kill_tree(state.pid);
        println!(
            "Stopped service '{tool}' in '{env_name}' (pid {})",
            state.pid
        );
    } else {
        println!("Service '{tool}' was not running; cleared stale state.");
    }
    ServiceState::remove(env_name, tool)?;
    Ok(())
}

fn status_services(env_name: &str) -> Result<()> {
    use crate::service::ServiceState;

    let services = ServiceState::all_for_env(env_name)?;
    if services.is_empty() {
        println!("No services in '{env_name}'.");
        return Ok(());
    }
    println!("{:<20} {:<9} {:<8} PID", "SERVICE", "STATUS", "UPTIME");
    for s in services {
        let (status, uptime) = if crate::service::is_alive(s.pid) {
            ("running", format!("{}s", s.uptime_secs()))
        } else {
            ("stopped", "-".to_string())
        };
        println!("{:<20} {:<9} {:<8} {}", s.tool, status, uptime, s.pid);
    }
    Ok(())
}

fn logs_service(env_name: &str, tool: &str, lines: Option<usize>) -> Result<()> {
    let log = crate::service::ServiceState::log_path(env_name, tool)?;
    if !log.exists() {
        anyhow::bail!("no logs for service '{tool}' in '{env_name}' (never started?)");
    }
    print!("{}", crate::service::tail(&log, lines)?);
    Ok(())
}

/// Resolve a declared tool or a program to its path within an env and print it.
/// The env defaults to `$TOOLBOX_ACTIVE_ENV` (set while a tool runs), so a tool
/// can call `toolbox which <sibling>` with no `--env`. Read-only.
fn which(name: &str, env_arg: Option<&str>) -> Result<()> {
    let env_name = match env_arg {
        Some(e) => e.to_string(),
        None => std::env::var("TOOLBOX_ACTIVE_ENV").map_err(|_| {
            anyhow!("no env given and TOOLBOX_ACTIVE_ENV is unset; pass --env or activate an env")
        })?,
    };
    let env = load_registered_env(&env_name)?;

    let path = match env.manifest.tools.get(name) {
        Some(spec) => resolve_tool_program(&env, &spec.run)?,
        None => activate::resolve_program(&env, name).ok_or_else(|| {
            anyhow!("'{name}' is not a declared tool or a program on the PATH of env '{env_name}'")
        })?,
    };
    println!("{}", path.display());
    Ok(())
}

fn verify(name: &str) -> Result<()> {
    let reg = Registry::load()?;
    let entry = reg.get(name)?;
    let env = Env::load(&entry.path)?;
    println!(
        "Env '{}' v{} at {}",
        env.manifest.name,
        env.manifest.version,
        env.root.display()
    );
    let idx = crate::relocate::RelocateIndex::load(&env.root)?;
    println!("Relocate entries: {}", idx.entries.len());
    if let Some(last) = crate::relocate::last_prefix(&env.root) {
        if last != env.root {
            println!(
                "Prefix drift detected: last activated at {} but currently mounted at {}. Relocation needed.",
                last.display(),
                env.root.display()
            );
        } else {
            println!("Prefix unchanged since last activate.");
        }
    } else {
        println!("Env has never been activated on this machine.");
    }
    Ok(())
}

fn shellenv(shell_arg: &str) -> Result<()> {
    let shell = parse_shell(shell_arg)?;
    let bin = std::env::current_exe()
        .unwrap_or_else(|_| paths::install_root().unwrap().join("bin/toolbox"));
    print!("{}", activate::shell_init(shell, &bin));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::merge_activation;
    use crate::manifest::ActivationBlock;
    use std::collections::BTreeMap;

    fn block(paths: &[&str], env: &[(&str, &str)]) -> ActivationBlock {
        ActivationBlock {
            path_prepend: paths.iter().map(|s| s.to_string()).collect(),
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn merge_adds_new_os_block() {
        let mut base = BTreeMap::new();
        let mut add = BTreeMap::new();
        add.insert("all".to_string(), block(&["share/bin"], &[("FOO", "bar")]));
        merge_activation(&mut base, &add);
        assert_eq!(base["all"].path_prepend, vec!["share/bin"]);
        assert_eq!(base["all"].env["FOO"], "bar");
    }

    #[test]
    fn merge_dedups_path_and_overwrites_env() {
        let mut base = BTreeMap::new();
        base.insert("all".to_string(), block(&["share/bin"], &[("FOO", "old")]));
        let mut add = BTreeMap::new();
        // "share/bin" already present (should not duplicate); "lib" is new.
        add.insert(
            "all".to_string(),
            block(&["share/bin", "lib"], &[("FOO", "new")]),
        );
        merge_activation(&mut base, &add);
        assert_eq!(base["all"].path_prepend, vec!["share/bin", "lib"]);
        assert_eq!(base["all"].env["FOO"], "new");
    }

    #[test]
    fn merge_is_idempotent() {
        let mut base = BTreeMap::new();
        let mut add = BTreeMap::new();
        add.insert("all".to_string(), block(&["share/bin"], &[("FOO", "bar")]));
        merge_activation(&mut base, &add);
        let once = base.clone();
        merge_activation(&mut base, &add);
        assert_eq!(base["all"].path_prepend, once["all"].path_prepend);
        assert_eq!(base["all"].env, once["all"].env);
    }
}
