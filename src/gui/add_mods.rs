use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use futures_util::{StreamExt as _, stream};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle,
    WeakEntity, Window, div, img, px, relative, uniform_list,
};
use riven_format::{Reason, Source as EntrySource};
use riven_launch::LaunchError;
use riven_launch::instances::Instances;
use riven_launch::own::Workbench;
use riven_resolve::{Plan, ResolveError};
use riven_sources::modrinth::ProjectPage;
use riven_sources::{Cache, Hit, Modrinth, Source as _, Support, Target};
use rust_i18n::t;
use tokio::sync::mpsc;

use super::instance::InstanceView;
use super::mods;
use super::runtime;
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{
    Button, ButtonSize, IconName, Tabs, TextField, h_flex, icon, motion, scrollbar, v_flex,
};

const RESULTS: u32 = 40;
const SEARCH_DELAY: Duration = Duration::from_millis(250);
const PARALLEL_ICONS: usize = 8;
const HIT_HEIGHT: f32 = 58.;
const WIDTH: f32 = 1080.;
const DETAILS_WIDTH: f32 = 360.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Modrinth,
    Link,
    File,
}

const MODES: [Mode; 3] = [Mode::Modrinth, Mode::Link, Mode::File];

/// What to install: Modrinth projects (id, title), a link to a jar, or jars from disk.
enum Job {
    Projects(Vec<(String, String)>),
    Link(String),
    Files(Vec<PathBuf>),
}

/// What an install did, project by project.
#[derive(Default)]
struct Outcome {
    plans: Vec<Plan>,
    /// Projects the instance already had.
    already: Vec<String>,
    /// `(title, error)` of projects that could not be installed.
    failed: Vec<(String, String)>,
}

/// A project page, fetched when its project is first shown in the details.
#[derive(Clone)]
enum Page {
    Loading,
    Ready(Rc<ProjectPage>),
    Failed,
}

impl PartialEq for Page {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Page::Ready(a), Page::Ready(b)) => Rc::ptr_eq(a, b),
            (Page::Loading, Page::Loading) | (Page::Failed, Page::Failed) => true,
            _ => false,
        }
    }
}

/// Everything the details panel shows, compared to skip drawing it again.
#[derive(Clone, PartialEq)]
struct Shown {
    hit: Hit,
    icon: Option<PathBuf>,
    page: Option<Page>,
    installed: bool,
    installing: bool,
    busy: bool,
    selected: bool,
}

/// The project page on the right; a view of its own so hovering the list does not relay it out.
struct Details {
    add: WeakEntity<AddMods>,
    shown: Option<Shown>,
    height: f32,
}

/// The "Add mods" dialog of an instance.
pub struct AddMods {
    id: String,
    view: WeakEntity<InstanceView>,
    target: Option<Target>,
    mode: Mode,
    query: Entity<InputState>,
    link: Entity<InputState>,
    hits: Vec<Hit>,
    icons: HashMap<String, PathBuf>,
    searching: bool,
    search_serial: u64,
    /// Modrinth projects the instance already has.
    installed: HashSet<String>,
    /// Projects picked for installing together, in the order they were picked.
    selected: Vec<Hit>,
    /// The project shown on the right.
    focus: Option<Hit>,
    pages: HashMap<String, Page>,
    details: Entity<Details>,
    /// The project id, link or file being installed.
    installing: Option<String>,
    /// `(done, total, current title)` while several projects install.
    progress: Option<(usize, usize, String)>,
    /// The last outcome; `true` marks a failure.
    status: Option<(bool, SharedString)>,
    problems: Vec<SharedString>,
    scroll: UniformListScrollHandle,
    _subs: Vec<Subscription>,
}

fn modrinth() -> Modrinth {
    let modrinth = Modrinth::new(riven_sources::client());
    match mods::cache_dir() {
        Some(dir) => modrinth.with_cache(Cache::new(dir.join("api"), Duration::from_secs(3600))),
        None => modrinth,
    }
}

/// The project slug of a Modrinth project or version page.
fn modrinth_slug(link: &str) -> Option<String> {
    let url = url::Url::parse(link).ok()?;
    if !url.host_str()?.ends_with("modrinth.com") {
        return None;
    }
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        [_, slug] | [_, slug, "version", ..] => Some((*slug).to_owned()),
        _ => None,
    }
}

fn compact(n: u64) -> String {
    let short = |v: f64, unit: &str| {
        let text = format!("{v:.1}");
        format!("{}{unit}", text.trim_end_matches(".0"))
    };
    match n {
        1_000_000.. => short(n as f64 / 1e6, "M"),
        1_000.. => short(n as f64 / 1e3, "K"),
        _ => n.to_string(),
    }
}

/// Installs one Modrinth project with its dependencies.
async fn install_project(store: &Instances, id: &str, project: &str) -> Result<Plan, LaunchError> {
    let bench = Workbench::open(store, id).await?;
    let plan = bench.add(project).await?;
    bench.apply(&plan).await?;
    Ok(plan)
}

async fn install(
    store: Instances,
    id: String,
    job: Job,
    progress: mpsc::UnboundedSender<(usize, usize, String)>,
) -> Result<Outcome, LaunchError> {
    let mut outcome = Outcome::default();
    match job {
        Job::Projects(projects) => {
            let total = projects.len();
            for (done, (project, title)) in projects.into_iter().enumerate() {
                let _ = progress.send((done, total, title.clone()));
                match install_project(&store, &id, &project).await {
                    Ok(plan) => outcome.plans.push(plan),
                    Err(LaunchError::Resolve(ResolveError::AlreadyPresent { .. })) => {
                        outcome.already.push(project)
                    }
                    Err(e) if total == 1 => return Err(e),
                    Err(e) => outcome.failed.push((title, e.to_string())),
                }
            }
        }
        Job::Link(link) => {
            let bench = Workbench::open(&store, &id).await?;
            let plan = match modrinth_slug(&link) {
                Some(slug) => bench.add(&slug).await?,
                None => bench.add_url(&link).await?,
            };
            bench.apply(&plan).await?;
            outcome.plans.push(plan);
        }
        Job::Files(paths) => {
            for path in paths {
                let bench = Workbench::open(&store, &id).await?;
                let plan = bench.add_file(&path).await?;
                bench.apply(&plan).await?;
                outcome.plans.push(plan);
            }
        }
    }
    Ok(outcome)
}

/// "Added Sodium and 2 dependencies" for what the plans installed.
fn summary(plans: &[Plan]) -> Option<SharedString> {
    let added = plans.iter().flat_map(|p| &p.add);
    let names: Vec<&str> = added
        .clone()
        .filter(|e| e.reason == Reason::Explicit)
        .map(|e| e.name.as_str())
        .collect();
    if names.is_empty() {
        return None;
    }
    let deps = added.filter(|e| e.reason == Reason::Dependency).count();
    let names = names.join(", ");
    Some(if deps == 0 {
        t!("add_mods.added", names = names).into()
    } else {
        t!("add_mods.added_with_deps", names = names, n = deps).into()
    })
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

fn support_label(support: Support) -> SharedString {
    match support {
        Support::Required => t!("add_mods.side_required"),
        Support::Optional => t!("add_mods.side_optional"),
        Support::Unsupported => t!("add_mods.side_unsupported"),
        Support::Unknown => "—".into(),
    }
    .into()
}

impl AddMods {
    fn new(
        id: String,
        view: WeakEntity<InstanceView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let target = AppState::global(cx).read(cx).instance(&id).and_then(|i| {
            Some(Target {
                minecraft: i.minecraft.clone(),
                loader: i.loader.as_ref()?.kind,
                kind: riven_format::Kind::Mod,
            })
        });
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("add_mods.search").to_string()));
        query.update(cx, |s, cx| s.focus(window, cx));
        let link = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("add_mods.link_hint").to_string())
        });
        let subs = vec![
            cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.search(cx);
                }
            }),
            cx.subscribe_in(&link, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_link(window, cx);
                }
            }),
        ];
        let add = cx.entity().downgrade();
        let details = cx.new(|_| Details {
            add,
            shown: None,
            height: 0.,
        });
        let mut this = Self {
            id,
            view,
            target,
            mode: Mode::Modrinth,
            query,
            link,
            hits: Vec::new(),
            icons: HashMap::new(),
            searching: false,
            search_serial: 0,
            installed: HashSet::new(),
            selected: Vec::new(),
            focus: None,
            pages: HashMap::new(),
            details,
            installing: None,
            progress: None,
            status: None,
            problems: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            _subs: subs,
        };
        this.search(cx);
        this.read_installed(cx);
        this
    }

    /// Learns which Modrinth projects the instance has, to mark them in the results.
    fn read_installed(&mut self, cx: &mut Context<Self>) {
        let Some(store) = AppState::global(cx).read(cx).store.clone() else {
            return;
        };
        let id = self.id.clone();
        cx.spawn(async move |this, cx| {
            let found = runtime::spawn(async move {
                Workbench::open(&store, &id)
                    .await
                    .map(|bench| bench.projects())
            })
            .await;
            if let Ok(Ok(projects)) = found {
                let _ = this.update(cx, |this, cx| {
                    this.installed.extend(projects);
                    this.selected.retain(|h| !this.installed.contains(&h.id));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Searches Modrinth shortly after typing stops; an empty query lists popular mods.
    fn search(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
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
                runtime::spawn(async move { modrinth().search(&query, &target, RESULTS).await })
                    .await
                    .unwrap_or_else(|_| Ok(vec![]));
            let hits = match found {
                Ok(hits) => hits,
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.search_serial == serial {
                            this.searching = false;
                            this.status = Some((true, e.to_string().into()));
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

    /// Shows a project on the right, fetching its page the first time.
    fn show(&mut self, hit: Hit, cx: &mut Context<Self>) {
        let id = hit.id.clone();
        self.focus = Some(hit);
        cx.notify();
        if self.pages.contains_key(&id) {
            return;
        }
        self.pages.insert(id.clone(), Page::Loading);
        cx.spawn(async move |this, cx| {
            let project = id.clone();
            let page = runtime::spawn(async move { modrinth().page(&project).await })
                .await
                .ok()
                .and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                let page = match page {
                    Some(page) => Page::Ready(Rc::new(page)),
                    None => Page::Failed,
                };
                this.pages.insert(id, page);
                cx.notify();
            });
        })
        .detach();
    }

    /// Hands the details panel what it shows; it is drawn again only when that changed.
    fn sync_details(&mut self, height: f32, cx: &mut Context<Self>) {
        let shown = self.focus.as_ref().map(|hit| Shown {
            hit: hit.clone(),
            icon: self.icons.get(&hit.id).cloned(),
            page: self.pages.get(&hit.id).cloned(),
            installed: self.installed.contains(&hit.id),
            installing: self.installing.as_deref() == Some(hit.id.as_str()),
            busy: self.installing.is_some(),
            selected: self.is_selected(&hit.id),
        });
        self.details.update(cx, |details, cx| {
            if details.shown != shown || details.height != height {
                details.shown = shown;
                details.height = height;
                cx.notify();
            }
        });
    }

    fn is_selected(&self, id: &str) -> bool {
        self.selected.iter().any(|h| h.id == id)
    }

    fn toggle(&mut self, hit: &Hit, cx: &mut Context<Self>) {
        if self.installed.contains(&hit.id) {
            return;
        }
        match self.selected.iter().position(|h| h.id == hit.id) {
            Some(at) => {
                self.selected.remove(at);
            }
            None => self.selected.push(hit.clone()),
        }
        cx.notify();
    }

    fn run(&mut self, job: Job, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.installing.is_some() {
            return;
        }
        let Some(store) = AppState::global(cx).read(cx).store.clone() else {
            return;
        };
        self.installing = Some(key);
        self.status = None;
        self.problems.clear();
        cx.notify();
        let id = self.id.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let done = runtime::spawn(install(store, id, job, tx));
        cx.spawn_in(window, async move |this, cx| {
            while let Some(step) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    this.progress = Some(step);
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = done
                .await
                .unwrap_or_else(|_| Err(LaunchError::Game("cancelled".into())));
            let _ = this.update_in(cx, |this, window, cx| {
                this.installing = None;
                this.progress = None;
                match result {
                    Ok(outcome) => this.finish(outcome, window, cx),
                    Err(LaunchError::Resolve(ResolveError::AlreadyPresent { .. })) => {
                        this.status = Some((false, t!("add_mods.already").into()));
                    }
                    Err(e) => {
                        tracing::warn!("{e}");
                        this.status = Some((true, e.to_string().into()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn finish(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        for entry in outcome.plans.iter().flat_map(|p| &p.add) {
            if let EntrySource::Modrinth { project, .. } = &entry.source {
                self.installed.insert(project.clone());
            }
        }
        self.installed.extend(outcome.already.iter().cloned());
        self.selected.retain(|h| !self.installed.contains(&h.id));
        self.problems = outcome
            .plans
            .iter()
            .flat_map(|p| &p.problems)
            .map(|p| p.to_string().into())
            .chain(
                outcome
                    .failed
                    .iter()
                    .map(|(title, e)| format!("{title}: {e}").into()),
            )
            .collect();
        let failed = !outcome.failed.is_empty();
        self.status = match summary(&outcome.plans) {
            Some(text) => Some((failed, text)),
            None if failed => Some((true, t!("add_mods.failed").into())),
            None => Some((false, t!("add_mods.already").into())),
        };
        if let Some((false, text)) = &self.status {
            super::toast::show(super::toast::ToastKind::Success, text.clone(), cx);
        }
        let _ = self.view.update(cx, |view, cx| view.reload(window, cx));
    }

    fn install_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let projects: Vec<(String, String)> = self
            .selected
            .iter()
            .map(|h| (h.id.clone(), h.title.clone()))
            .collect();
        if projects.is_empty() {
            return;
        }
        self.run(Job::Projects(projects), "selected".into(), window, cx);
    }

    fn add_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let link = self.link.read(cx).value().trim().to_string();
        if link.is_empty() {
            return;
        }
        self.run(Job::Link(link.clone()), link, window, cx);
    }

    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t!("add_mods.pick").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.run(Job::Files(paths), "files".into(), window, cx)
            });
        })
        .detach();
    }

    fn checkbox(&self, ix: usize, hit: &Hit, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let installed = self.installed.contains(&hit.id);
        let on = installed || self.is_selected(&hit.id);
        let toggled = hit.clone();
        div()
            .id(("hit-check", ix))
            .size(px(18.))
            .flex_none()
            .rounded(px(5.))
            .border_1()
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match (installed, on) {
                (true, _) => d.border_color(c.border).bg(c.sel),
                (false, true) => d.border_color(c.accent).bg(c.accent),
                _ => d.border_color(c.border).hover(|s| s.border_color(c.accent)),
            })
            .when(on, |d| {
                d.child(
                    icon(
                        IconName::Check,
                        if installed { c.muted } else { c.on_accent },
                    )
                    .size(px(12.)),
                )
            })
            .when(!installed, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle(&toggled, cx)
                    }))
            })
            .into_any_element()
    }

    fn render_hit(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors;
        let hit = self.hits.get(ix)?;
        let installed = self.installed.contains(&hit.id);
        let focused = self.focus.as_ref().is_some_and(|f| f.id == hit.id);
        let shown = hit.clone();
        Some(
            h_flex()
                .id(("hit", ix))
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
                .child(self.checkbox(ix, hit, cx))
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
                .when(installed, |r| {
                    r.child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(c.ok)
                            .child(t!("add_mods.installed").to_string()),
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
                        "hits-spinner",
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
                        "add-mods-hits",
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
            .child(
                self.details.clone().cached(
                    gpui_kit::StyleRefinement::default()
                        .w(px(DETAILS_WIDTH))
                        .h(px(height)),
                ),
            )
            .into_any_element()
    }

    fn render_link(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let busy = self.installing.is_some();
        v_flex()
            .gap(px(12.))
            .max_w(px(620.))
            .child(super::dialogs::field(
                t!("add_mods.link"),
                TextField::new(&self.link),
                cx,
            ))
            .child(
                div()
                    .text_color(c.muted)
                    .line_height(relative(1.5))
                    .child(t!("add_mods.link_about").to_string()),
            )
            .child(
                h_flex().child(
                    Button::new("add-link")
                        .primary()
                        .icon(IconName::Plus)
                        .label(if busy {
                            t!("add_mods.installing")
                        } else {
                            t!("add_mods.install")
                        })
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| this.add_link(window, cx))),
                ),
            )
            .into_any_element()
    }

    fn render_file(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let busy = self.installing.is_some();
        v_flex()
            .gap(px(12.))
            .max_w(px(620.))
            .child(
                div()
                    .text_color(c.muted)
                    .line_height(relative(1.5))
                    .child(t!("add_mods.file_about").to_string()),
            )
            .child(
                h_flex().child(
                    Button::new("pick-files")
                        .primary()
                        .icon(IconName::Folder)
                        .label(if busy {
                            t!("add_mods.installing")
                        } else {
                            t!("add_mods.pick")
                        })
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx))),
                ),
            )
            .into_any_element()
    }

    /// Under the list: what is picked, progress, and how the last install went.
    fn render_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let picked = self.selected.len();
        let left: AnyElement = if self.mode == Mode::Modrinth && picked > 0 {
            h_flex()
                .gap(px(8.))
                .child(t!("add_mods.picked", n = picked).to_string())
                .child(
                    Button::new("clear-selection")
                        .ghost()
                        .size(ButtonSize::Xs)
                        .label(t!("add_mods.clear"))
                        .disabled(self.installing.is_some())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.selected.clear();
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        };
        let right: AnyElement = match (&self.progress, &self.status) {
            (Some((done, total, title)), _) => h_flex()
                .gap(px(8.))
                .text_color(c.muted)
                .child(motion::spinner(
                    "install-spinner",
                    icon(IconName::Loader, c.muted).size(px(14.)),
                    cx,
                ))
                .child(
                    t!(
                        "add_mods.progress",
                        done = done + 1,
                        total = total,
                        name = title
                    )
                    .to_string(),
                )
                .into_any_element(),
            (None, Some((failed, text))) => div()
                .min_w_0()
                .truncate()
                .text_color(if *failed { c.warn } else { c.ok })
                .child(text.clone())
                .into_any_element(),
            _ => div().into_any_element(),
        };
        v_flex()
            .gap(px(4.))
            .child(
                h_flex()
                    .min_h(px(26.))
                    .gap(px(12.))
                    .child(left)
                    .child(div().flex_1())
                    .child(right),
            )
            .when(!self.problems.is_empty(), |col| {
                col.child(
                    v_flex()
                        .id("install-problems")
                        .max_h(px(72.))
                        .overflow_y_scroll()
                        .children(
                            self.problems.iter().map(|p| {
                                div().text_size(px(12.)).text_color(c.warn).child(p.clone())
                            }),
                        ),
                )
            })
            .into_any_element()
    }
}

impl Render for Details {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = self.height;
        let c = cx.theme().colors;
        let panel = v_flex()
            .id("add-mods-details")
            .w(px(DETAILS_WIDTH))
            .flex_none()
            .h(px(height))
            .overflow_y_scroll()
            .pl(px(16.))
            .border_l_1()
            .border_color(c.border);
        let Some(shown) = &self.shown else {
            return panel
                .justify_center()
                .items_center()
                .text_color(c.muted)
                .child(t!("add_mods.pick_one").to_string())
                .into_any_element();
        };
        let hit = &shown.hit;
        let (installed, installing, selected) = (shown.installed, shown.installing, shown.selected);
        let page = shown.page.as_ref();
        let ready = match page {
            Some(Page::Ready(ready)) => Some(&**ready),
            _ => None,
        };
        let slug = ready
            .map(|p| p.slug.clone())
            .unwrap_or_else(|| hit.slug.clone());
        let project = hit.id.clone();
        let title = hit.title.clone();
        let toggled = hit.clone();
        let action = match (installed, installing) {
            (true, _) => Button::new("details-install")
                .icon(IconName::Check)
                .label(t!("add_mods.installed"))
                .disabled(true),
            (false, true) => Button::new("details-install")
                .label(t!("add_mods.installing"))
                .disabled(true),
            (false, false) => Button::new("details-install")
                .primary()
                .icon(IconName::Plus)
                .label(t!("add_mods.install"))
                .disabled(shown.busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    let job = Job::Projects(vec![(project.clone(), title.clone())]);
                    let key = project.clone();
                    let _ = this.add.update(cx, |add, cx| add.run(job, key, window, cx));
                })),
        };
        let stats = match ready {
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
        let chip = |text: String| {
            div()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(10.))
                .bg(c.row)
                .text_size(px(11.))
                .text_color(c.text2)
                .child(text)
        };
        let fact = |label: SharedString, value: SharedString| {
            h_flex()
                .gap(px(8.))
                .text_size(px(12.))
                .child(
                    div()
                        .w(px(90.))
                        .flex_none()
                        .text_color(c.muted)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(value))
        };
        panel
            .gap(px(12.))
            .child(
                h_flex()
                    .gap(px(12.))
                    .child(logo(shown.icon.as_ref(), 56., cx))
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
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(action)
                    .when(!installed, |row| {
                        row.child(
                            Button::new("details-select")
                                .icon(if selected {
                                    IconName::Check
                                } else {
                                    IconName::Plus
                                })
                                .label(if selected {
                                    t!("add_mods.selected")
                                } else {
                                    t!("add_mods.select")
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let _ = this.add.update(cx, |add, cx| add.toggle(&toggled, cx));
                                })),
                        )
                    })
                    .child(
                        Button::new("details-open")
                            .ghost()
                            .icon(IconName::Compass)
                            .tooltip(t!("add_mods.open_page"))
                            .on_click(move |_, _, cx| {
                                cx.open_url(&format!("https://modrinth.com/project/{slug}"))
                            }),
                    ),
            )
            .map(|col| match (page, ready) {
                (_, Some(page)) => {
                    let links = page.links.iter().enumerate().map(|(i, (label, url))| {
                        let url = url.clone();
                        Button::new(("details-link", i))
                            .size(ButtonSize::Xs)
                            .label(match *label {
                                "source" => t!("add_mods.link_source"),
                                "issues" => t!("add_mods.link_issues"),
                                "wiki" => t!("add_mods.link_wiki"),
                                _ => "Discord".into(),
                            })
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    });
                    col.child(
                        h_flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .children(page.categories.iter().map(|c| chip(c.clone()))),
                    )
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .child(fact(
                                t!("add_mods.client").into(),
                                support_label(page.client),
                            ))
                            .child(fact(
                                t!("add_mods.server").into(),
                                support_label(page.server),
                            ))
                            .when_some(page.license.clone(), |col, l| {
                                col.child(fact(t!("add_mods.license").into(), l.into()))
                            }),
                    )
                    .when(!page.links.is_empty(), |col| {
                        col.child(h_flex().flex_wrap().gap(px(6.)).children(links))
                    })
                    .child(div().h(px(1.)).bg(c.border))
                    .children(blocks.iter().take(SHOWN_BLOCKS).map(|block| {
                        match block {
                            Block::Heading(text) => div()
                                .pt(px(4.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(text.clone())
                                .into_any_element(),
                            Block::Paragraph(text) => div()
                                .text_color(c.text2)
                                .line_height(relative(1.5))
                                .child(text.clone())
                                .into_any_element(),
                            Block::Item(text) => h_flex()
                                .items_start()
                                .gap(px(8.))
                                .text_color(c.text2)
                                .line_height(relative(1.5))
                                .child(div().flex_none().child("•"))
                                .child(div().flex_1().min_w_0().child(text.clone()))
                                .into_any_element(),
                        }
                    }))
                }
                (Some(Page::Failed), _) => col.child(
                    div()
                        .text_color(c.muted)
                        .child(t!("add_mods.page_failed").to_string()),
                ),
                _ => col.child(
                    h_flex()
                        .gap(px(8.))
                        .text_color(c.muted)
                        .child(motion::spinner(
                            "page-spinner",
                            icon(IconName::Loader, c.muted).size(px(14.)),
                            cx,
                        ))
                        .child(t!("add_mods.loading_page").to_string()),
                ),
            })
            .child(div().h(px(8.)))
            .into_any_element()
    }
}

impl Render for AddMods {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = (f32::from(window.viewport_size().height) - 300.).clamp(240., 560.);
        self.sync_details(height, cx);
        let view = cx.entity().downgrade();
        let selected = MODES.iter().position(|m| *m == self.mode).unwrap_or(0);
        let tabs = Tabs::new(
            "add-mods-mode",
            vec![
                "Modrinth".into(),
                t!("add_mods.by_link").into(),
                t!("add_mods.from_file").into(),
            ],
            selected,
            move |ix, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.mode = MODES[ix];
                    let focus = match this.mode {
                        Mode::Modrinth => Some(&this.query),
                        Mode::Link => Some(&this.link),
                        Mode::File => None,
                    };
                    if let Some(input) = focus.cloned() {
                        input.update(cx, |s, cx| s.focus(window, cx));
                    }
                    cx.notify();
                });
            },
        );
        let content = match self.mode {
            Mode::Modrinth => self.render_modrinth(height, cx),
            Mode::Link => self.render_link(cx),
            Mode::File => self.render_file(cx),
        };
        let body = v_flex()
            .gap(px(14.))
            .child(h_flex().child(tabs))
            .child(content)
            .child(self.render_status(cx));
        let about = match &self.target {
            Some(target) => t!(
                "add_mods.description",
                minecraft = target.minecraft,
                loader = super::launch_bar::loader_display(target.loader)
            ),
            None => t!("mods.needs_loader"),
        };
        let picked = self.selected.len();
        let mut buttons = vec![
            Button::new("done")
                .outline()
                .size(ButtonSize::Md)
                .label(t!("add_mods.done"))
                .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx))),
        ];
        if self.mode == Mode::Modrinth {
            buttons.push(
                Button::new("install-selected")
                    .primary()
                    .size(ButtonSize::Md)
                    .icon(IconName::Plus)
                    .label(t!("add_mods.install_n", n = picked))
                    .disabled(picked == 0 || self.installing.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.install_selected(window, cx))),
            );
        }
        super::dialogs::dialog_shell(t!("add_mods.title"), about, body, buttons, cx)
    }
}

pub fn open(id: String, view: WeakEntity<InstanceView>, window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| AddMods::new(id, view, window, cx));
    super::dialogs::open(dialog.into(), WIDTH, cx);
}
