use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};
use tokio::sync::mpsc;

use crate::forge::{Installer, Kind};
use crate::install;
use crate::version::Features;
use crate::{Dirs, Error, Prepared, io, os};

/// Who plays: what the game is told about the player and their sign-in.
#[derive(Clone)]
pub struct Session {
    pub name: String,
    pub uuid: String,
    pub access_token: String,
    pub xuid: String,
    /// `msa` for Microsoft accounts, `legacy` for offline ones.
    pub user_type: String,
    pub client_id: String,
}

impl Session {
    pub fn offline(name: impl Into<String>, uuid: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            uuid: uuid.into(),
            access_token: "0".into(),
            xuid: "0".into(),
            user_type: "legacy".into(),
            client_id: String::new(),
        }
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("name", &self.name)
            .field("uuid", &self.uuid)
            .field("user_type", &self.user_type)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
}

#[derive(Debug, Clone)]
pub struct Launch {
    pub java: PathBuf,
    pub game_dir: PathBuf,
    /// Unpacked native libraries; emptied on every launch.
    pub natives_dir: PathBuf,
    pub session: Session,
    pub min_memory_mb: u32,
    pub max_memory_mb: u32,
    /// Extra JVM arguments, one per element.
    pub jvm_args: Vec<String>,
    pub window: Window,
    /// Commands the game starts through, such as `gamemoderun`; empty starts Java directly.
    pub wrapper: Vec<String>,
    pub launcher_name: String,
    pub launcher_version: String,
}

#[derive(Debug)]
pub struct Game {
    child: tokio::process::Child,
    pid: u32,
}

/// Starts the game; its output is read through [`Game::wait`].
pub fn launch(dirs: &Dirs, prepared: &Prepared, launch: &Launch) -> Result<Game, Error> {
    let args = arguments(dirs, prepared, launch);
    std::fs::create_dir_all(&launch.game_dir).map_err(io(&launch.game_dir))?;
    let (program, prefix) = match launch.wrapper.split_first() {
        Some((first, rest)) => (
            PathBuf::from(first),
            rest.iter()
                .cloned()
                .chain([launch.java.display().to_string()])
                .collect(),
        ),
        None => (launch.java.clone(), Vec::new()),
    };
    let mut command = tokio::process::Command::new(&program);
    command
        .args(prefix)
        .args(args)
        .current_dir(&launch.game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if prepared.before_1_6() {
        // Games before 1.6 find `.minecraft` themselves; this keeps it inside the game folder.
        command.env("APPDATA", &launch.game_dir);
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let child = command
        .spawn()
        .map_err(|source| Error::Spawn { program, source })?;
    let pid = child.id().unwrap_or_default();
    Ok(Game { child, pid })
}

impl Game {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Hands every output line to `on_line` (`true` for stderr) and returns the exit code.
    pub async fn wait(mut self, on_line: impl Fn(bool, String)) -> Option<i32> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        if let Some(out) = self.child.stdout.take() {
            tokio::spawn(lines(out, false, tx.clone()));
        }
        if let Some(err) = self.child.stderr.take() {
            tokio::spawn(lines(err, true, tx.clone()));
        }
        drop(tx);
        // Processes the game leaves behind may hold its pipes open long after it exits.
        let status = loop {
            tokio::select! {
                Some((stderr, line)) = rx.recv() => on_line(stderr, line),
                status = self.child.wait() => break status.ok()?,
            }
        };
        while let Ok(Some((stderr, line))) =
            tokio::time::timeout(Duration::from_millis(300), rx.recv()).await
        {
            on_line(stderr, line);
        }
        #[cfg(unix)]
        let signal = std::os::unix::process::ExitStatusExt::signal(&status).map(|s| 128 + s);
        #[cfg(not(unix))]
        let signal = None;
        status.code().or(signal)
    }
}

/// Lines as they come; invalid UTF-8 is replaced rather than ending the stream.
async fn lines(
    stream: impl AsyncRead + Unpin,
    stderr: bool,
    tx: mpsc::UnboundedSender<(bool, String)>,
) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf)
                    .trim_end_matches(['\r', '\n'])
                    .to_owned();
                if tx.send((stderr, line)).is_err() {
                    return;
                }
            }
        }
    }
}

/// The whole command line after `java`: JVM options, main class and game arguments.
pub fn arguments(dirs: &Dirs, prepared: &Prepared, launch: &Launch) -> Vec<String> {
    let version = &prepared.version;
    let features = Features {
        custom_resolution: !launch.window.fullscreen,
    };
    let index_id = version
        .asset_index
        .as_ref()
        .map(|i| i.id.clone())
        .or_else(|| version.assets.clone())
        .unwrap_or_else(|| "legacy".into());
    let game_assets = install::read_index(dirs, &index_id)
        .and_then(|index| install::legacy_assets_dir(dirs, &index_id, &index, &launch.game_dir))
        .unwrap_or_else(|| dirs.assets());

    let mut jvm = vec![
        format!("-Xms{}M", launch.min_memory_mb),
        format!("-Xmx{}M", launch.max_memory_mb),
    ];
    let user: Vec<String> = launch
        .jvm_args
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .collect();
    if user.is_empty() {
        jvm.extend(
            version
                .arguments
                .iter()
                .flat_map(|a| &a.default_user_jvm)
                .flat_map(|a| a.values(features))
                .filter(|a| !a.starts_with("-Xms") && !a.starts_with("-Xmx"))
                .cloned(),
        );
    }
    if prepared.before_1_6() {
        jvm.push(format!("-Duser.home={}", launch.game_dir.display()));
    }
    if let Some(Installer {
        kind: Kind::Legacy { .. },
        ..
    }) = &prepared.installer
    {
        jvm.push("-Dfml.ignoreInvalidMinecraftCertificates=true".into());
        jvm.push("-Dfml.ignorePatchDiscrepancies=true".into());
    }
    match version.arguments.as_ref().filter(|a| !a.jvm.is_empty()) {
        Some(args) => jvm.extend(args.jvm.iter().flat_map(|a| a.values(features)).cloned()),
        None => jvm.extend(legacy_jvm()),
    }
    for (property, var) in [
        ("-Dminecraft.launcher.brand=", "${launcher_name}"),
        ("-Dminecraft.launcher.version=", "${launcher_version}"),
    ] {
        if !jvm.iter().any(|a| a.starts_with(property)) {
            jvm.insert(2, format!("{property}{var}"));
        }
    }
    jvm.extend(user);

    let mut game: Vec<String> = match (&version.arguments, &version.minecraft_arguments) {
        (Some(args), _) if !args.game.is_empty() => args
            .game
            .iter()
            .flat_map(|a| a.values(features))
            .cloned()
            .collect(),
        (_, Some(line)) => {
            let mut game: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
            if features.custom_resolution {
                game.extend(
                    [
                        "--width",
                        "${resolution_width}",
                        "--height",
                        "${resolution_height}",
                    ]
                    .map(String::from),
                );
            }
            game
        }
        _ => Vec::new(),
    };
    if launch.window.fullscreen {
        game.push("--fullscreen".into());
    }

    let session = &launch.session;
    let uuid = session.uuid.replace('-', "");
    let auth_session = if session.user_type == "msa" {
        format!("token:{}:{uuid}", session.access_token)
    } else {
        "-".into()
    };
    let path = |p: &Path| p.display().to_string();
    let mut vars = BTreeMap::from([
        ("auth_player_name", session.name.clone()),
        ("auth_uuid", uuid),
        ("auth_access_token", session.access_token.clone()),
        ("auth_session", auth_session),
        ("auth_xuid", session.xuid.clone()),
        ("clientid", session.client_id.clone()),
        ("user_type", session.user_type.clone()),
        ("user_properties", "{}".into()),
        ("version_name", prepared.minecraft.clone()),
        (
            "version_type",
            version.kind.clone().unwrap_or_else(|| "release".into()),
        ),
        ("game_directory", path(&launch.game_dir)),
        ("assets_root", path(&dirs.assets())),
        ("game_assets", path(&game_assets)),
        ("assets_index_name", index_id),
        ("natives_directory", path(&launch.natives_dir)),
        ("library_directory", path(&dirs.libraries())),
        ("classpath_separator", os::CLASSPATH_SEPARATOR.into()),
        ("launcher_name", launch.launcher_name.clone()),
        ("launcher_version", launch.launcher_version.clone()),
        ("resolution_width", launch.window.width.to_string()),
        ("resolution_height", launch.window.height.to_string()),
    ]);

    let mut classpath = install::libraries(dirs, prepared).classpath;
    classpath.push(prepared.client_jar(dirs));
    let modules: HashSet<String> = jvm
        .iter()
        .position(|a| a == "-p" || a == "--module-path")
        .and_then(|i| jvm.get(i + 1))
        .map(|value| {
            substitute(value, &vars)
                .split(os::CLASSPATH_SEPARATOR)
                .map(same_path)
                .collect()
        })
        .unwrap_or_default();
    let classpath: Vec<String> = classpath
        .iter()
        .map(|p| path(p))
        .filter(|p| !modules.contains(&same_path(p)))
        .collect();
    vars.insert("classpath", classpath.join(os::CLASSPATH_SEPARATOR));

    let main_class = version
        .main_class
        .clone()
        .unwrap_or_else(|| "net.minecraft.client.main.Main".into());
    jvm.iter()
        .map(|a| substitute(a, &vars))
        .chain([main_class])
        .chain(game.iter().map(|a| substitute(a, &vars)))
        .collect()
}

/// A path in one spelling: on Windows library paths mix `\\` and the `/` of Maven paths.
fn same_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// JVM options for versions whose metadata lists none, as Mojang's launcher adds them.
fn legacy_jvm() -> Vec<String> {
    let mut args = Vec::new();
    if cfg!(target_os = "macos") {
        args.push("-XstartOnFirstThread".into());
    }
    if cfg!(windows) {
        args.push(
            "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump"
                .into(),
        );
    }
    if cfg!(target_arch = "x86") {
        args.push("-Xss1M".into());
    }
    args.extend(
        [
            "-Djava.library.path=${natives_directory}",
            "-cp",
            "${classpath}",
        ]
        .map(String::from),
    );
    args
}

/// Replaces `${name}` with its value; unknown names stay as they are.
fn substitute(arg: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(start) = rest.find("${") {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let key = &rest[start + 2..start + len];
        out.push_str(&rest[..start]);
        match vars.get(key) {
            Some(value) => out.push_str(value),
            None => out.push_str(&rest[start..=start + len]),
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn exit_is_seen_while_a_leftover_process_holds_the_pipes() {
        let child = tokio::process::Command::new("sh")
            .args(["-c", "sleep 30 & echo bye; exit 3"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let game = Game { child, pid };
        let seen = std::sync::Mutex::new(Vec::new());
        let code = tokio::time::timeout(
            Duration::from_secs(5),
            game.wait(|_, line| seen.lock().unwrap().push(line)),
        )
        .await
        .expect("wait returns once the game exits");
        assert_eq!(code, Some(3));
        assert_eq!(*seen.lock().unwrap(), ["bye"]);
    }

    fn prepared(minecraft: &str, json: &str) -> Prepared {
        Prepared {
            version: serde_json::from_str(json).unwrap(),
            minecraft: minecraft.into(),
            installer: None,
        }
    }

    fn launch(jvm_args: &[&str]) -> Launch {
        Launch {
            java: "java".into(),
            game_dir: "/game".into(),
            natives_dir: "/natives".into(),
            session: Session {
                name: "Steve".into(),
                uuid: "b50ad385-829d-3141-a216-7e7d7539ba7f".into(),
                access_token: "secret".into(),
                xuid: "42".into(),
                user_type: "msa".into(),
                client_id: "client".into(),
            },
            min_memory_mb: 1024,
            max_memory_mb: 2048,
            jvm_args: jvm_args.iter().map(|a| a.to_string()).collect(),
            window: Window {
                width: 854,
                height: 480,
                fullscreen: false,
            },
            wrapper: vec![],
            launcher_name: "riven".into(),
            launcher_version: "1.0".into(),
        }
    }

    fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
        let i = args.iter().position(|a| a == flag).unwrap();
        &args[i + 1]
    }

    #[test]
    fn old_versions_get_default_jvm_options_and_a_resolution() {
        let dirs = Dirs::new("/data");
        let p = prepared(
            "1.5.2",
            r#"{"mainClass":"net.minecraft.launchwrapper.Launch",
                "minecraftArguments":"${auth_player_name} ${auth_session} --gameDir ${game_directory}",
                "libraries":[{"name":"net.minecraft:launchwrapper:1.5"}]}"#,
        );
        let args = arguments(&dirs, &p, &launch(&["-XX:+UseG1GC"]));
        assert_eq!(&args[..2], ["-Xms1024M", "-Xmx2048M"]);
        assert!(args.contains(&format!(
            "-Djava.library.path={}",
            Path::new("/natives").display()
        )));
        let classpath: Vec<&str> = after(&args, "-cp").split(os::CLASSPATH_SEPARATOR).collect();
        assert_eq!(
            classpath,
            [
                dirs.libraries()
                    .join("net/minecraft/launchwrapper/1.5/launchwrapper-1.5.jar"),
                dirs.versions().join("1.5.2").join("1.5.2.jar"),
            ]
            .map(|p| p.display().to_string())
        );
        let main = args
            .iter()
            .position(|a| a == "net.minecraft.launchwrapper.Launch")
            .unwrap();
        assert_eq!(args[main - 1], "-XX:+UseG1GC");
        assert_eq!(
            &args[main + 1..],
            [
                "Steve",
                "token:secret:b50ad385829d3141a2167e7d7539ba7f",
                "--gameDir",
                &Path::new("/game").display().to_string(),
                "--width",
                "854",
                "--height",
                "480",
            ]
        );
    }

    #[test]
    fn module_path_jars_leave_the_classpath() {
        let dirs = Dirs::new("/data");
        let p = prepared(
            "1.20.1",
            r#"{"mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher",
                "arguments":{
                    "jvm":["-DignoreList=${version_name}.jar",
                           "-p","${library_directory}/cpw/mods/bootstraplauncher/1.1.2/bootstraplauncher-1.1.2.jar",
                           "-cp","${classpath}"],
                    "default-user-jvm":[{"value":["-Xmx4G","-XX:+UseZGC"]}],
                    "game":["--uuid","${auth_uuid}","--xuid","${auth_xuid}"]},
                "libraries":[{"name":"cpw.mods:bootstraplauncher:1.1.2"},{"name":"com.google.guava:guava:32.1.2"}]}"#,
        );
        let args = arguments(&dirs, &p, &launch(&[]));
        assert!(args.contains(&"-DignoreList=1.20.1.jar".to_owned()));
        assert!(args.contains(&"-XX:+UseZGC".to_owned()));
        assert!(!args.contains(&"-Xmx4G".to_owned()));
        assert_eq!(
            after(&args, "-cp"),
            [
                dirs.libraries()
                    .join("com/google/guava/guava/32.1.2/guava-32.1.2.jar"),
                dirs.versions().join("1.20.1").join("1.20.1.jar"),
            ]
            .map(|p| p.display().to_string())
            .join(os::CLASSPATH_SEPARATOR)
        );
        assert_eq!(
            same_path(r"C:\data\libraries/cpw/mods/a.jar"),
            same_path(r"C:\data\libraries\cpw/mods/a.jar"),
            "Windows module paths match their classpath entries"
        );
        assert_eq!(after(&args, "--uuid"), "b50ad385829d3141a2167e7d7539ba7f");
        assert_eq!(after(&args, "--xuid"), "42");
        assert!(!args.iter().any(|a| a.contains("--width")));
    }
}
