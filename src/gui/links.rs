use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui_kit::App;

/// The loopback port the running launcher listens on for links from later launches.
const PORT: u16 = 47655;
const HELLO: &str = "riven-open";

/// What the system asked Riven to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Open {
    /// Brings the window forward.
    Show,
    /// A pack link from `riven://install?url=…`.
    Link(String),
    /// A `.riven`, `.mrpack` or Prism `.zip` file.
    File(PathBuf),
}

impl Open {
    /// Reads a command-line argument or an URL from the OS; `None` when it is neither.
    pub fn parse(arg: &str) -> Option<Self> {
        if let Some(rest) = arg.strip_prefix("riven://") {
            let url = url::Url::parse(&format!("riven://{rest}")).ok()?;
            return match url.host_str() {
                Some("install") => url
                    .query_pairs()
                    .find(|(k, _)| k == "url")
                    .map(|(_, v)| Open::Link(v.into_owned()))
                    .filter(|o| !matches!(o, Open::Link(l) if l.is_empty())),
                _ => Some(Open::Show),
            };
        }
        let path = match arg.strip_prefix("file://") {
            Some(_) => url::Url::parse(arg).ok()?.to_file_path().ok()?,
            None => PathBuf::from(arg),
        };
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        (matches!(ext.as_str(), "riven" | "mrpack" | "zip") && path.is_file())
            .then_some(Open::File(path))
    }

    fn encode(&self) -> String {
        match self {
            Open::Show => String::new(),
            Open::Link(link) => format!("link {link}"),
            Open::File(path) => format!("file {}", path.display()),
        }
    }

    fn decode(line: &str) -> Self {
        match line.split_once(' ') {
            Some(("link", link)) => Open::Link(link.to_owned()),
            Some(("file", path)) => Open::File(path.into()),
            _ => Open::Show,
        }
    }
}

/// Who serves links: this process, the launcher already running, or nobody.
pub enum Claim {
    Primary(TcpListener),
    Forwarded,
    Alone,
}

/// Becomes the running launcher, or hands `open` to the one already running.
pub fn claim(open: Option<&Open>) -> Claim {
    let wait = if std::env::var_os(riven_launch::update::RELAUNCH_ENV).is_some() {
        Duration::from_secs(8)
    } else {
        Duration::ZERO
    };
    let deadline = Instant::now() + wait;
    loop {
        match TcpListener::bind(("127.0.0.1", PORT)) {
            Ok(listener) => return Claim::Primary(listener),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(150)),
            Err(e) => {
                if forward(open.unwrap_or(&Open::Show)) {
                    return Claim::Forwarded;
                }
                tracing::warn!("cannot hold port {PORT} for links: {e}");
                return Claim::Alone;
            }
        }
    }
}

/// `true` when a running launcher took the request.
fn forward(open: &Open) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", PORT)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if writeln!(stream, "{HELLO} {}", open.encode()).is_err() {
        return false;
    }
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).is_ok() && reply.trim() == HELLO
}

/// Passes requests from later launches to `tx`.
pub fn serve(listener: TcpListener, tx: mpsc::Sender<Open>) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut line = String::new();
            let Ok(mut reader) = stream.try_clone().map(BufReader::new) else {
                continue;
            };
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let Some(request) = line.trim_end().strip_prefix(HELLO) else {
                continue;
            };
            let mut stream = stream;
            let _ = writeln!(stream, "{HELLO}");
            if tx.send(Open::decode(request.trim_start())).is_err() {
                break;
            }
        }
    });
}

/// Handles requests as they arrive from `rx`.
pub fn listen(rx: mpsc::Receiver<Open>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            while let Ok(open) = rx.try_recv() {
                cx.update(|cx| handle(open, cx));
            }
        }
    })
    .detach();
}

/// Brings the window forward and starts what was asked.
pub fn handle(open: Open, cx: &mut App) {
    cx.activate(true);
    let Some(window) = cx.windows().into_iter().next() else {
        return;
    };
    let _ = window.update(cx, |_, window, cx| {
        window.activate_window();
        match &open {
            Open::Show => {}
            Open::Link(link) => super::new_instance::open_link(link, window, cx),
            Open::File(path) => super::new_instance::open_file(path, window, cx),
        }
    });
}

/// Registers `riven://` and `.riven` for copies no package installed: AppImages and unpacked archives.
#[cfg(target_os = "linux")]
pub fn register() {
    use riven_launch::update::Install;
    if cfg!(debug_assertions) {
        return;
    }
    let exe = match Install::detect() {
        Install::AppImage(path) | Install::Portable(path) => path,
        _ => return,
    };
    let Some(data) = dirs::data_dir() else {
        return;
    };
    let entry = super::assets::DESKTOP_ENTRY
        .replace("Exec=riven %u", &format!("Exec=\"{}\" %u", exe.display()));
    let files = [
        (data.join("applications/riven.desktop"), entry.into_bytes()),
        (
            data.join("mime/packages/riven.xml"),
            super::assets::MIME_INFO.as_bytes().to_vec(),
        ),
        (
            data.join("icons/hicolor/512x512/apps/riven.png"),
            super::assets::ICON_512.to_vec(),
        ),
    ];
    let mut changed = false;
    for (path, bytes) in files {
        if std::fs::read(&path).is_ok_and(|old| old == bytes) {
            continue;
        }
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, bytes));
        match written {
            Ok(()) => changed = true,
            Err(e) => tracing::warn!("cannot write {}: {e}", path.display()),
        }
    }
    if changed {
        for (tool, dir) in [
            ("update-mime-database", data.join("mime")),
            ("update-desktop-database", data.join("applications")),
        ] {
            let _ = std::process::Command::new(tool)
                .arg(dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn register() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_links_carry_the_pack_url() {
        assert_eq!(
            Open::parse(
                "riven://install?url=https%3A%2F%2Fx.github.io%2Fpack%2F%23key%3Ded25519%3Aab"
            ),
            Some(Open::Link(
                "https://x.github.io/pack/#key=ed25519:ab".into()
            ))
        );
        assert_eq!(
            Open::parse("riven://install/?url=gh:owner/repo"),
            Some(Open::Link("gh:owner/repo".into()))
        );
        assert_eq!(Open::parse("riven://install"), None);
        assert_eq!(Open::parse("riven://install?url="), None);
        assert_eq!(Open::parse("riven://"), Some(Open::Show));
        assert_eq!(Open::parse("install"), None);
        assert_eq!(Open::parse("--version"), None);
        let link = Open::Link("https://a.b/c d".into());
        assert_eq!(Open::decode(link.encode().as_str()), link);
    }
}
