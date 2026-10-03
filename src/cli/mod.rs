mod author;
mod output;
mod project;

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use riven_format::{LoaderKind, Side};

use output::Output;

#[derive(Parser)]
#[command(
    name = "riven",
    version,
    about = "Riven Launcher — Minecraft launcher and modpack toolkit"
)]
struct Cli {
    /// Print machine-readable JSON to stdout.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create `riven.json` in the current directory.
    Init {
        /// Minecraft version (default: latest release).
        #[arg(long)]
        mc: Option<String>,
        /// Mod loader, optionally with a version: `neoforge`, `fabric@0.16.10`.
        #[arg(long, default_value = "neoforge")]
        loader: String,
    },
    /// Add content and its dependencies.
    Add {
        /// Slug, project id, search query, or a Modrinth project/version URL.
        query: String,
        #[arg(long, value_enum, default_value_t = SourceArg::Modrinth)]
        source: SourceArg,
        #[arg(long, value_enum)]
        side: Option<SideArg>,
        #[arg(long)]
        group: Option<String>,
        /// Keep this version on `riven update`.
        #[arg(long)]
        pin: bool,
    },
    /// Remove an entry and the dependencies only it needed.
    Remove {
        id: String,
        #[arg(long)]
        keep_deps: bool,
    },
    /// Update entries (all `follow` ones by default) to their newest versions.
    Update {
        ids: Vec<String>,
        /// Show the changes without writing `riven.json`.
        #[arg(long)]
        dry_run: bool,
    },
    /// Keep an entry at its current version.
    Pin { id: String },
    /// Let `riven update` move an entry again.
    Unpin { id: String },
    /// Set where an entry is installed.
    Side {
        id: String,
        #[arg(value_enum)]
        side: SideArg,
    },
    /// Manage optional groups.
    #[command(subcommand)]
    Group(GroupCommand),
    /// List the pack's content.
    List {
        /// Show explicit entries with what they require.
        #[arg(long)]
        tree: bool,
        /// Only entries with a newer compatible version.
        #[arg(long)]
        outdated: bool,
    },
    /// Show what requires an entry.
    Why { id: String },
    /// Check dependencies, conflicts and loader mismatches.
    Check,
    /// Create `riven.json` from an existing modpack archive.
    Import {
        #[arg(value_enum)]
        format: ImportFormat,
        /// Path or URL of the archive.
        source: String,
    },
    /// Bump the pack version.
    Bump {
        /// `major`, `minor`, `patch` or an explicit version.
        #[arg(default_value = "patch")]
        to: String,
    },
}

#[derive(Subcommand)]
enum GroupCommand {
    /// Define a group.
    Add {
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: Option<String>,
        /// Enabled unless the user opts out.
        #[arg(long)]
        default: bool,
    },
    /// Delete a group; its entries become unconditional.
    Rm { id: String },
    /// Put an entry into a group, or `none` to take it out.
    Set { entry: String, group: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum ImportFormat {
    Mrpack,
}

#[derive(Clone, Copy, ValueEnum)]
enum SourceArg {
    Modrinth,
    Github,
    Url,
    Local,
}

#[derive(Clone, Copy, ValueEnum)]
enum SideArg {
    Client,
    Server,
    Both,
}

impl From<SideArg> for Side {
    fn from(side: SideArg) -> Self {
        match side {
            SideArg::Client => Side::Client,
            SideArg::Server => Side::Server,
            SideArg::Both => Side::Both,
        }
    }
}

fn parse_loader(raw: &str) -> anyhow::Result<(LoaderKind, Option<String>)> {
    let (name, version) = match raw.split_once('@') {
        Some((name, version)) => (name, Some(version.to_owned())),
        None => (raw, None),
    };
    let kind = match name.to_ascii_lowercase().as_str() {
        "fabric" => LoaderKind::Fabric,
        "quilt" => LoaderKind::Quilt,
        "forge" => LoaderKind::Forge,
        "neoforge" => LoaderKind::NeoForge,
        other => anyhow::bail!("unknown loader `{other}` (fabric, quilt, forge, neoforge)"),
    };
    Ok((kind, version))
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let out = Output::new(cli.json);
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime starts");
    match runtime.block_on(dispatch(cli.command, &out)) {
        Ok(code) => code,
        Err(e) => {
            out.error(&e);
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(command: Command, out: &Output) -> anyhow::Result<ExitCode> {
    match command {
        Command::Init { mc, loader } => author::init(out, mc, &loader).await,
        Command::Add {
            query,
            source,
            side,
            group,
            pin,
        } => author::add(out, &query, source, side.map(Into::into), group, pin).await,
        Command::Remove { id, keep_deps } => author::remove(out, &id, keep_deps).await,
        Command::Update { ids, dry_run } => author::update(out, &ids, dry_run).await,
        Command::Pin { id } => author::set_pinned(out, &id, true),
        Command::Unpin { id } => author::set_pinned(out, &id, false),
        Command::Side { id, side } => author::set_side(out, &id, side.into()),
        Command::Group(group) => author::group(out, group),
        Command::List { tree, outdated } => author::list(out, tree, outdated).await,
        Command::Why { id } => author::why(out, &id),
        Command::Check => author::check(out).await,
        Command::Import { format, source } => author::import(out, format, &source).await,
        Command::Bump { to } => author::bump(out, &to),
    }
}
