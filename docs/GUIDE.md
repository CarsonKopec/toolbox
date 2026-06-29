# ToolBox Guide

A practical walkthrough of using ToolBox day to day, plus how to build and share packages. Commands are shown for PowerShell; bash/zsh equivalents are noted where they differ.

## 1. Setup

Install the binary and load the shell hook once:

```powershell
cargo install --path .
# add to $PROFILE:
Invoke-Expression (& toolbox shellenv --shell pwsh | Out-String)
```

```sh
# bash/zsh equivalent in ~/.bashrc / ~/.zshrc
eval "$(toolbox shellenv --shell posix)"
```

The hook defines a `toolbox` shell function that runs the binary normally, except for `activate`/`deactivate`, which it evaluates so they can change your current shell. Without it, `toolbox activate` only *prints* the activation script.

## 2. Create an env

```powershell
toolbox init C:\envs\dev --name dev
toolbox register C:\envs\dev
```

`init` lays out the directory:

```
C:\envs\dev\
  windows\{bin,lib,share}   linux\...   macos\...   # per-OS trees
  share\                                            # OS-independent data
  .toolbox\                                         # metadata (relocation index, etc.)
  toolbox-env.tomlp                                 # the manifest
```

`register` records the env in this machine's registry (`%LOCALAPPDATA%\ToolBox\registry.tomlp`), so you can refer to it by name.

## 3. Put tools in the env

**Drop-in binaries.** The simplest approach — copy an executable into the per-OS `bin`; it's on PATH after activation, no declaration needed:

```powershell
Copy-Item rg.exe C:\envs\dev\windows\bin\
```

**Activation env vars and extra PATH dirs** via the `config` command:

```powershell
toolbox config add-path dev share\scripts
toolbox config set-env dev PYTHONHOME '$TOOLBOX_PREFIX/share/py'
toolbox config set-env dev EDITOR     '$ENV.EDITOR ?? \"vim\"'
toolbox config show dev
```

Activation values are **render-time templates**, resolved when you activate:

- `$TOOLBOX_PREFIX` → the env's current mount path.
- `$ENV.VAR ?? fallback` → read `VAR` from the host environment, with a default.

> **PowerShell 5.1 quoting:** embedded double quotes need `\"` (e.g. `'$ENV.EDITOR ?? \"vim\"'`) — the shell mangles bare `"` when calling native programs. bash/zsh are unaffected.

**Named tools** for parameterized runs (e.g. a script with a fixed interpreter). Declared in the manifest's `[tools]` section — today by editing `toolbox-env.tomlp` directly:

```toml
[tools]
fmt = #{ run = "python", args = ["$TOOLBOX_PREFIX/share/scripts/fmt.py"] }#
rg  = #{ run = "windows/bin/rg.exe" }#
```

A tool's `run` is resolved as an env-relative path (if it contains a separator) or looked up on PATH; `args` and `env` are templates like activation values.

## 4. The daily loop

Two ways to use an env, pick per task:

**Interactive** — enter the env, work, leave:

```powershell
toolbox activate dev      # bins + env vars live in this shell
rg TODO
toolbox deactivate
```

**One-off** — run a single tool without touching your shell (ideal for scripts and CI):

```powershell
toolbox run dev fmt src\          # a declared tool, with your args appended
toolbox run dev rg TODO           # a raw program in the env
```

`toolbox list` shows your envs and whether each is intact.

## 5. Portability — the payoff

An env is a plain directory. Copy or sync it anywhere, then point ToolBox at the new location:

```powershell
# ...on another machine, or a different drive:
toolbox register D:\envs\dev
toolbox activate dev      # paths inside binaries/scripts auto-relocate to D:\
```

On activation ToolBox detects the env moved (its recorded prefix no longer matches) and rewrites the `__TOOLBOX_PREFIX__`-derived paths inside the env's files to the new mount point. Run `toolbox verify dev` to check whether relocation is needed, or `toolbox relocate dev` to force it.

## 6. Building and sharing a package

A **package** is a tree that overlays onto an env: `windows/`, `linux/`, `macos/`, and/or `share/`, with files built to carry the `__TOOLBOX_PREFIX__` sentinel wherever they reference their own location.

1. Lay out the tree and (optionally) declare metadata, activation, and tools in a `toolbox-package.tomlp` at its root:

   ```toml
   name = "pytools"
   version = "1.0.0"

   [activation]
   all = #{ env = #{ PYTHONHOME = "$TOOLBOX_PREFIX/share/py" }# }#

   [tools]
   greet = #{ run = "python", args = ["$TOOLBOX_PREFIX/share/scripts/greet.py"] }#
   ```

2. Index the relocation sentinels:

   ```powershell
   toolbox pack-index .\pytools
   ```

3. Share it, either way:

   ```powershell
   # Push to an OCI registry. ToolBox uses your `docker login` credentials
   # (~/.docker/config.json, including credential helpers); a one-off override
   # is available via TOOLBOX_REGISTRY_USERNAME / _PASSWORD.
   docker login ghcr.io          # once, with a PAT (write:packages)
   toolbox push .\pytools ghcr.io/me/pytools:1.0.0

   # ...or just hand someone the directory
   ```

On the consuming side, the package's declared activation and tools merge into the env on install:

```powershell
toolbox install ghcr.io/me/pytools:1.0.0 -e dev   # from a registry
toolbox install --from .\pytools -e dev           # from a local directory
toolbox run dev greet                             # the package's tool is now available
toolbox uninstall pytools -e dev                  # removes exactly the files it laid down
```

## 7. Config file format

ToolBox's manifests and registry are written in [TOML+](https://github.com/CarsonKopec/tomlplus) (`.tomlp`) — a TOML superset adding annotations (`@required`, `@type`, `@pattern`, …) and variables. Generated manifests are self-validating: a value that violates its annotations (e.g. an env name with illegal characters) is rejected at write time. Block dictionaries use `#{ ... }#`.

## Troubleshooting

- **`toolbox activate` printed a script instead of doing anything** — the shell hook isn't loaded; add the `shellenv` line to your profile (step 1) and restart the shell.
- **A moved env's tools fail** — run `toolbox verify <env>`; if it reports drift, `toolbox activate` (or `toolbox relocate <env>`) re-patches it.
- **Anonymous push rejected** — run `docker login <registry>` (ToolBox reuses those credentials), or set `TOOLBOX_REGISTRY_USERNAME` / `TOOLBOX_REGISTRY_PASSWORD` for a one-off. Most registries reject anonymous pushes.
