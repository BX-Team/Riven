use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use super::markdown::{self, Described};
use futures_util::{StreamExt as _, stream};
use gpui_kit::base::Scrollbar;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, ExternalPaths, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle, Window,
    div, img, px, relative, uniform_list,
};
use riven_format::{Loader, LoaderKind, Release};
use riven_sources::{Cache, Hit, Modrinth, Version};
use riven_sync::update::{self, Request};
use rust_i18n::t;
use tokio::sync::mpsc;

use super::dialogs::{dialog_shell, field, game_meta, loader_label};
use super::launch_bar::loader_display;
use super::mods;
use super::runtime;
use super::state::{AppState, Route};
use moonglass::theme::ActiveTheme as _;
use moonglass::{
    Button, ButtonSize, Dropdown, IconName, MenuItem, Switch, Tabs, TextField, h_flex, icon,
    motion, scrollbar, v_flex,
};

const WIDTH: f32 = 1080.;
const SIDE_WIDTH: f32 = 380.;
const RESULTS: u32 = 40;
const SEARCH_DELAY: Duration = Duration::from_millis(250);
const PARALLEL_ICONS: usize = 8;
const HIT_HEIGHT: f32 = 58.;

const LOADERS: [Option<LoaderKind>; 5] = [
    None,
    Some(LoaderKind::NeoForge),
    Some(LoaderKind::Fabric),
    Some(LoaderKind::Quilt),
    Some(LoaderKind::Forge),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Modrinth,
    Custom,
    Import,
}

const MODES: [Mode; 3] = [Mode::Modrinth, Mode::Custom, Mode::Import];

/// Something fetched for a project the first time it is shown.
#[derive(Clone)]
enum Fetched<T> {
    Loading,
    Ready(T),
    Failed,
}

/// A pack checked and ready to install: its release and where the installer reads it from.
struct Prepared {
    release: Release,
    source: String,
    icon: Option<Vec<u8>>,
    /// The name the link's page gives the pack.
    title: Option<String>,
    modrinth: Option<riven_format::ModrinthPack>,
    github: Option<riven_format::GitHubPack>,
}

/// "New instance": a Modrinth modpack, an empty game, or a pack from a link or file.
pub struct NewInstance {
    mode: Mode,
    query: Entity<InputState>,
    hits: Vec<Hit>,
    icons: HashMap<String, PathBuf>,
    searching: bool,
    search_serial: u64,
    focus: Option<Hit>,
    pages: HashMap<String, Fetched<Rc<Described>>>,
    versions: HashMap<String, Fetched<Rc<Vec<Version>>>>,
    /// The picked version id of the focused modpack; none picks the newest.
    version: Option<SharedString>,
    scroll: UniformListScrollHandle,

    name: Entity<InputState>,
    mc_search: Entity<InputState>,
    mc_scroll: UniformListScrollHandle,
    releases: Vec<SharedString>,
    minecraft: Option<SharedString>,
    loader: usize,
    loader_versions: Vec<SharedString>,
    loader_version: Option<SharedString>,
    /// The build the loader suggests for the picked game version.
    loader_recommended: Option<SharedString>,
    loader_loading: bool,
    loader_scroll: UniformListScrollHandle,
    loader_serial: u64,
    /// A picked icon: a preview file and the PNG to store.
    icon: Option<(PathBuf, Vec<u8>)>,

    link: Entity<InputState>,
    prepared: Option<Prepared>,
    groups: BTreeMap<String, bool>,

    /// What is running, shown with a spinner; actions wait for it.
    busy: Option<SharedString>,
    /// Stops what is running; closing the dialog stops it too.
    abort: Option<runtime::Abort>,
    error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

impl Drop for NewInstance {
    fn drop(&mut self) {
        if let Some(abort) = &self.abort {
            abort.abort();
        }
    }
}

fn modrinth() -> Modrinth {
    let modrinth = Modrinth::new(riven_sources::client());
    match mods::cache_dir() {
        Some(dir) => modrinth.with_cache(Cache::new(dir.join("api"), Duration::from_secs(600))),
        None => modrinth,
    }
}

fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f32 / 1e3),
        _ => format!("{:.1}M", n as f32 / 1e6),
    }
}

fn pretty_loader(name: &str) -> String {
    match name {
        "neoforge" => "NeoForge".into(),
        "fabric" => "Fabric".into(),
        "quilt" => "Quilt".into(),
        "forge" => "Forge".into(),
        other => other.to_owned(),
    }
}

/// "1.21.1 · Fabric" for a hit or version.
fn target_label(game_versions: &[String], loaders: &[String]) -> String {
    let mut parts: Vec<String> = game_versions.last().cloned().into_iter().collect();
    parts.extend(loaders.iter().map(|l| pretty_loader(l)));
    parts.join(" · ")
}

fn packs_dir() -> Result<PathBuf, String> {
    riven_launch::modpack::packs_dir().map_err(|_| t!("new_instance.no_data_dir").to_string())
}

/// Copies a picked `.riven` next to converted packs, so the instance keeps working once the original is gone.
fn keep_archive(path: &std::path::Path) -> Result<String, String> {
    let dir = packs_dir()?;
    if path.parent() == Some(dir.as_path()) {
        return Ok(path.display().to_string());
    }
    let name = path
        .file_name()
        .ok_or_else(|| t!("new_instance.bad_file").to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let kept = dir.join(name);
    std::fs::copy(path, &kept).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(kept.display().to_string())
}

/// Reads the release a link, a `.riven` file or a converted `.mrpack` points at.
async fn preview(source: String) -> Result<Release, String> {
    let store = riven_sync::Store::default_location()
        .ok_or_else(|| t!("new_instance.no_data_dir").to_string())?;
    update::preview(&store, &riven_sources::client(), &source, None)
        .await
        .map_err(|e| e.to_string())
}

fn import_stage(stage: riven_build::import::ImportStage) -> SharedString {
    use riven_build::import::ImportStage;
    match stage {
        ImportStage::Reading => t!("new_instance.stage_reading"),
        ImportStage::Identifying { files } => t!("new_instance.stage_identifying", n = files),
        ImportStage::Downloading => t!("new_instance.stage_downloading"),
        ImportStage::Writing => t!("new_instance.stage_writing"),
    }
    .into()
}

/// Converts an `.mrpack` (path or URL) to a `.riven` archive named `stem` and reads its release.
async fn prepare_mrpack(
    source: String,
    stem: String,
    progress: mpsc::UnboundedSender<SharedString>,
) -> Result<(Release, String), String> {
    let out = packs_dir()?.join(format!("{stem}.riven"));
    riven_build::ship::archive_mrpack(&source, &out, |stage| {
        let _ = progress.send(import_stage(stage));
    })
    .await
    .map_err(|e| e.to_string())?;
    let source = out.to_string_lossy().into_owned();
    Ok((preview(source.clone()).await?, source))
}

async fn fetch_icon_png(url: Option<String>) -> Option<Vec<u8>> {
    let response = riven_sources::client()
        .get(url?)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .ok()?;
    let bytes = response.bytes().await.ok()?;
    tokio::task::spawn_blocking(move || mods::instance_icon(&bytes))
        .await
        .ok()?
}

impl NewInstance {
    fn new(mode: Mode, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |hint: SharedString, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(hint.to_string()))
        };
        let query = input(t!("new_instance.search_packs").into(), window, cx);
        let name = input(t!("new_instance.name_hint").into(), window, cx);
        let mc_search = input(t!("new_instance.find_version").into(), window, cx);
        let link = input(t!("new_instance.link_hint").into(), window, cx);
        let subs = vec![
            cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.search(cx);
                }
            }),
            cx.subscribe(&mc_search, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.subscribe(&link, |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.prepared = None;
                    this.error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.check_link(cx),
                _ => {}
            }),
        ];
        cx.spawn(async move |this, cx| {
            let releases = runtime::spawn(async { game_meta().minecraft_releases().await })
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.releases = releases.into_iter().map(Into::into).collect();
                this.minecraft = this.releases.first().cloned();
                this.load_loader_versions(cx);
                cx.notify();
            });
        })
        .detach();
        let mut this = Self {
            mode,
            query,
            hits: Vec::new(),
            icons: HashMap::new(),
            searching: false,
            search_serial: 0,
            focus: None,
            pages: HashMap::new(),
            versions: HashMap::new(),
            version: None,
            scroll: UniformListScrollHandle::new(),
            name,
            mc_search,
            mc_scroll: UniformListScrollHandle::new(),
            releases: Vec::new(),
            minecraft: None,
            loader: 0,
            loader_versions: Vec::new(),
            loader_version: None,
            loader_recommended: None,
            loader_loading: false,
            loader_scroll: UniformListScrollHandle::new(),
            loader_serial: 0,
            icon: None,
            link,
            prepared: None,
            groups: BTreeMap::new(),
            busy: None,
            abort: None,
            error: None,
            _subs: subs,
        };
        this.search(cx);
        this.focus_input(window, cx);
        this
    }

    fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        let input = match self.mode {
            Mode::Modrinth => &self.query,
            Mode::Custom => &self.name,
            Mode::Import => &self.link,
        };
        input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Searches Modrinth modpacks shortly after typing stops; empty lists popular ones.
    fn search(&mut self, cx: &mut Context<Self>) {
        self.search_serial += 1;
        let serial = self.search_serial;
        let query = self.query.read(cx).value().trim().to_string();
        self.searching = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            if !matches!(
                this.read_with(cx, |this, _| this.search_serial == serial),
                Ok(true)
            ) {
                return;
            }
            let found =
                runtime::spawn(
                    async move { modrinth().search_modpacks(&query, None, RESULTS).await },
                )
                .await
                .unwrap_or_else(|_| Ok(vec![]));
            let hits = match found {
                Ok(hits) => hits,
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.search_serial == serial {
                            this.searching = false;
                            this.error = Some(e.to_string().into());
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            let wanted: Vec<(String, String)> = hits
                .iter()
                .filter_map(|h| Some((h.id.clone(), h.icon_url.clone()?)))
                .collect();
            let current = this.update(cx, |this, cx| {
                if this.search_serial != serial {
                    return false;
                }
                this.searching = false;
                let first = hits.first().cloned();
                this.hits = hits;
                if this.focus.is_none()
                    && let Some(hit) = first
                {
                    this.show(hit, cx);
                }
                cx.notify();
                true
            });
            if !matches!(current, Ok(true)) {
                return;
            }
            let Some(dir) = mods::cache_dir().map(|d| d.join("icons")) else {
                return;
            };
            let icons: Vec<(String, PathBuf)> = runtime::spawn(async move {
                stream::iter(wanted)
                    .map(|(id, url)| {
                        let dir = dir.clone();
                        async move { Some((id.clone(), mods::fetch_icon(&dir, &id, &url).await?)) }
                    })
                    .buffer_unordered(PARALLEL_ICONS)
                    .filter_map(|found| async move { found })
                    .collect()
                    .await
            })
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.icons.extend(icons);
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows a modpack on the right, fetching its page and versions the first time.
    fn show(&mut self, hit: Hit, cx: &mut Context<Self>) {
        let id = hit.id.clone();
        if self.focus.as_ref().map(|f| &f.id) != Some(&id) {
            self.version = None;
        }
        self.focus = Some(hit);
        cx.notify();
        if !self.pages.contains_key(&id) {
            self.pages.insert(id.clone(), Fetched::Loading);
            let project = id.clone();
            cx.spawn(async move |this, cx| {
                let key = project.clone();
                let page =
                    runtime::spawn(async move { modrinth().page(&key).await.map(Described::new) })
                        .await
                        .ok()
                        .and_then(Result::ok);
                let _ = this.update(cx, |this, cx| {
                    let page = page.map_or(Fetched::Failed, |p| Fetched::Ready(Rc::new(p)));
                    this.pages.insert(project, page);
                    cx.notify();
                });
            })
            .detach();
        }
        if !self.versions.contains_key(&id) {
            self.versions.insert(id.clone(), Fetched::Loading);
            cx.spawn(async move |this, cx| {
                let key = id.clone();
                let versions =
                    runtime::spawn(async move { modrinth().project_versions(&key).await })
                        .await
                        .ok()
                        .and_then(Result::ok);
                let _ = this.update(cx, |this, cx| {
                    let versions = versions.map_or(Fetched::Failed, |v| Fetched::Ready(Rc::new(v)));
                    this.versions.insert(id, versions);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// The focused modpack's picked version, the newest when none was picked.
    fn picked_version(&self) -> Option<Version> {
        let hit = self.focus.as_ref()?;
        let Some(Fetched::Ready(versions)) = self.versions.get(&hit.id) else {
            return None;
        };
        match &self.version {
            Some(id) => versions.iter().find(|v| v.id == id.as_ref()).cloned(),
            None => versions.first().cloned(),
        }
    }

    /// Looks up the builds of the chosen loader for the chosen Minecraft.
    fn load_loader_versions(&mut self, cx: &mut Context<Self>) {
        self.loader_serial += 1;
        self.loader_versions.clear();
        self.loader_version = None;
        self.loader_recommended = None;
        let (Some(kind), Some(minecraft)) = (LOADERS[self.loader], self.minecraft.clone()) else {
            self.loader_loading = false;
            cx.notify();
            return;
        };
        self.loader_loading = true;
        let serial = self.loader_serial;
        cx.spawn(async move |this, cx| {
            let found = runtime::spawn(async move {
                let meta = game_meta();
                let all = meta.loader_versions(kind, &minecraft).await?;
                let recommended = meta.loader_version(kind, &minecraft).await.ok();
                Ok::<_, riven_sources::Error>((all, recommended))
            })
            .await
            .ok()
            .and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                if this.loader_serial != serial {
                    return;
                }
                this.loader_loading = false;
                if let Some((all, recommended)) = found {
                    this.loader_recommended = recommended.clone().map(Into::into);
                    this.loader_version =
                        recommended.or_else(|| all.first().cloned()).map(Into::into);
                    this.loader_versions = all.into_iter().map(Into::into).collect();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs `work` with a spinner, turning its error into the dialog's error line.
    fn run<T: Send + 'static>(
        &mut self,
        label: SharedString,
        work: impl std::future::Future<Output = Result<T, String>> + Send + 'static,
        progress: Option<mpsc::UnboundedReceiver<SharedString>>,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label);
        self.error = None;
        cx.notify();
        if let Some(mut progress) = progress {
            cx.spawn(async move |this, cx| {
                while let Some(text) = progress.recv().await {
                    let _ = this.update(cx, |this, cx| {
                        if this.busy.is_some() {
                            this.busy = Some(text);
                            cx.notify();
                        }
                    });
                }
            })
            .detach();
        }
        let (result, abort) = runtime::spawn_abortable(work);
        self.abort = Some(abort);
        cx.spawn(async move |this, cx| {
            let result = result.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                this.abort = None;
                match result {
                    Ok(Ok(value)) => done(this, value, cx),
                    Ok(Err(e)) => this.error = Some(e.into()),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install_modrinth(&mut self, cx: &mut Context<Self>) {
        let (Some(hit), Some(version)) = (self.focus.clone(), self.picked_version()) else {
            return;
        };
        let Some(url) = version.primary_file().and_then(|f| f.url.clone()) else {
            self.error = Some(t!("new_instance.no_file").into());
            cx.notify();
            return;
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let stem = format!("{}-{}", hit.id, version.id);
        let icon_url = hit.icon_url.clone();
        let title = hit.title.clone();
        let modrinth = riven_format::ModrinthPack {
            project: hit.id.clone(),
            version: version.id.clone(),
        };
        self.run(
            t!("new_instance.preparing").into(),
            async move {
                let (release, source) = prepare_mrpack(url, stem, tx).await?;
                let icon = fetch_icon_png(icon_url).await;
                Ok(Prepared {
                    release,
                    source,
                    icon,
                    title: None,
                    modrinth: Some(modrinth),
                    github: None,
                })
            },
            Some(rx),
            move |this, prepared, cx| this.finish(prepared, Some(title), cx),
            cx,
        );
    }

    fn check_link(&mut self, cx: &mut Context<Self>) {
        let link = self.link.read(cx).value().trim().to_string();
        if link.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        self.run(
            t!("new_instance.checking").into(),
            async move {
                let linked = riven_launch::modpack::resolve(&link, |stage| {
                    let _ = tx.send(import_stage(stage));
                })
                .await
                .map_err(|e| e.to_string())?;
                let release = preview(linked.source.clone()).await?;
                Ok(Prepared {
                    release,
                    source: linked.source,
                    icon: fetch_icon_png(linked.icon_url).await,
                    title: linked.title,
                    modrinth: linked.modrinth,
                    github: linked.github,
                })
            },
            Some(rx),
            |this, prepared, _| this.show_prepared(prepared),
            cx,
        );
    }

    fn show_prepared(&mut self, prepared: Prepared) {
        self.groups = prepared
            .release
            .groups
            .iter()
            .map(|g| (g.id.clone(), g.default))
            .collect();
        self.prepared = Some(prepared);
    }

    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let paths = super::pick::files(t!("pick.packs"), super::pick::PACKS, false).await;
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.open_file(path, cx));
        })
        .detach();
    }

    /// Reads a dropped or picked pack: `.riven`/`.mrpack` show their release, a Prism export becomes an instance.
    pub fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.mode = Mode::Import;
        self.prepared = None;
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let shown = path.display().to_string();
        match ext.as_str() {
            "riven" => self.run(
                t!("new_instance.checking").into(),
                async move {
                    let release = preview(shown).await?;
                    Ok(Prepared {
                        release,
                        source: keep_archive(&path)?,
                        icon: None,
                        title: None,
                        modrinth: None,
                        github: None,
                    })
                },
                None,
                |this, prepared, _| this.show_prepared(prepared),
                cx,
            ),
            "mrpack" => {
                let (tx, rx) = mpsc::unbounded_channel();
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "pack".into());
                self.run(
                    t!("new_instance.preparing").into(),
                    async move {
                        let (release, source) = prepare_mrpack(shown, stem, tx).await?;
                        Ok(Prepared {
                            release,
                            source,
                            icon: None,
                            title: None,
                            modrinth: None,
                            github: None,
                        })
                    },
                    Some(rx),
                    |this, prepared, _| this.show_prepared(prepared),
                    cx,
                );
            }
            "zip" => {
                let Some(store) = AppState::global(cx).read(cx).store.clone() else {
                    return;
                };
                self.run(
                    t!("new_instance.importing").into(),
                    async move {
                        tokio::task::spawn_blocking(move || {
                            riven_launch::prism::import(&store, &path).map_err(|e| e.to_string())
                        })
                        .await
                        .map_err(|e| e.to_string())?
                    },
                    None,
                    |_, id, cx| {
                        AppState::global(cx).update(cx, |s, cx| {
                            s.reload_instances(cx);
                            s.navigate(Route::Instance(id), cx);
                            s.close_modal(cx);
                        })
                    },
                    cx,
                );
            }
            _ => {
                self.error = Some(t!("new_instance.bad_file").into());
                cx.notify();
            }
        }
    }

    /// Creates an instance for a checked pack and starts installing it into the launch bar.
    fn finish(&mut self, prepared: Prepared, title: Option<String>, cx: &mut Context<Self>) {
        let state = AppState::global(cx);
        let Some(store) = state.read(cx).store.clone() else {
            return;
        };
        let Prepared {
            release,
            source,
            icon,
            title: linked_title,
            modrinth,
            github,
        } = prepared;
        let title = title.or(linked_title);
        let typed = self.name.read(cx).value().trim().to_string();
        let name = match () {
            _ if !typed.is_empty() && self.mode != Mode::Modrinth => typed,
            _ => title.unwrap_or_else(|| release.name.clone()),
        };
        let id = match store.create(&name, &release.minecraft, Some(release.loader.clone())) {
            Ok(id) => id,
            Err(e) => {
                self.error = Some(e.to_string().into());
                cx.notify();
                return;
            }
        };
        if let Some(png) = icon
            && let Err(e) = store.set_icon(&id, &png)
        {
            tracing::warn!("{e}");
        }
        if (modrinth.is_some() || github.is_some())
            && let Err(e) = store.load(&id).and_then(|mut instance| {
                instance.modrinth = modrinth;
                instance.github = github;
                store.save(&id, &instance)
            })
        {
            tracing::warn!("{e}");
        }
        let request = Request {
            source: Some(source),
            groups: self
                .groups
                .iter()
                .map(|(g, on)| format!("{}{g}", if *on { "+" } else { "-" }))
                .collect(),
            ..Request::default()
        };
        state.update(cx, |s, cx| {
            s.reload_instances(cx);
            s.navigate(Route::Instance(id.clone()), cx);
            s.close_modal(cx);
            s.install_pack(&id, request, cx);
        });
    }

    /// The name typed, else one made from the loader and the game version.
    fn custom_name(&self, minecraft: &str, cx: &App) -> String {
        let typed = self.name.read(cx).value().trim().to_string();
        match (typed.is_empty(), LOADERS[self.loader]) {
            (false, _) => typed,
            (true, Some(kind)) => format!("{} {minecraft}", loader_display(kind)),
            (true, None) => format!("Minecraft {minecraft}"),
        }
    }

    fn pick_icon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let paths = super::pick::files(t!("pick.images"), super::pick::IMAGES, false).await;
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let made = runtime::blocking(move || {
                let png = std::fs::read(&path)
                    .ok()
                    .and_then(|b| mods::instance_icon(&b))?;
                let dir = mods::cache_dir()?.join("icons");
                std::fs::create_dir_all(&dir).ok()?;
                let preview = dir.join(format!("new-{}.png", png.len()));
                std::fs::write(&preview, &png).ok()?;
                Some((preview, png))
            })
            .await
            .ok()
            .flatten();
            let _ = this.update(cx, |this, cx| {
                match made {
                    Some(icon) => this.icon = Some(icon),
                    None => this.error = Some(t!("new_instance.bad_icon").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Creates an empty instance of the chosen version and loader build.
    fn create(&mut self, cx: &mut Context<Self>) {
        let Some(minecraft) = self.minecraft.clone() else {
            self.error = Some(t!("new_instance.pick_version").into());
            cx.notify();
            return;
        };
        let Some(store) = AppState::global(cx).read(cx).store.clone() else {
            return;
        };
        let kind = LOADERS[self.loader];
        let name = self.custom_name(&minecraft, cx);
        let picked = self.loader_version.as_ref().map(ToString::to_string);
        let icon = self.icon.as_ref().map(|(_, png)| png.clone());
        let minecraft = minecraft.to_string();
        self.run(
            t!("new_instance.creating").into(),
            async move {
                let loader = match kind {
                    Some(kind) => Some(Loader {
                        kind,
                        version: match picked {
                            Some(v) => v,
                            None => game_meta()
                                .loader_version(kind, &minecraft)
                                .await
                                .map_err(|e| e.to_string())?,
                        },
                    }),
                    None => None,
                };
                let id = store
                    .create(&name, &minecraft, loader)
                    .map_err(|e| e.to_string())?;
                if let Some(png) = icon {
                    store.set_icon(&id, &png).map_err(|e| e.to_string())?;
                }
                Ok(id)
            },
            None,
            |_, id, cx| {
                AppState::global(cx).update(cx, |s, cx| {
                    s.reload_instances(cx);
                    s.navigate(Route::Instance(id), cx);
                    s.close_modal(cx);
                })
            },
            cx,
        );
    }

    fn render_hit(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors;
        let hit = self.hits.get(ix)?;
        let focused = self.focus.as_ref().is_some_and(|f| f.id == hit.id);
        let shown = hit.clone();
        let target = target_label(&hit.game_versions, &hit.loaders);
        Some(
            h_flex()
                .id(("pack-hit", ix))
                .w_full()
                .h(px(HIT_HEIGHT))
                .gap(px(12.))
                .px(px(10.))
                .rounded(px(8.))
                .cursor_pointer()
                .map(|r| {
                    if focused {
                        r.bg(c.sel)
                    } else {
                        r.hover(|s| s.bg(c.row))
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| this.show(shown.clone(), cx)))
                .child(logo(self.icons.get(&hit.id), 36., cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .min_w_0()
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .truncate()
                                        .child(hit.title.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(12.))
                                        .text_color(c.muted)
                                        .child(format!(
                                            "{} · ↓ {}",
                                            hit.author,
                                            compact(hit.downloads)
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(c.text2)
                                .truncate()
                                .child(hit.description.clone()),
                        ),
                )
                .when(!target.is_empty(), |r| {
                    r.child(
                        div()
                            .flex_none()
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(10.))
                            .bg(c.row)
                            .text_size(px(11.))
                            .text_color(c.text2)
                            .child(target),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_modrinth(&self, height: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let list: AnyElement = if self.hits.is_empty() {
            h_flex()
                .size_full()
                .justify_center()
                .gap(px(8.))
                .text_color(c.muted)
                .when(self.searching, |h| {
                    h.child(motion::spinner(
                        "packs-spinner",
                        icon(IconName::Loader, c.muted).size(px(14.)),
                        cx,
                    ))
                    .child(t!("add_mods.searching").to_string())
                })
                .when(!self.searching, |h| {
                    h.child(t!("add_mods.nothing").to_string())
                })
                .into_any_element()
        } else {
            div()
                .relative()
                .size_full()
                .child(
                    uniform_list(
                        "pack-hits",
                        self.hits.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .filter_map(|ix| this.render_hit(ix, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full()
                    .pr(px(10.))
                    .track_scroll(&self.scroll),
                )
                .child(scrollbar(&self.scroll))
                .into_any_element()
        };
        h_flex()
            .items_start()
            .gap(px(16.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h(px(height))
                    .gap(px(10.))
                    .child(
                        TextField::new(&self.query)
                            .leading(IconName::Search)
                            .clearable(),
                    )
                    .child(div().flex_1().min_h_0().child(list)),
            )
            .child(side_panel(height, cx).child(self.render_pack_details(cx)))
            .into_any_element()
    }

    fn render_pack_details(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let Some(hit) = &self.focus else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.muted)
                .child(t!("new_instance.pick_pack").to_string())
                .into_any_element();
        };
        let page = match self.pages.get(&hit.id) {
            Some(Fetched::Ready(page)) => Some(page.clone()),
            _ => None,
        };
        let versions = self.versions.get(&hit.id).cloned();
        let picked = self.picked_version();
        let stats = match &page {
            Some(page) => {
                let updated = riven_launch::parse_rfc3339(&page.updated).map(|secs| {
                    super::time::ago(std::time::UNIX_EPOCH + Duration::from_secs(secs))
                });
                let mut parts = vec![
                    format!("↓ {}", compact(page.downloads)),
                    format!("♥ {}", compact(page.followers)),
                ];
                parts.extend(updated.map(|u| t!("add_mods.updated", when = u).to_string()));
                parts.join(" · ")
            }
            None => format!("↓ {}", compact(hit.downloads)),
        };
        let version_picker: AnyElement = match versions {
            Some(Fetched::Ready(list)) if !list.is_empty() => {
                let items = list
                    .iter()
                    .map(|v| {
                        let label = format!(
                            "{} · {}",
                            v.number,
                            target_label(&v.game_versions, &v.loaders)
                        );
                        MenuItem::new(v.id.clone(), label)
                    })
                    .collect();
                let view = cx.entity().downgrade();
                Dropdown::new(
                    "pack-version",
                    items,
                    picked.as_ref().map(|v| v.id.clone().into()),
                    move |id, _, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.version = Some(id);
                            cx.notify();
                        });
                    },
                )
                .width(px(SIDE_WIDTH - 16.))
                .into_any_element()
            }
            Some(Fetched::Failed) | Some(Fetched::Ready(_)) => div()
                .text_color(c.warn)
                .child(t!("new_instance.no_versions").to_string())
                .into_any_element(),
            _ => h_flex()
                .gap(px(8.))
                .text_color(c.muted)
                .child(motion::spinner(
                    "versions-spinner",
                    icon(IconName::Loader, c.muted).size(px(14.)),
                    cx,
                ))
                .child(t!("new_instance.loading_versions").to_string())
                .into_any_element(),
        };
        let slug = hit.slug.clone();
        v_flex()
            .gap(px(12.))
            .child(
                h_flex()
                    .gap(px(12.))
                    .child(logo(self.icons.get(&hit.id), 56., cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .text_size(px(16.))
                                    .line_height(relative(1.3))
                                    .child(hit.title.clone()),
                            )
                            .child(
                                div()
                                    .text_color(c.muted)
                                    .child(t!("add_mods.by", author = hit.author).to_string()),
                            )
                            .child(div().text_size(px(12.)).text_color(c.muted).child(stats)),
                    ),
            )
            .child(
                div()
                    .text_color(c.text2)
                    .line_height(relative(1.5))
                    .child(hit.description.clone()),
            )
            .child(field(t!("new_instance.version"), version_picker, cx))
            .child(
                h_flex().gap(px(8.)).child(
                    Button::new("pack-open")
                        .icon(IconName::ExternalLink)
                        .label(t!("add_mods.open_page"))
                        .on_click(move |_, _, cx| {
                            cx.open_url(&format!("https://modrinth.com/modpack/{slug}"))
                        }),
                ),
            )
            .when_some(page, |col, page| {
                col.child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .children(page.categories.iter().map(|cat| {
                            div()
                                .px(px(8.))
                                .py(px(2.))
                                .rounded(px(10.))
                                .bg(c.row)
                                .text_size(px(11.))
                                .text_color(c.text2)
                                .child(cat.clone())
                        })),
                )
                .child(div().h(px(1.)).bg(c.border))
                .children(markdown::view(&page.about, cx))
            })
            .into_any_element()
    }

    /// A list of versions to pick one from; `marked` gets a small tag beside it.
    #[allow(clippy::too_many_arguments)]
    fn version_list(
        &self,
        id: &'static str,
        items: Vec<SharedString>,
        picked: Option<SharedString>,
        marked: Option<(SharedString, SharedString)>,
        empty: SharedString,
        scroll: &UniformListScrollHandle,
        pick: impl Fn(&mut Self, SharedString, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let frame = div()
            .relative()
            .flex_1()
            .min_h_0()
            .rounded(px(8.))
            .border_1()
            .border_color(c.border)
            .bg(c.bg);
        if items.is_empty() {
            return frame
                .flex()
                .items_center()
                .justify_center()
                .p(px(12.))
                .text_center()
                .text_color(c.muted)
                .child(empty)
                .into_any_element();
        }
        let pick = Rc::new(pick);
        frame
            .child(
                uniform_list(
                    id,
                    items.len(),
                    cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                        let c = cx.theme().colors;
                        range
                            .map(|ix| {
                                let version = items[ix].clone();
                                let on = picked.as_ref() == Some(&version);
                                let tag = marked
                                    .as_ref()
                                    .filter(|(v, _)| *v == version)
                                    .map(|(_, tag)| tag.clone());
                                let pick = pick.clone();
                                let chosen = version.clone();
                                h_flex()
                                    .id((id, ix))
                                    .w_full()
                                    .h(px(30.))
                                    .px(px(10.))
                                    .gap(px(8.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .map(|r| {
                                        if on {
                                            r.bg(c.sel).font_weight(FontWeight::SEMIBOLD)
                                        } else {
                                            r.text_color(c.text2).hover(|s| s.bg(c.row))
                                        }
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        pick(this, chosen.clone(), cx);
                                        cx.notify();
                                    }))
                                    .child(div().flex_1().min_w_0().truncate().child(version))
                                    .when_some(tag, |r, tag| {
                                        r.child(
                                            div()
                                                .flex_none()
                                                .px(px(6.))
                                                .rounded(px(4.))
                                                .bg(c.row)
                                                .text_size(px(11.))
                                                .text_color(c.muted)
                                                .child(tag),
                                        )
                                    })
                                    .when(on, |r| r.child(icon(IconName::Check, c.accent)))
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .p(px(4.))
                .track_scroll(scroll),
            )
            .child(
                // Both lists come from this one call site, so each bar needs an id of its own.
                div().absolute().inset_0().child(
                    Scrollbar::vertical(scroll)
                        .id(SharedString::from(format!("{id}-bar")))
                        .viewport_from_layout(),
                ),
            )
            .into_any_element()
    }

    /// The loaders as cards with their logos, one picked.
    fn render_loaders(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let cards = LOADERS.iter().enumerate().map(|(i, kind)| {
            let on = i == self.loader;
            h_flex()
                .id(("loader-card", i))
                .flex_1()
                .min_w_0()
                .h(px(44.))
                .px(px(12.))
                .gap(px(8.))
                .rounded(px(8.))
                .border_1()
                .cursor_pointer()
                .map(|d| {
                    if on {
                        d.border_color(c.accent)
                            .bg(c.accent.opacity(0.1))
                            .font_weight(FontWeight::SEMIBOLD)
                    } else {
                        d.border_color(c.border)
                            .text_color(c.text2)
                            .hover(|s| s.bg(c.row))
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.loader != i {
                        this.loader = i;
                        this.load_loader_versions(cx);
                    }
                }))
                .child(
                    icon(
                        super::assets::loader_icon(*kind),
                        if on { c.accent } else { c.muted },
                    )
                    .size(px(18.)),
                )
                .child(div().truncate().child(loader_label(*kind)))
        });
        h_flex().gap(px(8.)).children(cards).into_any_element()
    }

    fn render_custom(&self, height: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let kind = LOADERS[self.loader];
        let query = self.mc_search.read(cx).value().trim().to_lowercase();
        let releases: Vec<SharedString> = self
            .releases
            .iter()
            .filter(|r| r.to_lowercase().contains(&query))
            .cloned()
            .collect();
        let releases_empty = if self.releases.is_empty() {
            t!("new_instance.loading")
        } else {
            t!("add_mods.nothing")
        };
        let label = |text: SharedString| {
            div()
                .flex_1()
                .text_size(px(12.))
                .text_color(c.muted)
                .child(text)
        };
        let minecraft = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(6.))
            .child(
                h_flex()
                    .h(px(32.))
                    .gap(px(10.))
                    .child(label(t!("new_instance.minecraft").into()))
                    .child(
                        TextField::new(&self.mc_search)
                            .leading(IconName::Search)
                            .clearable()
                            .w(px(200.)),
                    ),
            )
            .child(
                self.version_list(
                    "mc-release",
                    releases,
                    self.minecraft.clone(),
                    self.releases
                        .first()
                        .map(|v| (v.clone(), t!("new_instance.latest").into())),
                    releases_empty.into(),
                    &self.mc_scroll,
                    |this, v, cx| {
                        this.minecraft = Some(v);
                        this.load_loader_versions(cx);
                    },
                    cx,
                ),
            );
        let builds = kind.map(|kind| {
            let empty = if self.loader_loading {
                t!("new_instance.loading")
            } else {
                t!("new_instance.no_builds", loader = loader_display(kind))
            };
            v_flex()
                .w(px(310.))
                .flex_none()
                .gap(px(6.))
                .child(
                    h_flex()
                        .h(px(32.))
                        .child(label(t!("new_instance.loader_version").into())),
                )
                .child(
                    self.version_list(
                        "loader-build",
                        self.loader_versions.clone(),
                        self.loader_version.clone(),
                        self.loader_recommended
                            .clone()
                            .map(|v| (v, t!("new_instance.recommended").into())),
                        empty.into(),
                        &self.loader_scroll,
                        |this, v, _| this.loader_version = Some(v),
                        cx,
                    ),
                )
        });
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .h(px(height))
            .gap(px(14.))
            .child(field(
                t!("new_instance.loader"),
                self.render_loaders(cx),
                cx,
            ))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .gap(px(12.))
                    .child(minecraft)
                    .children(builds),
            );
        let game = self.minecraft.clone().unwrap_or_default();
        let loader = match (kind, &self.loader_version) {
            (Some(kind), Some(v)) => format!("{} {v}", loader_display(kind)),
            _ => loader_label(kind).to_string(),
        };
        let parts = [format!("Minecraft {game}"), loader];
        let tile = match &self.icon {
            Some((path, _)) => img(path.clone())
                .size(px(56.))
                .flex_none()
                .rounded(px(12.))
                .into_any_element(),
            None => div()
                .size(px(56.))
                .flex_none()
                .rounded(px(12.))
                .bg(c.sel)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(super::assets::loader_icon(kind), c.muted).size(px(24.)))
                .into_any_element(),
        };
        let summary = v_flex()
            .gap(px(14.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(c.border)
            .bg(c.bg)
            .child(
                h_flex().gap(px(12.)).child(tile).child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_size(px(15.))
                                .truncate()
                                .child(self.custom_name(&game, cx)),
                        )
                        .child(
                            div()
                                .mt(px(2.))
                                .text_size(px(12.))
                                .text_color(c.muted)
                                .truncate()
                                .child(parts.join(" · ")),
                        ),
                ),
            )
            .child(field(
                t!("new_instance.name"),
                TextField::new(&self.name),
                cx,
            ))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        Button::new("pick-icon")
                            .icon(IconName::Image)
                            .label(t!("new_instance.pick_icon"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_icon(window, cx)),
                            ),
                    )
                    .when(self.icon.is_some(), |row| {
                        row.child(
                            Button::new("clear-icon")
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip(t!("new_instance.clear_icon"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.icon = None;
                                    cx.notify();
                                })),
                        )
                    }),
            );
        let right = side_panel(height, cx)
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .child(t!("new_instance.result").to_string()),
            )
            .child(summary)
            .child(
                div()
                    .text_color(c.muted)
                    .line_height(relative(1.5))
                    .child(t!("new_instance.description").to_string()),
            );
        h_flex()
            .items_start()
            .gap(px(16.))
            .h(px(height))
            .child(left)
            .child(right)
            .into_any_element()
    }

    fn render_import(&self, height: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let busy = self.busy.is_some();
        let link = v_flex()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(c.border)
            .bg(c.bg)
            .child(
                h_flex()
                    .gap(px(8.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon(IconName::ExternalLink, c.text2))
                    .child(t!("new_instance.link").to_string()),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(TextField::new(&self.link).flex_1())
                    .child(
                        Button::new("check-link")
                            .label(t!("new_instance.check"))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.check_link(cx))),
                    ),
            )
            .child(chips(
                ["Modrinth", "GitHub", ".mrpack", "packwiz", "Riven"],
                cx,
            ));
        let or = h_flex()
            .gap(px(10.))
            .text_size(px(12.))
            .text_color(c.muted)
            .child(div().flex_1().h(px(1.)).bg(c.border))
            .child(t!("new_instance.or").to_string())
            .child(div().flex_1().h(px(1.)).bg(c.border));
        let drop = v_flex()
            .id("pack-drop")
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_dashed()
            .border_color(c.border)
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(c.accent).bg(c.accent.opacity(0.06))
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                cx.stop_propagation();
                if let Some(path) = paths.paths().first() {
                    this.open_file(path.clone(), cx);
                }
            }))
            .child(
                div()
                    .size(px(48.))
                    .rounded(px(12.))
                    .bg(c.sel)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(IconName::Upload, c.muted).size(px(22.))),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("new_instance.drop_here").to_string()),
            )
            .child(
                Button::new("pick-pack-file")
                    .icon(IconName::Folder)
                    .label(t!("new_instance.pick_file"))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.pick_file(window, cx))),
            )
            .child(chips([".riven", ".mrpack", "Prism Launcher", "MultiMC"], cx).justify_center());
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .h(px(height))
            .gap(px(12.))
            .child(link)
            .child(or)
            .child(drop);
        let right = side_panel(height, cx)
            .gap(px(14.))
            .map(|col| match &self.prepared {
                Some(prepared) => {
                    col.child(self.render_release(&prepared.release, cx))
                        .child(field(
                            t!("new_instance.name"),
                            TextField::new(&self.name),
                            cx,
                        ))
                }
                None => col.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_center()
                        .text_color(c.muted)
                        .child(t!("new_instance.import_empty").to_string()),
                ),
            });
        h_flex()
            .items_start()
            .gap(px(16.))
            .h(px(height))
            .child(left)
            .child(right)
            .into_any_element()
    }

    /// A checked release: name, version, game and its optional parts as switches.
    fn render_release(&self, release: &Release, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let view = cx.entity().downgrade();
        let loader = format!(
            "Minecraft {} · {} {}",
            release.minecraft,
            loader_display(release.loader.kind),
            release.loader.version
        );
        let groups = release.groups.iter().enumerate().map(|(i, g)| {
            let on = self.groups.get(&g.id).copied().unwrap_or(g.default);
            let id = g.id.clone();
            let view = view.clone();
            h_flex()
                .gap(px(12.))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().child(g.name.clone()))
                        .when_some(g.description.clone(), |col, d| {
                            col.child(div().text_size(px(12.)).text_color(c.muted).child(d))
                        }),
                )
                .child(
                    Switch::new(SharedString::from(format!("group-{i}")), on)
                        .accessible(g.name.clone())
                        .on_change(move |on, _, cx| {
                            let id = id.clone();
                            let _ = view.update(cx, |this, cx| {
                                this.groups.insert(id, on);
                                cx.notify();
                            });
                        }),
                )
        });
        v_flex()
            .gap(px(12.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(c.border)
            .bg(c.bg)
            .child(
                h_flex()
                    .gap(px(12.))
                    .child(super::launch_bar::instance_tile(
                        &release.name,
                        None,
                        40.,
                        8.,
                        cx,
                    ))
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::BOLD)
                                            .truncate()
                                            .child(release.name.clone()),
                                    )
                                    .child(
                                        div().text_color(c.muted).child(release.version.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(12.))
                                    .text_color(c.muted)
                                    .child(format!(
                                        "{loader} · {}",
                                        t!("new_instance.files", n = release.files.len())
                                    )),
                            ),
                    ),
            )
            .when(!release.groups.is_empty(), |card| {
                card.child(
                    v_flex()
                        .gap(px(10.))
                        .pt(px(12.))
                        .border_t_1()
                        .border_color(c.border)
                        .children(groups),
                )
            })
            .into_any_element()
    }
}

/// Small tags naming what a field or drop area takes.
fn chips<const N: usize>(names: [&'static str; N], cx: &App) -> gpui_kit::Div {
    let c = cx.theme().colors;
    h_flex()
        .flex_wrap()
        .gap(px(6.))
        .children(names.into_iter().map(|name| {
            div()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(10.))
                .bg(c.row)
                .text_size(px(11.))
                .text_color(c.text2)
                .child(name)
        }))
}

/// The right column of every mode, with a rule on its left.
fn side_panel(height: f32, cx: &App) -> gpui_kit::Stateful<gpui_kit::Div> {
    v_flex()
        .id("new-instance-side")
        .w(px(SIDE_WIDTH))
        .flex_none()
        .h(px(height))
        .overflow_y_scroll()
        .pl(px(16.))
        .border_l_1()
        .border_color(cx.theme().colors.border)
}

fn logo(file: Option<&PathBuf>, size: f32, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    match file {
        Some(path) => img(path.clone())
            .size(px(size))
            .flex_none()
            .rounded(px(size / 5.))
            .into_any_element(),
        None => div()
            .size(px(size))
            .flex_none()
            .rounded(px(size / 5.))
            .bg(c.sel)
            .flex()
            .items_center()
            .justify_center()
            .child(icon(IconName::Package, c.muted))
            .into_any_element(),
    }
}

impl Render for NewInstance {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let height = (f32::from(window.viewport_size().height) - 300.).clamp(240., 560.);
        let view = cx.entity().downgrade();
        let selected = MODES.iter().position(|m| *m == self.mode).unwrap_or(0);
        let tabs = Tabs::new(
            "new-instance-mode",
            vec![
                "Modrinth".into(),
                t!("new_instance.empty").into(),
                t!("new_instance.from_pack").into(),
            ],
            selected,
            move |ix, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.mode = MODES[ix];
                    this.error = None;
                    this.focus_input(window, cx);
                    cx.notify();
                });
            },
        );
        let content = match self.mode {
            Mode::Modrinth => self.render_modrinth(height, cx),
            Mode::Custom => self.render_custom(height, cx),
            Mode::Import => self.render_import(height, cx),
        };
        let status: AnyElement = match (&self.busy, &self.error) {
            (Some(text), _) => h_flex()
                .gap(px(8.))
                .text_color(c.muted)
                .child(motion::spinner(
                    "new-instance-busy",
                    icon(IconName::Loader, c.muted).size(px(14.)),
                    cx,
                ))
                .child(text.clone())
                .into_any_element(),
            (None, Some(error)) => div()
                .text_color(c.warn)
                .line_height(relative(1.4))
                .child(error.clone())
                .into_any_element(),
            _ => div().into_any_element(),
        };
        let body = v_flex()
            .gap(px(14.))
            .child(h_flex().child(tabs))
            .child(content)
            .child(div().min_h(px(22.)).child(status));
        let busy = self.busy.is_some();
        let cancel = match self.abort.clone() {
            Some(abort) => Button::new("stop")
                .outline()
                .size(ButtonSize::Md)
                .label(t!("common.stop"))
                .on_click(move |_, _, _| abort.abort()),
            None => Button::new("cancel")
                .outline()
                .size(ButtonSize::Md)
                .label(t!("common.cancel"))
                .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx))),
        };
        let action = match self.mode {
            Mode::Modrinth => Button::new("install-modpack")
                .primary()
                .size(ButtonSize::Md)
                .icon(IconName::Plus)
                .label(t!("new_instance.install"))
                .disabled(busy || self.picked_version().is_none())
                .on_click(cx.listener(|this, _, _, cx| this.install_modrinth(cx))),
            Mode::Custom => Button::new("create")
                .primary()
                .size(ButtonSize::Md)
                .label(t!("new_instance.create"))
                .disabled(busy || self.minecraft.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
            Mode::Import => {
                let ready = self.prepared.is_some();
                Button::new("install-pack")
                    .primary()
                    .size(ButtonSize::Md)
                    .label(if ready {
                        t!("new_instance.install")
                    } else {
                        t!("new_instance.check")
                    })
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| match this.prepared.take() {
                        Some(prepared) => this.finish(prepared, None, cx),
                        None => this.check_link(cx),
                    }))
            }
        };
        dialog_shell(
            t!("new_instance.title"),
            t!("new_instance.subtitle"),
            body,
            vec![cancel, action],
            cx,
        )
    }
}

pub fn open(window: &mut Window, cx: &mut App) {
    open_mode(Mode::Modrinth, window, cx);
}

pub fn open_mode(mode: Mode, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| NewInstance::new(mode, window, cx));
    super::dialogs::open(form.into(), WIDTH, cx);
}

/// Opens the dialog on a pack link, as from a `riven://install` link.
pub fn open_link(link: &str, window: &mut Window, cx: &mut App) {
    let link = link.to_owned();
    let form = cx.new(|cx| {
        let mut form = NewInstance::new(Mode::Import, window, cx);
        form.link
            .update(cx, |s, cx| s.set_value(link.clone(), window, cx));
        form.check_link(cx);
        form
    });
    super::dialogs::open(form.into(), WIDTH, cx);
}

/// Opens the dialog on a pack file, as when one is dropped on the window.
pub fn open_file(path: &Path, window: &mut Window, cx: &mut App) {
    let path = path.to_owned();
    let form = cx.new(|cx| {
        let mut form = NewInstance::new(Mode::Import, window, cx);
        form.open_file(path, cx);
        form
    });
    super::dialogs::open(form.into(), WIDTH, cx);
}
