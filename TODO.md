# ToolBox — TODO

Working backlog, ordered by impact. Check items off as they land.

## Done

- [x] `toolbox run` — run a command inside an env without persistent activation
- [x] `toolbox uninstall` — remove a package's files + manifest entry (per-package file tracking)
- [x] `toolbox push` — package a tree as an OCI artifact and push it
- [x] Migrate config files to TOML+ (`.tomlp`) via `tomlplus-syntax` (manifest + registry)
- [x] Tighten manifest annotations + self-validating `save` (env-name `@pattern`, fail-fast `init`)
- [x] Spike: TOML+ variable resolution for activation values (`$ENV.X ?? fallback`), wired behind the render path

## Do first — prove and unblock the core loop

- [x] **Install from a local artifact** (`install --from <dir>`) — offline path; unblocks testing below. Overlays a local package tree, runs the full manifest/record/relocate pipeline (sentinel patching verified live)
- [x] **End-to-end relocation test** — `tests/relocation_e2e.rs` drives the real binary: build → install → move env dir → re-register → activate; asserts the sentinel re-patches from the old path to the new one
- [x] **Package-declared activation** — packages declare activation in `toolbox-package.tomlp`; `push` embeds it in the OCI config blob, `install`/`install --from` merge it into the env manifest (dedup paths, overwrite env vars, idempotent)

## Close behind — make the new work first-class

- [x] **Formalize the varspike** — adopted "render-time templates" as the model; renamed `varspike` → `activation_vars`, reframed the docs, documented the value syntax (`$TOOLBOX_PREFIX`, `$ENV.VAR ?? fallback`) on `ActivationBlock.env`
- [x] **CLI to edit activation** — `toolbox config set-env / unset-env / add-path / remove-path / show`; edits the env manifest's activation (os-scoped via `--os`, self-validating save, empty-block pruning)
- [x] **CLI to manage tools** — `toolbox config add-tool / remove-tool` (and `show` now lists tools), so declaring a runnable tool no longer needs hand-editing the manifest

## Tool runtime (new initiative)

Goal: run tools (incl. custom scripts) through toolbox, let tools call back, and
supervise long-running services. All three sit on the declarative-tools substrate.

- [x] **Phase 1 — Declarative tools.** `[tools.x]` in the manifest (`run`/`args`/`env`, template-resolved); `toolbox run <env> <tool|program> [args]` runs a declared tool or falls back to a raw program. Packages ship tools via `toolbox-package.tomlp` → OCI config blob → merged on install.
- [x] **Phase 2 — Callback API (resolve).** `toolbox which <name>` resolves a declared tool or program to its path; env defaults to `$TOOLBOX_ACTIVE_ENV`, so a running tool can find a sibling with no `--env`. (Event emission, if ever wanted, would build on this.)
- [ ] **Phase 3 — Long-running services.** A service is a tool with a restart policy; supervise via a per-service `toolbox __supervise` shim + state file (pid/log), no central daemon. `start`/`stop`/`status`/`logs`. Socket IPC only if live interaction is needed later. _(large)_

## Later — adoption and reach

- [x] **Revert package activation/tools on uninstall** — the install record now stores what each package contributed; uninstall subtracts its activation + tools, skipping anything another installed package still provides or the user has since changed.

- [x] **Real registry auth** — pull and push source credentials from the Docker config (`~/.docker/config.json` / `$DOCKER_CONFIG`): `auths` (base64 or plain) and credential helpers (`credHelpers`/`credsStore`). `TOOLBOX_REGISTRY_*` env vars override; anonymous fallback. So `docker login` just works.
- [x] **README + packaging guide** — `README.md` (overview, install, quickstart, command reference) and `docs/GUIDE.md` (day-to-day walkthrough + packaging). Published with the v0.1.0 GitHub release.
- [x] **`update` command** — `toolbox update [pkg] -e <env>` re-installs one or all packages from their recorded source (`file://` dir or registry ref)
