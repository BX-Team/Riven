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
    #[cfg(all(windows, not(feature = "gui")))]
    if std::env::args_os().len() == 1 && open_window() {
        return ExitCode::SUCCESS;
    }
    attach_parent_console();
    init_logging();
    cli::run()
}

/// The installer's `riven.com` is the console build; with no arguments it opens the launcher next to it.
#[cfg(all(windows, not(feature = "gui")))]
fn open_window() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    if !exe
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("com"))
    {
        return false;
    }
    let window = exe.with_extension("exe");
    window.is_file() && std::process::Command::new(window).spawn().is_ok()
}

#[cfg(feature = "gui")]
fn run_gui(arg: Option<String>) -> ExitCode {
    init_logging();
    gui::run(arg)
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("RIVEN_LOG")
        // Graphics driver probing and optional D-Bus services warn on healthy systems.
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,wgpu_hal=error,zbus=error"));
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
