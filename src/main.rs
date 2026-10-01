#![cfg_attr(
    all(feature = "gui", not(debug_assertions)),
    windows_subsystem = "windows"
)]

fn main() -> std::process::ExitCode {
    riven_lib::run()
}
