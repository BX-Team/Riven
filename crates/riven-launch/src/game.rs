use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use lighty_launcher::auth::UserProfile;
use lighty_launcher::event::{
    ConsoleStream, Event, EventBus, EventReceiveError, JavaEvent, LaunchEvent,
};
use lighty_launcher::launch::{InstanceControl as _, Launch as _};
use lighty_launcher::{JavaDistribution, Loader, VersionBuilder};
use riven_format::{
    Account, AccountKind, Instance, JavaChoice, LaunchSettings, LoaderKind, Release,
};
use riven_sync::install;
use riven_sync::update::{self, Check, Pending, Request};

use crate::instances::Instances;
use crate::{LaunchError, io};

/// What a launch is busy with, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
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

fn lighty_ready() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        if let Err(e) = lighty_launcher::core::AppState::init("riven") {
            tracing::debug!("lighty paths: {e}");
        }
    });
}

fn loader_of(instance: &Instance) -> (Loader, String) {
    match &instance.loader {
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
    }
}

fn data_dir() -> Result<PathBuf, LaunchError> {
    riven_sync::data_dir().ok_or(LaunchError::NoDir("data"))
}

/// Links `<game>/libraries` and `<game>/assets` to shared folders where links are allowed.
fn share_downloads(game_dir: &Path, data: &Path) {
    for name in ["libraries", "assets"] {
        let link = game_dir.join(name);
        if link.symlink_metadata().is_ok() {
            continue;
        }
        let target = data.join(name);
        if std::fs::create_dir_all(&target).is_err() {
            continue;
        }
        if let Err(e) = crate::link_dir(&target, &link) {
            tracing::debug!("not sharing {name}: {e}");
        }
    }
}

/// lighty runs Java only from its own layout; a folder of links in that shape points it at the picked one.
fn custom_java_dir(java: &Path) -> Result<PathBuf, LaunchError> {
    let home = java
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| LaunchError::BadJava(java.to_owned()))?;
    if !java.is_file() {
        return Err(LaunchError::BadJava(java.to_owned()));
    }
    let digest = crate::short_hash(home.to_string_lossy().as_bytes());
    let dir = data_dir()?.join("java-custom").join(digest);
    let distributions = [
        JavaDistribution::Temurin,
        JavaDistribution::Zulu,
        JavaDistribution::Liberica,
    ];
    for (major, dist) in [8, 11, 16, 17, 21, 25]
        .into_iter()
        .flat_map(|m| distributions.iter().map(move |d| (m, d)))
    {
        let slot = dir.join(format!("{}_{major}", dist.get_name()));
        let link = slot.join("jdk");
        if link.symlink_metadata().is_ok() {
            continue;
        }
        std::fs::create_dir_all(&slot).map_err(io(&slot))?;
        crate::link_dir(home, &link).map_err(io(&link))?;
    }
    Ok(dir)
}

/// Runs a user command through the shell in the game directory; a failure stops the launch.
async fn run_hook(command: &str, game_dir: &Path, id: &str) -> Result<(), LaunchError> {
    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
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
    let state = pending.apply(&files, &http, &game_dir, &progress).await?;
    let names = crate::own::yield_to_pack(&game_dir, &release, &state.files)?;
    if !names.is_empty() {
        report(Progress::Replaced { names });
    }
    adopt_release(store, id, instance, &release)?;
    report(Progress::Updated {
        version: release.version,
    });
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

/// Turns lighty's events into [`Progress`]; Java reports running totals, game files report chunks.
#[derive(Default)]
struct Tracker {
    done: u64,
    total: u64,
}

impl Tracker {
    fn stage(&mut self, stage: Stage, total: u64, report: &Report) {
        self.done = 0;
        self.total = total;
        report(Progress::Stage(stage));
        self.amount(report);
    }

    fn amount(&self, report: &Report) {
        report(Progress::Amount {
            done: self.done.min(self.total),
            total: self.total,
        });
    }

    fn forward(&mut self, event: Event, id: &str, report: &Report) -> Option<i32> {
        match event {
            Event::Loader(_) => report(Progress::Stage(Stage::Metadata)),
            Event::Java(JavaEvent::JavaDownloadStarted { total_bytes, .. }) => {
                self.stage(Stage::Java, total_bytes, report)
            }
            Event::Java(JavaEvent::JavaDownloadProgress { bytes }) => {
                self.done = bytes;
                self.amount(report);
            }
            Event::Launch(LaunchEvent::InstallStarted { total_bytes, .. }) => {
                self.stage(Stage::Game, total_bytes, report)
            }
            Event::Launch(LaunchEvent::InstallProgress { bytes }) => {
                self.done += bytes;
                self.amount(report);
            }
            Event::Launch(
                LaunchEvent::InstallCompleted { .. } | LaunchEvent::IsInstalled { .. },
            ) => report(Progress::Stage(Stage::Starting)),
            Event::InstanceLaunched(e) if e.instance_name == id => {
                report(Progress::Started { pid: e.pid })
            }
            Event::ConsoleOutput(line) if line.instance_name == id => report(Progress::Line {
                stderr: line.stream == ConsoleStream::Stderr,
                text: crate::strip_ansi(&line.line),
            }),
            Event::InstanceExited(e) if e.instance_name == id => {
                return Some(e.exit_code.unwrap_or(-1));
            }
            _ => {}
        }
        None
    }
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
        return Err(LaunchError::MicrosoftPending);
    }
    update_pack(store, id, &mut instance, &report).await?;

    let settings = instance.overrides.resolve(defaults);
    let game_dir = store.game_dir(id);
    std::fs::create_dir_all(&game_dir).map_err(io(&game_dir))?;
    let data = data_dir()?;
    share_downloads(&game_dir, &data);
    if let Some(cmd) = &settings.commands.pre_launch {
        run_hook(cmd, &game_dir, id).await?;
    }
    if settings.commands.wrapper.is_some() {
        tracing::warn!("wrapper commands are not supported yet; starting java directly");
    }

    lighty_ready();
    report(Progress::Stage(Stage::Metadata));
    let (loader, loader_version) = loader_of(&instance);
    let mut version = VersionBuilder::new(id, loader, &loader_version, &instance.minecraft);
    version.game_dirs = game_dir.clone();
    version.runtime_dir = game_dir.clone();
    version.java_dirs = match &settings.java {
        JavaChoice::Auto => data.join("java"),
        JavaChoice::Path { path } => custom_java_dir(Path::new(path))?,
    };

    let bus = EventBus::new(4096);
    let mut events = bus.subscribe();
    let watcher = {
        let report = report.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let mut tracker = Tracker::default();
            loop {
                match events.next().await {
                    Ok(event) => {
                        if let Some(code) = tracker.forward(event, &id, &report) {
                            return Some(code);
                        }
                    }
                    Err(EventReceiveError::Lagged { .. }) => {}
                    Err(EventReceiveError::BusDropped) => return None,
                }
            }
        })
    };
    let profile = UserProfile::offline(account.name.clone(), account.id.clone());
    if let Err(e) = launch(&mut version, &profile, &settings, &bus).await {
        watcher.abort();
        return Err(LaunchError::Game(e));
    }
    let since = Instant::now();
    let code = watcher.await.ok().flatten();
    let played = since.elapsed().as_secs();
    record_play(store, id, played);
    if let Some(cmd) = &settings.commands.post_exit
        && let Err(e) = run_hook(cmd, &game_dir, id).await
    {
        tracing::warn!("{e}");
    }
    report(Progress::Exited { code, played });
    Ok(())
}

/// JVM arguments as lighty option keys: it re-adds one `-` and loses order, so `--add-opens x` becomes `--add-opens=x`.
fn jvm_keys(args: &[String]) -> Vec<String> {
    let mut keys = Vec::new();
    let mut args = args
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .peekable();
    while let Some(arg) = args.next() {
        let Some(key) = arg.strip_prefix('-') else {
            tracing::warn!("ignoring JVM argument `{arg}`: it does not start with -");
            continue;
        };
        let pairs = key.starts_with('-') && !key.contains('=');
        match args.peek() {
            Some(value) if pairs && !value.starts_with('-') => {
                keys.push(format!("{key}={value}"));
                args.next();
            }
            _ => keys.push(key.to_owned()),
        }
    }
    keys
}

async fn launch(
    version: &mut VersionBuilder<Loader>,
    profile: &UserProfile,
    settings: &LaunchSettings,
    bus: &EventBus,
) -> Result<(), String> {
    let mut jvm = version
        .launch(profile, JavaDistribution::Temurin)
        .with_event_bus(bus)
        .with_jvm_options()
        .set("Xms", format!("{}M", settings.memory.min))
        .set("Xmx", format!("{}M", settings.memory.max));
    for key in jvm_keys(&settings.jvm_args) {
        jvm = jvm.set(key, "");
    }
    let mut args = jvm.done().with_arguments();
    if settings.window.fullscreen {
        args = args.set("fullscreen", "");
    } else {
        args = args
            .set("width", settings.window.width.to_string())
            .set("height", settings.window.height.to_string());
    }
    args.done().run().await.map_err(|e| e.to_string())
}

fn record_play(store: &Instances, id: &str, seconds: u64) {
    let Ok(mut instance) = store.load(id) else {
        return;
    };
    instance.play_seconds += seconds;
    instance.last_played = Some(crate::now_rfc3339());
    if let Err(e) = store.save(id, &instance) {
        tracing::warn!("{e}");
    }
}

/// Asks a running game to close, then kills it.
pub async fn stop(id: &str, pid: u32) -> Result<(), LaunchError> {
    lighty_ready();
    let version = VersionBuilder::new(id, Loader::Vanilla, "", "");
    version
        .close_instance(pid)
        .await
        .map_err(|e| LaunchError::Game(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jvm_arguments_keep_their_dashes_and_pairs() {
        let args: Vec<String> = [
            "-XX:+UseG1GC",
            "-Dfml.ignorePatchDiscrepancies=true",
            "--add-opens",
            "java.base/java.lang=ALL-UNNAMED",
            "--add-exports=java.base/sun.nio.ch=ALL-UNNAMED",
            "--enable-preview",
            "-Xss2M",
            "stray",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            jvm_keys(&args),
            [
                "XX:+UseG1GC",
                "Dfml.ignorePatchDiscrepancies=true",
                "-add-opens=java.base/java.lang=ALL-UNNAMED",
                "-add-exports=java.base/sun.nio.ch=ALL-UNNAMED",
                "-enable-preview",
                "Xss2M",
            ]
        );
    }
}
