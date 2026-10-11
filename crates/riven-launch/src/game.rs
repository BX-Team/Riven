use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use riven_format::{
    Account, AccountKind, Instance, JavaChoice, LaunchCommands, LaunchSettings, LoaderKind,
    Release, Running,
};
use riven_game::{Event, GameVersion, Loader, Step};
use riven_sync::install;
use riven_sync::update::{self, Check, Pending, Request};

use crate::instances::Instances;
use crate::{LaunchError, io};

/// What a launch is busy with, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Refreshing a Microsoft account's session.
    SignIn,
    /// Checking the pack's channel and swapping changed files in.
    Pack,
    /// Reading version and loader metadata.
    Metadata,
    Java,
    /// Libraries, assets, the client jar and loader processors.
    Game,
    Starting,
}

#[derive(Debug, Clone)]
pub enum Progress {
    Stage(Stage),
    /// How far the current stage got, in its own unit (bytes or files).
    Amount {
        done: u64,
        total: u64,
    },
    /// The pack was installed for the first time.
    Installed {
        version: String,
    },
    /// The pack was updated to `version`.
    Updated {
        version: String,
    },
    /// The pack now ships these mods the player had added; their copies were removed.
    Replaced {
        names: Vec<String>,
    },
    /// The pack could not be checked; the game starts with what is installed.
    Offline {
        reason: String,
    },
    Started {
        pid: u32,
        /// Seconds it had already run: non-zero for a game an earlier launcher started.
        played: u64,
    },
    Line {
        stderr: bool,
        text: String,
    },
    Exited {
        code: Option<i32>,
        played: u64,
    },
}

type Report = Arc<dyn Fn(Progress) + Send + Sync>;

fn game_version(instance: &Instance) -> GameVersion {
    let (loader, loader_version) = match &instance.loader {
        None => (Loader::Vanilla, String::new()),
        Some(l) => (
            match l.kind {
                LoaderKind::Fabric => Loader::Fabric,
                LoaderKind::Quilt => Loader::Quilt,
                LoaderKind::Forge => Loader::Forge,
                LoaderKind::NeoForge => Loader::NeoForge,
            },
            l.version.clone(),
        ),
    };
    GameVersion {
        minecraft: instance.minecraft.clone(),
        loader,
        loader_version,
    }
}

fn data_dir() -> Result<PathBuf, LaunchError> {
    riven_sync::data_dir().ok_or(LaunchError::NoDir("data"))
}

/// What the game starts through: `gamemoderun mangohud java …`; Linux only, as the tools are.
fn wrapper(commands: &LaunchCommands) -> Vec<String> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    [
        ("gamemoderun", commands.gamemode),
        ("mangohud", commands.mangohud),
    ]
    .into_iter()
    .filter(|(_, on)| *on)
    .map(|(cmd, _)| cmd.to_owned())
    .collect()
}

/// Runs a user command through the shell in the game directory; a failure stops the launch.
async fn run_hook(command: &str, game_dir: &Path, id: &str) -> Result<(), LaunchError> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").raw_arg(command).creation_flags(0x08000000);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    let status = cmd
        .current_dir(game_dir)
        .env("INST_ID", id)
        .env("INST_MC_DIR", game_dir)
        .status()
        .await
        .map_err(io(game_dir))?;
    if status.success() {
        Ok(())
    } else {
        Err(LaunchError::Hook {
            command: command.to_owned(),
            code: status.code(),
        })
    }
}

/// Brings a pack instance up to its channel; being offline is reported, not fatal.
async fn update_pack(
    store: &Instances,
    id: &str,
    instance: &mut Instance,
    report: &Report,
) -> Result<(), LaunchError> {
    let game_dir = store.game_dir(id);
    if install::load_state(&game_dir)?.is_none() {
        return Ok(());
    }
    report(Progress::Stage(Stage::Pack));
    let files = riven_sync::Store::default_location().ok_or(LaunchError::NoDir("data"))?;
    let http = riven_sources::client();
    let pending = match update::check(&files, &http, &game_dir, &Request::default()).await {
        Ok(Check::Ready(pending)) => pending,
        Ok(Check::UpToDate { .. }) => return Ok(()),
        Err(e) => {
            tracing::warn!("cannot check the pack: {e}");
            report(Progress::Offline {
                reason: e.to_string(),
            });
            return Ok(());
        }
    };
    apply(store, id, instance, *pending, report).await
}

async fn apply(
    store: &Instances,
    id: &str,
    instance: &mut Instance,
    pending: Pending,
    report: &Report,
) -> Result<(), LaunchError> {
    let files = riven_sync::Store::default_location().ok_or(LaunchError::NoDir("data"))?;
    let http = riven_sources::client();
    let release = pending.release.clone();
    let sink = report.clone();
    let progress = move |e: install::Event| {
        let (done, total) = match e {
            install::Event::Downloading { done, total } => (done, total * 2),
            install::Event::Installing { done, total } => (total + done, total * 2),
        };
        sink(Progress::Amount {
            done: done as u64,
            total: total as u64,
        });
    };
    let game_dir = store.game_dir(id);
    let before = install::load_state(&game_dir)?.map(|s| s.version);
    let icon = pending.icon(&files, &http).await;
    let state = pending.apply(&files, &http, &game_dir, &progress).await?;
    if let Some(png) = icon.and_then(|p| std::fs::read(p).ok())
        && let Err(e) = store.set_pack_icon(id, &png)
    {
        tracing::warn!("{e}");
    }
    let names = crate::own::yield_to_pack(&game_dir, &release, &state.files)?;
    if !names.is_empty() {
        report(Progress::Replaced { names });
    }
    adopt_release(store, id, instance, &release)?;
    match before {
        None => report(Progress::Installed {
            version: release.version,
        }),
        Some(old) if old != release.version => report(Progress::Updated {
            version: release.version,
        }),
        Some(_) => {}
    }
    Ok(())
}

/// Installs a pack into a fresh instance, which takes the pack's Minecraft and loader.
pub async fn install_pack(
    store: &Instances,
    id: &str,
    request: Request,
    report: impl Fn(Progress) + Send + Sync + 'static,
) -> Result<(), LaunchError> {
    let report: Report = Arc::new(report);
    let mut instance = store.load(id)?;
    let game_dir = store.game_dir(id);
    std::fs::create_dir_all(&game_dir).map_err(io(&game_dir))?;
    report(Progress::Stage(Stage::Pack));
    let files = riven_sync::Store::default_location().ok_or(LaunchError::NoDir("data"))?;
    let http = riven_sources::client();
    match update::check(&files, &http, &game_dir, &request).await? {
        Check::Ready(pending) => apply(store, id, &mut instance, *pending, &report).await,
        Check::UpToDate { .. } => Ok(()),
    }
}

/// Takes the Minecraft and loader versions a pack release asks for.
pub fn adopt_release(
    store: &Instances,
    id: &str,
    instance: &mut Instance,
    release: &Release,
) -> Result<(), LaunchError> {
    if instance.minecraft == release.minecraft && instance.loader.as_ref() == Some(&release.loader)
    {
        return Ok(());
    }
    instance.minecraft = release.minecraft.clone();
    instance.loader = Some(release.loader.clone());
    store.save(id, instance)
}

/// Updates a pack instance, installs what the game needs, starts it and reports until it exits.
pub async fn play(
    store: &Instances,
    id: &str,
    account: &Account,
    defaults: &LaunchSettings,
    report: impl Fn(Progress) + Send + Sync + 'static,
) -> Result<(), LaunchError> {
    let report: Report = Arc::new(report);
    let mut instance = store.load(id)?;
    if account.kind == AccountKind::Microsoft {
        report(Progress::Stage(Stage::SignIn));
    }
    let session = crate::accounts::session(account).await?;
    update_pack(store, id, &mut instance, &report).await?;

    let settings = instance.overrides.resolve(defaults);
    let game_dir = store.game_dir(id);
    std::fs::create_dir_all(&game_dir).map_err(io(&game_dir))?;
    if let Some(cmd) = &settings.commands.pre_launch {
        run_hook(cmd, &game_dir, id).await?;
    }

    report(Progress::Stage(Stage::Metadata));
    let http = riven_sources::client();
    let dirs = riven_game::Dirs::new(data_dir()?);
    let prepared = riven_game::prepare(&http, &dirs, &game_version(&instance)).await?;
    let sink = report.clone();
    let progress = move |event: Event| {
        sink(match event {
            Event::Step(Step::Java) => Progress::Stage(Stage::Java),
            Event::Step(Step::Files | Step::Processors) => Progress::Stage(Stage::Game),
            Event::Amount { done, total } => Progress::Amount { done, total },
        })
    };
    let java = match &settings.java {
        JavaChoice::Auto => {
            riven_game::java::ensure(
                &http,
                &dirs,
                prepared.java_major(),
                prepared.needs_x64_java(),
                &progress,
            )
            .await?
        }
        JavaChoice::Path { path } => {
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Err(LaunchError::BadJava(path));
            }
            path
        }
    };
    let launch = riven_game::Launch {
        java,
        game_dir: game_dir.clone(),
        natives_dir: store.dir(id).join("natives"),
        session,
        min_memory_mb: settings.memory.min,
        max_memory_mb: settings.memory.max,
        jvm_args: settings.jvm_args.clone(),
        window: riven_game::Window {
            width: settings.window.width,
            height: settings.window.height,
            fullscreen: settings.window.fullscreen,
        },
        wrapper: wrapper(&settings.commands),
        launcher_name: "riven".into(),
        launcher_version: env!("CARGO_PKG_VERSION").into(),
    };
    riven_game::install(&http, &dirs, &prepared, &launch, &progress).await?;

    report(Progress::Stage(Stage::Starting));
    let game = riven_game::launch(&dirs, &prepared, &launch)?;
    let pid = game.pid();
    report(Progress::Started { pid, played: 0 });
    let since = Instant::now();
    let marker = Running {
        pid,
        started: crate::now_rfc3339(),
        process: riven_game::started(pid),
    };
    if let Err(e) = crate::save(&running_path(store, id), &marker) {
        tracing::warn!("{e}");
    }
    let lines = report.clone();
    let code = game
        .wait(move |stderr, text| {
            lines(Progress::Line {
                stderr,
                text: crate::strip_ansi(&text),
            })
        })
        .await;
    let played = since.elapsed().as_secs();
    finish(store, id, played, unix_now(), &settings).await;
    report(Progress::Exited { code, played });
    Ok(())
}

/// Records the play time, forgets the running game and runs the post-exit command.
async fn finish(store: &Instances, id: &str, played: u64, ended: u64, settings: &LaunchSettings) {
    record_play(store, id, played, ended);
    let _ = std::fs::remove_file(running_path(store, id));
    if let Some(cmd) = &settings.commands.post_exit
        && let Err(e) = run_hook(cmd, &store.game_dir(id), id).await
    {
        tracing::warn!("{e}");
    }
}

fn record_play(store: &Instances, id: &str, seconds: u64, at: u64) {
    let Ok(mut instance) = store.load(id) else {
        return;
    };
    instance.play_seconds += seconds;
    instance.last_played = Some(crate::rfc3339(at));
    if let Err(e) = store.save(id, &instance) {
        tracing::warn!("{e}");
    }
}

fn running_path(store: &Instances, id: &str) -> PathBuf {
    store.dir(id).join("running.json")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The game an earlier launcher started for `id` when it still runs.
///
/// One that ended meanwhile gets its play time recorded, up to its log's last write.
pub fn running(store: &Instances, id: &str) -> Option<Running> {
    let path = running_path(store, id);
    let text = std::fs::read_to_string(&path).ok()?;
    let marker = riven_format::from_str::<Running>(&text).ok();
    if let Some(m) = &marker
        && riven_game::alive(m.pid, m.process)
    {
        return marker;
    }
    if let Some(started) = marker.and_then(|m| crate::parse_rfc3339(&m.started)) {
        let ended = std::fs::metadata(latest_log(store, id))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(started, |d| d.as_secs().max(started));
        record_play(store, id, ended - started, ended);
    }
    let _ = std::fs::remove_file(&path);
    None
}

fn latest_log(store: &Instances, id: &str) -> PathBuf {
    store.game_dir(id).join("logs").join("latest.log")
}

/// Follows a game an earlier launcher started until it exits; its output comes from its log file.
pub async fn watch(
    store: &Instances,
    id: &str,
    game: Running,
    defaults: &LaunchSettings,
    report: impl Fn(Progress) + Send + Sync + 'static,
) -> Result<(), LaunchError> {
    let report: Report = Arc::new(report);
    let started = crate::parse_rfc3339(&game.started).unwrap_or_else(unix_now);
    report(Progress::Started {
        pid: game.pid,
        played: unix_now().saturating_sub(started),
    });
    let mut log = Tail::new(latest_log(store, id));
    let lines = |log: &mut Tail| {
        for text in log.read() {
            report(Progress::Line {
                stderr: false,
                text: crate::strip_ansi(&text),
            });
        }
    };
    while riven_game::alive(game.pid, game.process) {
        lines(&mut log);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    lines(&mut log);
    let ended = unix_now();
    let played = ended.saturating_sub(started);
    let settings = store.load(id)?.overrides.resolve(defaults);
    finish(store, id, played, ended, &settings).await;
    report(Progress::Exited { code: None, played });
    Ok(())
}

/// Reads whole lines appended to a file since the last read.
struct Tail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl Tail {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: Vec::new(),
        }
    }

    fn read(&mut self) -> Vec<String> {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        if file.metadata().is_ok_and(|m| m.len() < self.offset) {
            self.offset = 0;
            self.partial.clear();
        }
        let mut buf = Vec::new();
        if file.seek(SeekFrom::Start(self.offset)).is_err() || file.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        self.offset += buf.len() as u64;
        self.partial.extend_from_slice(&buf);
        let Some(end) = self.partial.iter().rposition(|&b| b == b'\n') else {
            return Vec::new();
        };
        let rest = self.partial.split_off(end + 1);
        let whole = std::mem::replace(&mut self.partial, rest);
        String::from_utf8_lossy(&whole)
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

pub fn stop(pid: u32) -> Result<(), LaunchError> {
    riven_game::kill(pid).map_err(|e| LaunchError::Stop(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_partial_lines_and_follows_a_rotated_log() {
        let dir = std::env::temp_dir().join(format!("riven-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("latest.log");
        let mut tail = Tail::new(path.clone());
        assert!(tail.read().is_empty());

        std::fs::write(&path, "one\ntw").unwrap();
        assert_eq!(tail.read(), ["one"]);
        std::fs::write(&path, "one\ntwo\r\nthree\n").unwrap();
        assert_eq!(tail.read(), ["two", "three"]);

        std::fs::write(&path, "new\n").unwrap();
        assert_eq!(tail.read(), ["new"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_reused_pid_is_not_the_game() {
        let pid = std::process::id();
        let started = riven_game::started(pid);
        assert!(started.is_some());
        assert!(riven_game::alive(pid, started));
        assert!(riven_game::alive(pid, None));
        assert!(!riven_game::alive(pid, started.map(|s| s + 1)));
    }
}
