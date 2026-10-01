use std::process::ExitCode;

use clap::{CommandFactory, Parser};

#[derive(Parser)]
#[command(
    name = "riven",
    version,
    about = "Riven Launcher — Minecraft launcher and modpack toolkit"
)]
struct Cli {}

pub fn run() -> ExitCode {
    Cli::parse();
    match Cli::command().print_help() {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
