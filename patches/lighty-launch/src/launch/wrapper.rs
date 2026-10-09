// Riven: commands the game is started through, per instance (`gamemoderun`, `mangohud`).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};

use lighty_java::errors::JavaRuntimeError;
use tokio::process::{Child, Command};

static WRAPPERS: LazyLock<Mutex<HashMap<String, Vec<String>>>> = LazyLock::new(Default::default);

/// Starts `instance`'s game as `argv… java …`; an empty `argv` starts Java directly.
pub fn set_wrapper(instance: &str, argv: Vec<String>) {
    let mut wrappers = WRAPPERS.lock().unwrap_or_else(|e| e.into_inner());
    if argv.is_empty() {
        wrappers.remove(instance);
    } else {
        wrappers.insert(instance.to_owned(), argv);
    }
}

pub(crate) fn wrapper(instance: &str) -> Option<Vec<String>> {
    WRAPPERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(instance)
        .cloned()
}

pub(crate) fn spawn(
    argv: &[String],
    java: &Path,
    arguments: Vec<String>,
    game_dir: &Path,
) -> Result<Child, JavaRuntimeError> {
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .arg(java)
        .args(arguments)
        .current_dir(game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    Ok(command.spawn()?)
}
