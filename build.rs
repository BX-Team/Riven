use std::process::Command;

fn main() {
    // rust-i18n embeds the locale files at compile time; cargo cannot see that dependency.
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
    println!("cargo:rerun-if-env-changed=RIVEN_REV");

    let rev = std::env::var("RIVEN_REV").ok().or_else(|| {
        let out = Command::new("git")
            .args(["rev-parse", "--short=8", "HEAD"])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    });
    println!(
        "cargo:rustc-env=RIVEN_REV={}",
        rev.unwrap_or_else(|| "unknown".into())
    );
    #[cfg(windows)]
    windows_resources();
}

/// The exe's icon and the name Explorer and the task manager show.
#[cfg(windows)]
fn windows_resources() {
    println!("cargo:rerun-if-changed=assets/brand/riven.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/brand/riven.ico")
        .set("ProductName", "Riven Launcher")
        .set("FileDescription", "Riven Launcher")
        .set("CompanyName", "BX Team")
        .set("LegalCopyright", "BX Team, GPL-3.0-or-later");
    if let Err(e) = res.compile() {
        println!("cargo:warning=cannot embed the Windows resources: {e}");
    }
}
