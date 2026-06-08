# ToolBox

[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

**Portable, relocatable tool environments — carry your toolbelt between machines and paths, and it just keeps working.**

ToolBox manages self-contained *environments* of CLI tools, libraries, and scripts. An env is an ordinary directory you can copy to a USB stick, sync to another machine, or move to a different drive — and the binaries and scripts inside it keep working, because ToolBox rewrites the absolute paths baked into them when the env moves. Tools are distributed as OCI artifacts (the same registries that host container images) or installed straight from a local directory.

> **Status:** early (v0.1.0). The core loop — create, install, activate, run, and *relocate* across a move — works and is covered by end-to-end tests. See [TODO.md](TODO.md) for the roadmap.

## Why

Existing tool managers tie an environment to a fixed install path. Move it and the shebangs, RPATHs, and embedded path constants break. ToolBox builds envs to be **position-independent**: a sentinel (`__TOOLBOX_PREFIX__`) is baked in wherever the env's own path appears, and it's patched to the current mount point on activation. The result is an environment you can relocate freely.

## Install

ToolBox is a single Rust binary. With a Rust toolchain installed:

```sh
cargo install --path .
```

This puts `toolbox` in `~/.cargo/bin` (already on most PATHs). Or build a release binary directly with `cargo build --release` (output at `target/release/toolbox`).

Then add the shell hook to your profile so `toolbox activate` can modify your live shell:

```powershell
# PowerShell ($PROFILE)
Invoke-Expression (& toolbox shellenv --shell pwsh | Out-String)
```

```sh
# bash/zsh (~/.bashrc or ~/.zshrc)
eval "$(toolbox shellenv --shell posix)"
```

## Quickstart

```sh
# Create an env and register it on this machine
toolbox init ~/envs/dev --name dev
toolbox register ~/envs/dev

# Add tools: drop binaries into <env>/<os>/bin, or install a package
toolbox install --from ./my-package -e dev        # from a local package tree
toolbox install ghcr.io/me/ripgrep:14.1.0 -e dev  # from an OCI registry

# Use it
toolbox activate dev          # tools on PATH + env vars, in your current shell
toolbox run dev rg TODO       # run one tool without activating
toolbox deactivate
```

Now copy `~/envs/dev` to another machine, `toolbox register` it there, and `toolbox activate dev` — the env relocates to its new path automatically.

## Concepts

- **Env** — a directory with per-OS `bin/lib/share` trees, a shared `share/`, a `.toolbox/` metadata dir, and a `toolbox-env.tomlp` manifest. The per-OS `bin` is added to PATH on activation.
- **Manifest** — `toolbox-env.tomlp`, written in [TOML+](https://github.com/CarsonKopec/tomlplus) (a TOML superset with annotations and variables). Declares the env's packages, activation contributions, and tools.
- **Activation** — what entering an env does: prepend bin dirs to PATH, set `TOOLBOX_PREFIX` / `TOOLBOX_ACTIVE_ENV`, and apply declared env vars. Values are render-time templates that may use `$TOOLBOX_PREFIX` and `$ENV.VAR ?? fallback`.
- **Tools** — named, runnable recipes (`[tools]` in the manifest): an interpreter/command plus args and env. `toolbox run <env> <tool>` runs one; an unrecognized name is run as a raw program.
- **Packages** — overlays distributed as OCI artifacts (custom media types) or local directories. A package's `toolbox-package.tomlp` can declare activation and tools that merge into the env on install.
- **Relocation** — the `__TOOLBOX_PREFIX__` sentinel is patched to the env's current path on install/activate, so a moved env keeps working.

## Commands

| Command | Purpose |
| --- | --- |
| `init <path> --name <n>` | Create a new empty env |
| `register <path>` / `unregister <name>` | Add/remove an env from this machine's registry |
| `list` | Show registered envs and their status |
| `install <ref> -e <env>` | Install a package from an OCI registry |
| `install --from <dir> -e <env>` | Install a package from a local directory |
| `uninstall <pkg> -e <env>` | Remove an installed package |
| `activate <env>` / `deactivate` | Enter/leave an env in your shell |
| `run <env> <tool\|program> [args]` | Run a declared tool, or a raw program |
| `config set-env / unset-env / add-path / remove-path / show` | Edit an env's activation |
| `verify <env>` / `relocate <env>` | Check integrity / re-patch paths |
| `push <dir> <ref>` | Package a tree and push it to a registry |
| `pack-index <dir>` | Scan a package tree for the relocation sentinel |
| `shellenv` | Print the shell hook for your profile |

See **[docs/GUIDE.md](docs/GUIDE.md)** for a full day-to-day walkthrough and a packaging guide.

## Building from source

```sh
cargo build            # debug build
cargo test             # unit + end-to-end integration tests
cargo build --release  # optimized binary
```

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
