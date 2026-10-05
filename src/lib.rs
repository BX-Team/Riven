#[cfg(not(feature = "cli"))]
compile_error!("the `cli` feature is required");

mod cli;
#[cfg(feature = "gui")]
mod gui;

#[cfg(feature = "gui")]
rust_i18n::i18n!("locales", fallback = "en-US");

use std::process::ExitCode;

pub fn run() -> ExitCode {
    #[cfg(feature = "gui")]
    if std::env::args_os().len() <= 1 {
        return run_gui();
    }
    attach_parent_console();
    init_logging();
    cli::run()
}

#[cfg(feature = "gui")]
fn run_gui() -> ExitCode {
    init_logging();
    gui::run()
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
