use std::path::PathBuf;

use rust_i18n::t;

pub const IMAGES: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];
pub const JARS: &[&str] = &["jar"];
pub const ARCHIVES: &[&str] = &["zip"];
pub const PACKS: &[&str] = &["riven", "mrpack", "zip"];
pub const MRPACKS: &[&str] = &["mrpack", "zip"];

/// The system file picker showing `extensions` first; empty when cancelled.
pub async fn files(name: impl Into<String>, extensions: &[&str], multiple: bool) -> Vec<PathBuf> {
    let mut dialog = rfd::AsyncFileDialog::new().add_filter(name, extensions);
    // macOS merges every filter into one list, so a catch-all would undo the first.
    if !cfg!(target_os = "macos") {
        dialog = dialog.add_filter(t!("pick.all_files"), &["*"]);
    }
    if multiple {
        dialog
            .pick_files()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|f| f.path().to_owned())
            .collect()
    } else {
        dialog
            .pick_file()
            .await
            .map(|f| f.path().to_owned())
            .into_iter()
            .collect()
    }
}
