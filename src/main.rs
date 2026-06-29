use anyhow::Result;
use clap::Parser;

mod activate;
mod activation_vars;
mod cli;
mod commands;
mod dockercfg;
mod env;
mod installed;
mod manifest;
mod oci;
mod paths;
mod registry;
mod relocate;
mod tomlp;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    commands::dispatch(cli.command)
}
