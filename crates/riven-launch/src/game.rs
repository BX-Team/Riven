use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use riven_format::{
    Account, AccountKind, Instance, JavaChoice, LaunchSettings, LoaderKind, Release,
};
use riven_game::{Event, GameVersion, Loader, Step};
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
    /// The pack was installed for the first time.
    Installed {
        version: String,
    },
    /// The pack was updated to `version`.
    Updated {
        version: String,
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
    let before = install::load_state(&store.game_dir(id))?.map(|s| s.version);
    pending
        .apply(&files, &http, &store.game_dir(id), &progress)
        .await?;
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
        return Err(LaunchError::MicrosoftPending);
    }
    update_pack(store, id, &mut instance, &report).await?;

    let settings = instance.overrides.resolve(defaults);
    let game_dir = store.game_dir(id);
    std::fs::create_dir_all(&game_dir).map_err(io(&game_dir))?;
    if let Some(cmd) = &settings.commands.pre_launch {
        run_hook(cmd, &game_dir, id).await?;
    }
    if settings.commands.wrapper.is_some() {
        tracing::warn!("wrapper commands are not supported yet; starting java directly");
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
        session: riven_game::Session::offline(account.name.clone(), account.id.clone()),
        min_memory_mb: settings.memory.min,
        max_memory_mb: settings.memory.max,
        jvm_args: settings.jvm_args.clone(),
        window: riven_game::Window {
            width: settings.window.width,
            height: settings.window.height,
            fullscreen: settings.window.fullscreen,
        },
        launcher_name: "riven".into(),
        launcher_version: env!("CARGO_PKG_VERSION").into(),
    };
    riven_game::install(&http, &dirs, &prepared, &launch, &progress).await?;

    report(Progress::Stage(Stage::Starting));
    let game = riven_game::launch(&dirs, &prepared, &launch)?;
    report(Progress::Started { pid: game.pid() });
    let since = Instant::now();
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
    record_play(store, id, played);
    if let Some(cmd) = &settings.commands.post_exit
        && let Err(e) = run_hook(cmd, &game_dir, id).await
    {
        tracing::warn!("{e}");
    }
    report(Progress::Exited { code, played });
    Ok(())
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

pub fn stop(pid: u32) -> Result<(), LaunchError> {
    riven_game::kill(pid).map_err(|e| LaunchError::Stop(e.to_string()))
}
