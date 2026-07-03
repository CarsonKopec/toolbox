use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "toolbox",
    version,
    about = "Portable env manager with relocatable envs as OCI artifacts."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new empty env at <path>.
    Init {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// Install a package into an env, from a registry or a local directory.
    Install {
        /// OCI reference to pull. Omit when using --from.
        package: Option<String>,
        #[arg(short, long)]
        env: String,
        /// Install from a local package tree (windows/, linux/, macos/, share/)
        /// instead of pulling from a registry.
        #[arg(long, value_name = "DIR")]
        from: Option<PathBuf>,
        /// Package name for --from (defaults to the directory name).
        #[arg(long)]
        name: Option<String>,
        /// Package version for --from (defaults to 0.0.0).
        #[arg(long)]
        version: Option<String>,
    },
    /// Remove an installed package from an env (deletes its files).
    Uninstall {
        /// Package name as recorded in the env manifest (not the OCI reference).
        package: String,
        #[arg(short, long)]
        env: String,
    },
    /// Re-install installed packages from their recorded source.
    Update {
        /// Package to update. Omit to update every installed package.
        package: Option<String>,
        #[arg(short, long)]
        env: String,
    },
    /// Add an existing env at <path> to this machine's registry.
    Register {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove an env from this machine's registry (files untouched).
    Unregister { name: String },
    /// Delete a registered env's directory *and* unregister it.
    Remove { name: String },
    /// Show registered envs and their mount status.
    List,
    /// Show a summary of an env: packages, tools, activation, and status.
    Info { name: String },
    /// Emit shell code to activate an env. Use with `eval $(toolbox activate <name>)`.
    Activate {
        name: String,
        #[arg(long, default_value = "auto")]
        shell: String,
    },
    /// Emit shell code to deactivate the current env.
    Deactivate {
        #[arg(long, default_value = "auto")]
        shell: String,
    },
    /// Run a declared tool, or a raw program, inside an env without persistent
    /// activation.
    ///
    /// `toolbox run <env> <tool|program> [args...]`. If the first argument names
    /// a declared tool it is run; otherwise it is executed as a program.
    Run {
        name: String,
        /// A declared tool name, or a program to execute.
        cmd: String,
        /// Arguments passed to the tool/program.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Resolve a declared tool or a program to its path within an env, printing
    /// just the path. Meant to be called from inside a running tool, where
    /// TOOLBOX_ACTIVE_ENV supplies the env if --env is omitted.
    Which {
        /// A declared tool name, or a program to look up on the env's PATH.
        name: String,
        /// Env to resolve in. Defaults to $TOOLBOX_ACTIVE_ENV.
        #[arg(short, long)]
        env: Option<String>,
    },
    /// Verify env integrity and check whether relocation is needed.
    Verify { name: String },
    /// Patch a registered env's files to match its current mount path.
    /// Normally run automatically on activate.
    Relocate { name: String },
    /// Packaging helper: scan an env tree for the prefix sentinel and write
    /// `.toolbox/relocate.json`. Run this once when building a package.
    PackIndex { path: PathBuf },
    /// Package a tree (windows/, linux/, macos/, share/) as an OCI artifact and
    /// push it to a registry. Set TOOLBOX_REGISTRY_USERNAME / _PASSWORD for auth.
    Push {
        /// Directory containing the package's overlay tree.
        path: PathBuf,
        /// OCI reference to push to, e.g. ghcr.io/me/ripgrep:14.1.0.
        reference: String,
        /// Package name (defaults to the last segment of the reference repo).
        #[arg(long)]
        name: Option<String>,
        /// Package version (defaults to the reference tag).
        #[arg(long)]
        version: Option<String>,
        /// Platform to record; repeatable. Defaults to the per-OS dirs present.
        #[arg(long = "platform")]
        platforms: Vec<String>,
    },
    /// Print the shell init snippet for sourcing in .bashrc / $PROFILE.
    Shellenv {
        #[arg(long, default_value = "auto")]
        shell: String,
    },
    /// View or edit an env's activation config (PATH additions and env vars).
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Set an activation environment variable. The value may use
    /// $TOOLBOX_PREFIX and `$ENV.VAR ?? fallback`, resolved on activate.
    SetEnv {
        env: String,
        key: String,
        value: String,
        /// Scope: all | windows | linux | macos.
        #[arg(long, default_value = "all")]
        os: String,
    },
    /// Remove an activation environment variable.
    UnsetEnv {
        env: String,
        key: String,
        #[arg(long, default_value = "all")]
        os: String,
    },
    /// Add a PATH-prepend directory, relative to the env root.
    AddPath {
        env: String,
        path: String,
        #[arg(long, default_value = "all")]
        os: String,
    },
    /// Remove a PATH-prepend directory.
    RemovePath {
        env: String,
        path: String,
        #[arg(long, default_value = "all")]
        os: String,
    },
    /// Declare (or replace) a runnable tool. `run`, args, and env values may use
    /// $TOOLBOX_PREFIX and `$ENV.VAR ?? fallback`, resolved at run time.
    AddTool {
        env: String,
        /// Tool name, used as `toolbox run <env> <name>`.
        name: String,
        /// Program to run: a command on PATH or an env-relative path.
        #[arg(long)]
        run: String,
        /// An argument (repeatable, in order). May start with `-`.
        #[arg(long = "arg", allow_hyphen_values = true)]
        args: Vec<String>,
        /// An extra env var as KEY=VALUE (repeatable).
        #[arg(long = "env-var")]
        env_vars: Vec<String>,
    },
    /// Remove a declared tool.
    RemoveTool { env: String, name: String },
    /// Show the env's activation config and declared tools.
    Show { env: String },
}
