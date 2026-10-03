mod author;
mod import;
mod output;
mod project;
mod release;

use std::path::PathBuf;
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
        /// Modrinth slug, id, search query or URL; a GitHub repo or release URL; a file URL; a local file.
        query: String,
        /// Where `query` points (default: guessed from it).
        #[arg(long, value_enum)]
        source: Option<SourceArg>,
        /// GitHub asset name pattern, `*` matching the version: `mymod-neoforge-*.jar`.
        #[arg(long)]
        asset: Option<String>,
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
        /// Point a URL entry at a new file.
        #[arg(long)]
        url: Option<String>,
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
        /// Path or URL of the archive or pack.
        source: String,
    },
    /// Bump the pack version.
    Bump {
        /// `major`, `minor`, `patch` or an explicit version.
        #[arg(default_value = "patch")]
        to: String,
    },
    /// Create the signing key for this pack, or show the existing one.
    Keygen,
    /// Manage keys pinned for installed packs.
    #[command(subcommand)]
    Trust(TrustCommand),
    /// Build the current version into `dist/` and point a channel at it.
    Build {
        #[arg(long, default_value = "stable")]
        channel: String,
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        /// Build without a signature.
        #[arg(long)]
        unsigned: bool,
    },
    /// Point a channel at an already built release; also how to roll back.
    Publish {
        channel: String,
        version: String,
        #[arg(long, default_value = "dist")]
        out: PathBuf,
    },
    /// Commit `dist/` to a branch served by GitHub Pages and push it.
    Deploy {
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        #[arg(long, default_value = "gh-pages")]
        branch: String,
        #[arg(long, default_value = "origin")]
        remote: String,
        /// Only commit to the local branch.
        #[arg(long)]
        no_push: bool,
    },
    /// Export the pack for other launchers.
    Export {
        #[arg(value_enum)]
        format: ExportFormat,
        /// Only content for this side (Prism defaults to client).
        #[arg(long, value_enum)]
        side: Option<InstallSideArg>,
        /// Output file (default: exports/<id>-<version>.<ext>).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum TrustCommand {
    /// Show pinned keys.
    List,
    /// Forget the key pinned for a pack URL, to accept a rotated key.
    Reset { url: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportFormat {
    Mrpack,
    Prism,
}

#[derive(Clone, Copy, ValueEnum)]
enum InstallSideArg {
    Client,
    Server,
}

impl From<InstallSideArg> for riven_format::InstallSide {
    fn from(side: InstallSideArg) -> Self {
        match side {
            InstallSideArg::Client => Self::Client,
            InstallSideArg::Server => Self::Server,
        }
    }
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
    /// A packwiz pack: its directory, `pack.toml`, or the URL of `pack.toml`.
    Packwiz,
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
            asset,
            side,
            group,
            pin,
        } => {
            let options = author::AddArgs {
                source,
                asset,
                side: side.map(Into::into),
                group,
                pin,
            };
            author::add(out, &query, options).await
        }
        Command::Remove { id, keep_deps } => author::remove(out, &id, keep_deps).await,
        Command::Update { ids, dry_run, url } => {
            author::update(out, &ids, dry_run, url.as_deref()).await
        }
        Command::Pin { id } => author::set_pinned(out, &id, true),
        Command::Unpin { id } => author::set_pinned(out, &id, false),
        Command::Side { id, side } => author::set_side(out, &id, side.into()),
        Command::Group(group) => author::group(out, group),
        Command::List { tree, outdated } => author::list(out, tree, outdated).await,
        Command::Why { id } => author::why(out, &id),
        Command::Check => author::check(out).await,
        Command::Import { format, source } => import::import(out, format, &source).await,
        Command::Bump { to } => author::bump(out, &to),
        Command::Keygen => release::keygen(out),
        Command::Trust(command) => release::trust(out, command),
        Command::Build {
            channel,
            out: dist,
            unsigned,
        } => release::build(out, &channel, &dist, unsigned).await,
        Command::Publish {
            channel,
            version,
            out: dist,
        } => release::publish(out, &channel, &version, &dist),
        Command::Deploy {
            out: dist,
            branch,
            remote,
            no_push,
        } => release::deploy(out, &dist, &branch, &remote, !no_push),
        Command::Export {
            format,
            side,
            out: path,
        } => release::export(out, format, side.map(Into::into), path).await,
    }
}
