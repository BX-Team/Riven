#[cfg(not(feature = "cli"))]
compile_error!("the `cli` feature is required");

mod cli;
#[cfg(feature = "gui")]
mod gui;

#[cfg(feature = "gui")]
rust_i18n::i18n!("locales", fallback = "en-US");

use std::process::ExitCode;

pub fn run() -> ExitCode {
    riven_launch::update::cleanup();
    #[cfg(feature = "gui")]
    {
        let args: Vec<String> = std::env::args_os()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        match args.as_slice() {
            [] => return run_gui(None),
            [arg] if gui::opens(arg) => return run_gui(Some(arg.clone())),
            _ => {}
        }
    }
    attach_parent_console();
    init_logging();
    cli::run()
}

#[cfg(feature = "gui")]
fn run_gui(arg: Option<String>) -> ExitCode {
    init_logging();
    gui::run(arg)
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("RIVEN_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// A GUI-subsystem exe has no console; reuse the terminal it was started from.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}
