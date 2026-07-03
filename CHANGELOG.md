# Changelog

All notable changes to ToolBox are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project aims to follow
[Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-07-03

### Added
- Background services — `toolbox start` / `stop` / `status` / `logs` run a declared tool detached, stream its output to a log, and track state under `<install_root>/run`.
- Service restart supervision — a tool's `restart` policy (`no` / `on-failure` / `always`) is enforced by a per-service supervisor that respawns the process with backoff; set it with `config add-tool --restart`.

## [0.1.1] - 2026-06-29

### Added
- `toolbox remove <env>` — delete an env's directory and unregister it (refuses a directory that isn't a toolbox env).
- `toolbox info <env>` — one-view summary: version, mount path, relocation status, packages, tools, and activation.
- `toolbox which <name>` — resolve a declared tool or program to its path; defaults to `$TOOLBOX_ACTIVE_ENV` so a running tool can find a sibling.
- `toolbox update [pkg] -e <env>` — re-install one or all packages from their recorded source, pruning files the new version no longer ships.
- `toolbox config add-tool` / `remove-tool`, and `config show` now lists tools.
- Registry credentials are sourced from the Docker config (`docker login`), including credential helpers/`credsStore`. `TOOLBOX_REGISTRY_*` still overrides.
- CI across Linux, macOS, and Windows; a release workflow builds per-OS binaries.

### Changed
- Uninstall now reverts the activation and tools a package contributed, keeping anything another installed package still relies on.

### Fixed
- Tolerate a leading UTF-8 BOM in TOML+ files (Windows editors add one).

### Docs
- Guide: "What makes a package relocatable" — how text vs. length-bounded binary slots are patched, and how to build a relocatable package.

## [0.1.0] - 2026-06-07

Initial release. Portable, relocatable tool environments distributed as OCI
artifacts:

- Relocatable envs — the `__TOOLBOX_PREFIX__` sentinel is re-patched on install/activate, so a moved env keeps working.
- `init` / `register` / `unregister` / `list`; `install` (registry or `--from` a local directory), `uninstall`, `push`.
- `activate` / `deactivate` / `run`; declarative tools (`run <env> <tool>`) with package-declared activation and tools merged on install.
- `config` commands to edit activation; TOML+ (`.tomlp`) self-validating config via `tomlplus-syntax`.

[0.2.0]: https://github.com/CarsonKopec/toolbox/releases/tag/v0.2.0
[0.1.1]: https://github.com/CarsonKopec/toolbox/releases/tag/v0.1.1
[0.1.0]: https://github.com/CarsonKopec/toolbox/releases/tag/v0.1.0
