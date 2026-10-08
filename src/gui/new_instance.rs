use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use super::markdown::{self, Described};
use futures_util::{StreamExt as _, stream};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString,
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
use super::theme::ActiveTheme as _;
use super::ui::{
    Button, ButtonSize, Dropdown, IconName, MenuItem, Switch, Tabs, TextField, h_flex, icon,
    motion, scrollbar, v_flex,
};

const WIDTH: f32 = 1080.;
const SIDE_WIDTH: f32 = 380.;
const RESULTS: u32 = 40;
const SEARCH_DELAY: Duration = Duration::from_millis(250);
const PARALLEL_ICONS: usize = 8;
const HIT_HEIGHT: f32 = 58.;
const FIELD_WIDTH: f32 = 340.;

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
    releases: Vec<SharedString>,
    minecraft: Option<SharedString>,
    loader: usize,
    loader_versions: Vec<SharedString>,
    loader_version: Option<SharedString>,
    loader_serial: u64,
    /// A picked icon: a preview file and the PNG to store.
    icon: Option<(PathBuf, Vec<u8>)>,

    link: Entity<InputState>,
    prepared: Option<Prepared>,
    groups: BTreeMap<String, bool>,

    /// What is running, shown with a spinner; actions wait for it.
    busy: Option<SharedString>,
    error: Option<SharedString>,
    _subs: Vec<Subscription>,
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
    riven_sync::data_dir()
        .map(|d| d.join("packs"))
        .ok_or_else(|| t!("new_instance.no_data_dir").to_string())
}

/// Reads the release a link, a `.riven` file or a converted `.mrpack` points at.
async fn preview(source: String) -> Result<Release, String> {
    let store = riven_sync::Store::default_location()
        .ok_or_else(|| t!("new_instance.no_data_dir").to_string())?;
    update::preview(&store, &riven_sources::client(), &source, None)
        .await
        .map_err(|e| e.to_string())
}

/// Converts an `.mrpack` (path or URL) to a `.riven` archive named `stem` and reads its release.
async fn prepare_mrpack(
    source: String,
    stem: String,
    progress: mpsc::UnboundedSender<SharedString>,
) -> Result<(Release, String), String> {
    let out = packs_dir()?.join(format!("{stem}.riven"));
    riven_build::ship::archive_mrpack(&source, &out, |stage| {
        use riven_build::import::ImportStage;
        let text = match stage {
            ImportStage::Reading => t!("new_instance.stage_reading"),
            ImportStage::Identifying { files } => t!("new_instance.stage_identifying", n = files),
            ImportStage::Downloading => t!("new_instance.stage_downloading"),
            ImportStage::Writing => t!("new_instance.stage_writing"),
        };
        let _ = progress.send(text.into());
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
            releases: Vec::new(),
            minecraft: None,
            loader: 0,
            loader_versions: Vec::new(),
            loader_version: None,
            loader_serial: 0,
            icon: None,
            link,
            prepared: None,
            groups: BTreeMap::new(),
            busy: None,
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
        let (Some(kind), Some(minecraft)) = (LOADERS[self.loader], self.minecraft.clone()) else {
            cx.notify();
            return;
        };
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
                if let Some((all, recommended)) = found {
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
        let result = runtime::spawn(work);
        cx.spawn(async move |this, cx| {
            let result = result.await.unwrap_or_else(|_| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(value) => done(this, value, cx),
                    Err(e) => this.error = Some(e.into()),
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
        let stem = format!("{}-{}", hit.slug, version.id);
        let icon_url = hit.icon_url.clone();
        let title = hit.title.clone();
        self.run(
            t!("new_instance.preparing").into(),
            async move {
                let (release, source) = prepare_mrpack(url, stem, tx).await?;
                let icon = fetch_icon_png(icon_url).await;
                Ok(Prepared {
                    release,
                    source,
                    icon,
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
        self.run(
            t!("new_instance.checking").into(),
            async move {
                Ok(Prepared {
                    release: preview(link.clone()).await?,
                    source: link,
                    icon: None,
                })
            },
            None,
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
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
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
                    Ok(Prepared {
                        release: preview(shown.clone()).await?,
                        source: shown,
                        icon: None,
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
        } = prepared;
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

    fn pick_icon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
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
        let typed = self.name.read(cx).value().trim().to_string();
        let name = match (typed.is_empty(), kind) {
            (false, _) => typed,
            (true, Some(kind)) => format!("{} {minecraft}", loader_display(kind)),
            (true, None) => format!("Minecraft {minecraft}"),
        };
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
                    .child(TextField::new(&self.query).leading(IconName::Search))
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

    fn render_custom(&self, height: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let pick_mc = cx.entity().downgrade();
        let pick_loader = pick_mc.clone();
        let pick_build = pick_mc.clone();
        let versions = self
            .releases
            .iter()
            .map(|r| MenuItem::new(r.clone(), r.clone()))
            .collect();
        let loaders = LOADERS
            .iter()
            .enumerate()
            .map(|(i, l)| MenuItem::new(i.to_string(), loader_label(*l)))
            .collect();
        let builds: Vec<MenuItem> = self
            .loader_versions
            .iter()
            .map(|v| MenuItem::new(v.clone(), v.clone()))
            .collect();
        let has_loader = LOADERS[self.loader].is_some();
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(14.))
            .child(field(
                t!("new_instance.name"),
                TextField::new(&self.name).w(px(FIELD_WIDTH)),
                cx,
            ))
            .child(field(
                t!("new_instance.minecraft"),
                Dropdown::new(
                    "minecraft",
                    versions,
                    self.minecraft.clone(),
                    move |v, _, cx| {
                        let _ = pick_mc.update(cx, |this, cx| {
                            this.minecraft = Some(v);
                            this.load_loader_versions(cx);
                        });
                    },
                )
                .width(px(FIELD_WIDTH))
                .placeholder(t!("new_instance.loading").to_string())
                .searchable(&self.mc_search),
                cx,
            ))
            .child(
                div()
                    .max_w(px(FIELD_WIDTH + 80.))
                    .text_color(c.muted)
                    .line_height(relative(1.5))
                    .child(t!("new_instance.description").to_string()),
            );
        let icon_preview = match &self.icon {
            Some((path, _)) => img(path.clone())
                .size(px(56.))
                .rounded(px(12.))
                .into_any_element(),
            None => div()
                .size(px(56.))
                .rounded(px(12.))
                .bg(c.sel)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconName::Image, c.muted).size(px(22.)))
                .into_any_element(),
        };
        let right = side_panel(height, cx)
            .gap(px(14.))
            .child(field(
                t!("new_instance.loader"),
                Dropdown::new(
                    "loader",
                    loaders,
                    Some(self.loader.to_string().into()),
                    move |v, _, cx| {
                        let _ = pick_loader.update(cx, |this, cx| {
                            this.loader = v.parse().unwrap_or(0);
                            this.load_loader_versions(cx);
                        });
                    },
                )
                .width(px(SIDE_WIDTH - 16.)),
                cx,
            ))
            .when(has_loader, |col| {
                col.child(field(
                    t!("new_instance.loader_version"),
                    Dropdown::new(
                        "loader-version",
                        builds,
                        self.loader_version.clone(),
                        move |v, _, cx| {
                            let _ = pick_build.update(cx, |this, cx| {
                                this.loader_version = Some(v);
                                cx.notify();
                            });
                        },
                    )
                    .width(px(SIDE_WIDTH - 16.))
                    .placeholder(t!("new_instance.loading").to_string()),
                    cx,
                ))
            })
            .child(field(
                t!("new_instance.icon"),
                h_flex()
                    .gap(px(12.))
                    .child(icon_preview)
                    .child(
                        Button::new("pick-icon")
                            .icon(IconName::Folder)
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
                cx,
            ));
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
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(14.))
            .child(field(
                t!("new_instance.link"),
                h_flex()
                    .gap(px(8.))
                    .child(TextField::new(&self.link).flex_1())
                    .child(
                        Button::new("check-link")
                            .label(t!("new_instance.check"))
                            .disabled(self.busy.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.check_link(cx))),
                    ),
                cx,
            ))
            .child(
                div()
                    .text_color(c.muted)
                    .line_height(relative(1.5))
                    .child(t!("new_instance.pack_description").to_string()),
            )
            .child(div().h(px(1.)).bg(c.border))
            .child(
                h_flex().child(
                    Button::new("pick-pack-file")
                        .icon(IconName::Folder)
                        .label(t!("new_instance.pick_file"))
                        .disabled(self.busy.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.pick_file(window, cx))),
                ),
            )
            .child(
                div()
                    .text_color(c.muted)
                    .line_height(relative(1.5))
                    .child(t!("new_instance.file_about").to_string()),
            );
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
        let cancel = Button::new("cancel")
            .outline()
            .size(ButtonSize::Md)
            .label(t!("common.cancel"))
            .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx)));
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
