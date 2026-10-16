mod content;
mod dialogs;
mod entry;
mod files;
mod git;
pub(super) mod highlight;
mod inspector;
mod overview;
mod panel;
mod releases;
mod shell;
mod sidebar;
mod test;
mod watch;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui_kit::base::input::{EditorState, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, DragMoveEvent, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, PathPromptOptions,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, Window, actions, div, px,
};
use riven_build::author::{AuthorError, CheckReport};
use riven_build::workspace::{OVERRIDE_SIDES, OVERRIDES, PROJECT_FILE, Workspace};
use riven_format::{Entry, Kind, PackPath, Project, Reason, Side};
use riven_resolve::Plan;
use rust_i18n::t;

use super::app::placeholder;
use super::file_tree::{FileTree, TreeEvent};
use super::runtime;
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{Button, ButtonSize, IconName, h_flex, motion, v_flex};

actions!(dev, [FocusSearch]);

const RECENT: usize = 8;
const KEY_CONTEXT: &str = "DevView";

/// Binds the developer section's shortcuts.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-k", FocusSearch, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-k", FocusSearch, Some(KEY_CONTEXT)),
    ]);
}

/// What the narrow bar on the left switches the sidebar to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    Content,
    Files,
    Search,
    Git,
}

/// Which entries a list tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum View {
    Kind(Kind),
    Updates,
    Changed,
    Problems,
    Dependencies,
    Side(Side),
    Disabled,
    Group(String),
}

impl View {
    fn label(&self, project: Option<&Project>) -> SharedString {
        match self {
            View::Kind(kind) => kind_label(*kind),
            View::Updates => t!("dev.view_updates").into(),
            View::Changed => t!("dev.view_changed").into(),
            View::Problems => t!("dev.view_problems").into(),
            View::Dependencies => t!("dev.view_dependencies").into(),
            View::Side(Side::Client) => t!("dev.view_client").into(),
            View::Side(Side::Server) => t!("dev.view_server").into(),
            View::Side(Side::Both) => t!("dev.side_both").into(),
            View::Disabled => t!("dev.view_disabled").into(),
            View::Group(id) => project
                .and_then(|p| p.groups.iter().find(|g| &g.id == id))
                .map_or_else(|| id.clone(), |g| g.name.clone())
                .into(),
        }
    }
}

fn kind_label(kind: Kind) -> SharedString {
    match kind {
        Kind::Mod => t!("dev.kind_mods"),
        Kind::ResourcePack => t!("dev.kind_resourcepacks"),
        Kind::ShaderPack => t!("dev.kind_shaders"),
        Kind::DataPack => t!("dev.kind_datapacks"),
        Kind::File => t!("dev.kind_files"),
    }
    .into()
}

/// An open tab in the middle.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tab {
    /// The pack itself: its facts and optional groups.
    Pack,
    List(View),
    Entry(String),
    File(PackPath),
    Releases,
    History,
}

impl Tab {
    fn key(&self) -> String {
        match self {
            Tab::Pack => "pack".into(),
            Tab::List(view) => format!("list-{view:?}"),
            Tab::Entry(id) => format!("entry-{id}"),
            Tab::File(path) => path.as_str().to_owned(),
            Tab::Releases => "releases".into(),
            Tab::History => "history".into(),
        }
    }

    fn icon(&self) -> IconName {
        match self {
            Tab::Pack => IconName::Package,
            Tab::List(_) => IconName::LayoutGrid,
            Tab::Entry(_) => IconName::Package,
            Tab::File(_) => IconName::File,
            Tab::Releases => IconName::Tag,
            Tab::History => IconName::History,
        }
    }
}

/// A border the user drags to resize what lies beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Edge {
    Sidebar,
    Inspector,
    /// A column of the content list: name, file, source, side, group.
    Column(usize),
}

/// What is dragged while an [`Edge`] moves.
struct EdgeDrag;

const COLUMN_DEFAULTS: [f32; 5] = [220., 200., 90., 70., 110.];
const COLUMN_MIN: f32 = 50.;
const COLUMN_MAX: f32 = 640.;
/// The middle keeps at least this much while the side panels are dragged wider.
const CENTER_MIN: f32 = 480.;

struct OpenFile {
    editor: Entity<EditorState>,
    /// The text on disk, to tell unsaved changes.
    saved: String,
    dirty: bool,
    _change: Subscription,
}

/// The bottom panel's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Problems,
    Output,
    Log,
}

enum Check {
    Idle,
    Running,
    Done(CheckReport),
    Failed(SharedString),
}

/// Newer versions of the pack's content, looked up on request.
enum Updates {
    Unchecked,
    Checking,
    Ready(Plan),
}

/// The developer section: one pack project laid out like an IDE.
pub struct DevView {
    project: Option<Workspace>,
    error: Option<SharedString>,
    /// The outcome of the last action that is not a failure.
    notice: Option<SharedString>,
    /// A long operation in progress, shown in the top bar.
    busy: Option<SharedString>,
    activity: Activity,
    tabs: Vec<Tab>,
    active: Tab,
    /// The entry the inspector on the right describes.
    selected: Option<String>,
    /// Sidebar groups folded by the user, by key.
    folded: HashSet<&'static str>,
    /// The `overrides/` tree, by paths under `overrides/`.
    tree: Entity<FileTree>,
    /// Every file under `overrides/`, for search and the inspector's configs.
    override_files: Vec<PackPath>,
    files: HashMap<PackPath, OpenFile>,
    panel: Panel,
    check: Check,
    check_serial: u64,
    /// The list tabs' filter.
    filter: Entity<InputState>,
    /// The search sidebar's query.
    search: Entity<InputState>,
    /// Indexes into the project's content the active list shows, sorted by name.
    shown: Vec<usize>,
    scroll: UniformListScrollHandle,
    updates: Updates,
    /// Modrinth facts about entries, for their pages and the inspector.
    pages: HashMap<String, entry::Page>,
    /// Pre-release versions are listed on entry pages.
    betas: bool,
    releases: releases::Releases,
    git: git::GitState,
    /// The bottom panel's stream, kept at the end while git output or the test log grows.
    panel_list: UniformListScrollHandle,
    panel_seen: usize,
    /// The panel's height while its edge is dragged; saved to the settings on drop.
    panel_drag: Option<f32>,
    watch: Option<watch::Watch>,
    /// Files changed on disk: open editors read them again on the next frame.
    reload_files: bool,
    /// The edge being dragged, where the drag started and the width then.
    resizing: Option<(Edge, f32, f32)>,
    /// Widths while their edge is dragged; saved to the settings on drop.
    live_widths: HashMap<Edge, f32>,
    /// How wide the content list's columns area was last drawn, to keep a dragged column on screen.
    columns_width: std::rc::Rc<std::cell::Cell<f32>>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

/// Shrinks column widths to `room`: first what they were widened past their defaults, then evenly.
fn fit_columns(mut widths: [f32; 5], room: f32) -> [f32; 5] {
    let mut excess = widths.iter().sum::<f32>() - room;
    // Before the list is first drawn its width is unknown.
    if excess <= 0. || room <= 0. {
        return widths;
    }
    for floor in [COLUMN_DEFAULTS, [COLUMN_MIN; 5]] {
        let slack: [f32; 5] = std::array::from_fn(|i| (widths[i] - floor[i]).max(0.));
        let total: f32 = slack.iter().sum();
        if total <= 0. {
            continue;
        }
        let take = excess.min(total);
        for (w, s) in widths.iter_mut().zip(slack) {
            *w -= s * take / total;
        }
        excess -= take;
        if excess <= 0. {
            break;
        }
    }
    widths
}

fn display_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

impl Focusable for DevView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DevView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("dev.filter").to_string()));
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("dev.search_placeholder").to_string())
        });
        let tree = cx.new(|_| FileTree::new("dev-tree").movable());
        let subs = vec![
            cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.refresh_rows(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.observe(&AppState::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe(&tree, |this, _, event: &TreeEvent, cx| {
                this.on_tree(event, cx)
            }),
        ];
        let releases = releases::Releases::new(window, cx);
        let git = git::GitState::new(window, cx);
        let pack_view = Tab::List(View::Kind(Kind::Mod));
        let mut view = Self {
            project: None,
            error: None,
            notice: None,
            busy: None,
            activity: Activity::Content,
            tabs: vec![Tab::Pack, pack_view.clone()],
            active: pack_view,
            selected: None,
            folded: HashSet::new(),
            tree,
            override_files: Vec::new(),
            files: HashMap::new(),
            panel: Panel::Problems,
            check: Check::Idle,
            check_serial: 0,
            filter,
            search,
            shown: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            updates: Updates::Unchecked,
            pages: HashMap::new(),
            betas: false,
            releases,
            git,
            panel_list: UniformListScrollHandle::new(),
            panel_seen: 0,
            panel_drag: None,
            watch: None,
            reload_files: false,
            resizing: None,
            live_widths: HashMap::new(),
            columns_width: Default::default(),
            focus: cx.focus_handle(),
            _subs: subs,
        };
        let last = AppState::global(cx)
            .read(cx)
            .settings
            .recent_projects
            .first()
            .cloned();
        if let Some(path) = last {
            view.open(PathBuf::from(path), cx);
        }
        view
    }

    /// Opens the project in `dir` and remembers it among the recent ones.
    fn open(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        match Workspace::open(&dir) {
            Ok(ws) => self.adopt(ws, cx),
            Err(e) => {
                self.error = Some(e.to_string().into());
                cx.notify();
            }
        }
    }

    fn adopt(&mut self, ws: Workspace, cx: &mut Context<Self>) {
        let path = ws.dir.display().to_string();
        AppState::global(cx).update(cx, |s, cx| {
            s.update_settings(
                |settings| {
                    let recent = &mut settings.recent_projects;
                    recent.retain(|p| p != &path);
                    recent.insert(0, path);
                    recent.truncate(RECENT);
                },
                cx,
            )
        });
        let switched = self.project.as_ref().is_some_and(|old| old.dir != ws.dir);
        let rewatch = switched || self.watch.is_none();
        self.project = Some(ws);
        if rewatch {
            self.watch(cx);
        }
        self.error = None;
        self.updates = Updates::Unchecked;
        if switched {
            let mods = Tab::List(View::Kind(Kind::Mod));
            self.files.clear();
            self.tabs = vec![Tab::Pack, mods.clone()];
            self.active = mods;
            self.selected = None;
            self.pages.clear();
        }
        self.tree.update(cx, |tree, cx| {
            for side in OVERRIDE_SIDES {
                tree.expand(side, cx);
            }
        });
        self.refresh_tree(cx);
        self.refresh_dist();
        self.refresh_rows(cx);
        self.run_check(cx);
        self.git.repo = git::Repo::Unknown;
        self.refresh_git(cx);
        self.check_updates(false, cx);
        cx.notify();
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("dev.open").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if let Some(dir) = paths.into_iter().next() {
                let _ = this.update(cx, |this, cx| this.open(dir, cx));
            }
        })
        .detach();
    }

    /// Entries the check found problems with, by id.
    fn troubled(&self) -> HashSet<String> {
        match &self.check {
            Check::Done(report) => report
                .problems
                .iter()
                .map(|p| p.entry().to_owned())
                .collect(),
            _ => HashSet::new(),
        }
    }

    /// Entries that differ from the `riven.json` committed in `HEAD`, by id.
    fn changed(&self) -> HashSet<String> {
        let (Some(ws), git::Repo::Ready(status)) = (&self.project, &self.git.repo) else {
            return HashSet::new();
        };
        let Some(committed) = &status.committed else {
            return ws.project.content.iter().map(|e| e.id.clone()).collect();
        };
        ws.project
            .content
            .iter()
            .filter(|e| committed.entry(&e.id) != Some(*e))
            .map(|e| e.id.clone())
            .collect()
    }

    fn updatable(&self) -> HashSet<String> {
        match &self.updates {
            Updates::Ready(plan) => plan.update.iter().map(|(old, _)| old.id.clone()).collect(),
            _ => HashSet::new(),
        }
    }

    /// A predicate telling which entries a view holds, with what it needs computed once.
    fn view_filter(&self, view: &View) -> impl Fn(&Entry) -> bool + use<> {
        let ids = match view {
            View::Updates => Some(self.updatable()),
            View::Changed => Some(self.changed()),
            View::Problems => Some(self.troubled()),
            _ => None,
        };
        let view = view.clone();
        move |e: &Entry| match &view {
            View::Kind(kind) => e.kind == *kind,
            View::Updates | View::Changed | View::Problems => {
                ids.as_ref().is_some_and(|ids| ids.contains(&e.id))
            }
            View::Dependencies => e.reason == Reason::Dependency,
            View::Side(side) => e.side == *side,
            View::Disabled => e.disabled,
            View::Group(id) => e.group.as_deref() == Some(id.as_str()),
        }
    }

    fn view_count(&self, view: &View) -> usize {
        let keep = self.view_filter(view);
        self.project.as_ref().map_or(0, |ws| {
            ws.project.content.iter().filter(|e| keep(e)).count()
        })
    }

    fn refresh_rows(&mut self, cx: &App) {
        let needle = self.filter.read(cx).value().trim().to_lowercase();
        let (Some(ws), Tab::List(view)) = (&self.project, &self.active) else {
            self.shown.clear();
            return;
        };
        let keep = self.view_filter(view);
        let content = &ws.project.content;
        self.shown = (0..content.len())
            .filter(|&i| {
                let e = &content[i];
                keep(e)
                    && (needle.is_empty()
                        || e.name.to_lowercase().contains(&needle)
                        || e.id.contains(&needle)
                        || e.file.path.file_name().to_lowercase().contains(&needle))
            })
            .collect();
        self.shown
            .sort_by_cached_key(|&i| content[i].name.to_lowercase());
    }

    /// Changes `riven.json` and writes it; a refused change leaves the project as it was.
    fn edit(
        &mut self,
        change: impl FnOnce(&mut Project) -> Result<(), AuthorError>,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = &self.project else {
            return;
        };
        let mut next = ws.clone();
        let result = change(&mut next.project).and_then(|()| next.save());
        match result {
            Ok(()) => {
                self.project = Some(next);
                self.forget_project_file();
                self.error = None;
                self.notice = None;
                if let Updates::Ready(plan) = &mut self.updates {
                    let project = &self.project.as_ref().expect("just saved").project;
                    plan.update
                        .retain(|(old, _)| project.entry(&old.id) == Some(old));
                }
                self.releases.changes = releases::Changes::Stale;
                self.forget_closed_entries(cx);
                self.refresh_rows(cx);
                self.run_check(cx);
                self.refresh_git(cx);
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
        cx.notify();
    }

    /// Closes the pages of entries that left the project.
    fn forget_closed_entries(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = &self.project else {
            return;
        };
        let gone: Vec<Tab> = self
            .tabs
            .iter()
            .filter(|t| matches!(t, Tab::Entry(id) if ws.project.entry(id).is_none()))
            .cloned()
            .collect();
        if self
            .selected
            .as_ref()
            .is_some_and(|id| ws.project.entry(id).is_none())
        {
            self.selected = None;
        }
        for tab in gone {
            self.drop_tab(tab, cx);
        }
    }

    fn apply_plan(&mut self, plan: &Plan, cx: &mut Context<Self>) {
        self.edit(
            |project| {
                plan.apply(project);
                Ok(())
            },
            cx,
        );
    }

    /// Plans a change in the background; the result opens in a preview to apply or drop.
    fn plan<F>(
        &mut self,
        title: SharedString,
        busy: SharedString,
        job: F,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        F: FnOnce(
                Workspace,
            )
                -> std::pin::Pin<Box<dyn Future<Output = Result<Plan, AuthorError>> + Send>>
            + Send
            + 'static,
    {
        let Some(ws) = self.project.clone() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(busy);
        self.error = None;
        self.notice = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = runtime::spawn(job(ws))
                .await
                .unwrap_or_else(|_| Err(AuthorError::NoDataDir));
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(plan) if plan.is_empty() && plan.notes.is_empty() => {
                        this.notice = Some(t!("dev.nothing_to_do").into());
                    }
                    Ok(plan) => {
                        let view = cx.entity().downgrade();
                        dialogs::open_preview(title, plan, view, window, cx);
                    }
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Turns the last outcome and failure into notifications.
    fn flush_messages(&mut self, cx: &mut Context<Self>) {
        let notice = self.notice.take();
        let error = self.error.take();
        if notice.is_none() && error.is_none() {
            return;
        }
        cx.defer(move |cx| {
            if let Some(text) = notice {
                super::toast::show(super::toast::ToastKind::Success, text, cx);
            }
            if let Some(text) = error {
                super::toast::show(super::toast::ToastKind::Error, text, cx);
            }
        });
    }

    /// Runs a long operation in the background, shown in the top bar; one at a time.
    fn job<T, E, Fut>(
        &mut self,
        busy: SharedString,
        work: Fut,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) where
        T: Send + 'static,
        E: ToString + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(busy);
        self.error = None;
        self.notice = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::spawn(async move { work.await.map_err(|e| e.to_string()) })
                .await
                .unwrap_or_else(|_| Err(String::new()));
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

    /// Runs `riven check` against the saved project; a newer run supersedes an older one.
    fn run_check(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.project.clone() else {
            return;
        };
        self.check_serial += 1;
        let serial = self.check_serial;
        self.check = Check::Running;
        cx.spawn(async move |this, cx| {
            let result = runtime::spawn(async move { riven_build::author::check(&ws).await })
                .await
                .unwrap_or_else(|_| Err(AuthorError::NoDataDir));
            let _ = this.update(cx, |this, cx| {
                if this.check_serial != serial {
                    return;
                }
                this.check = match result {
                    Ok(report) => Check::Done(report),
                    Err(e) => Check::Failed(e.to_string().into()),
                };
                if matches!(this.active, Tab::List(View::Problems)) {
                    this.refresh_rows(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if !self.tabs.contains(&tab) {
            self.tabs.push(tab.clone());
        }
        if let Tab::Entry(id) = &tab {
            self.select_entry(id.clone(), cx);
        }
        self.active = tab;
        self.refresh_rows(cx);
        cx.notify();
    }

    /// Shows an entry in the inspector, loading what Modrinth knows about it.
    fn select_entry(&mut self, id: String, cx: &mut Context<Self>) {
        self.load_page(&id, cx);
        self.selected = Some(id);
        cx.notify();
    }

    /// Closes a tab; a file with unsaved changes asks first.
    fn close_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Tab::File(path) = &tab
            && self.files.get(path).is_some_and(|f| f.dirty)
        {
            let view = cx.entity().downgrade();
            let path = path.clone();
            super::dialogs::confirm(
                t!("dev.discard_title", name = path.file_name()),
                t!("dev.discard_body"),
                t!("dev.discard"),
                move |_, cx| {
                    let path = path.clone();
                    let _ = view.update(cx, |this, cx| this.drop_tab(Tab::File(path), cx));
                },
                cx,
            );
            return;
        }
        self.drop_tab(tab, cx);
    }

    fn drop_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let Some(at) = self.tabs.iter().position(|t| *t == tab) else {
            return;
        };
        self.tabs.remove(at);
        if let Tab::File(path) = &tab {
            self.files.remove(path);
        }
        if self.active == tab {
            self.active = self
                .tabs
                .get(at.saturating_sub(1))
                .cloned()
                .unwrap_or(Tab::Pack);
            if self.tabs.is_empty() {
                self.tabs.push(self.active.clone());
            }
            self.refresh_rows(cx);
        }
        cx.notify();
    }

    fn pick_activity(&mut self, activity: Activity, window: &mut Window, cx: &mut Context<Self>) {
        let hidden = self.layout(cx).sidebar_hidden;
        if self.activity == activity && !hidden {
            self.set_layout(|p| p.sidebar_hidden = true, cx);
        } else {
            self.activity = activity;
            if hidden {
                self.set_layout(|p| p.sidebar_hidden = false, cx);
            }
        }
        if self.activity == Activity::Search && !self.layout(cx).sidebar_hidden {
            self.search.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.activity = Activity::Search;
        if self.layout(cx).sidebar_hidden {
            self.set_layout(|p| p.sidebar_hidden = false, cx);
        }
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn layout(&self, cx: &App) -> riven_format::DevPanel {
        AppState::global(cx).read(cx).settings.dev_panel
    }

    fn set_layout(
        &mut self,
        edit: impl FnOnce(&mut riven_format::DevPanel),
        cx: &mut Context<Self>,
    ) {
        AppState::global(cx).update(cx, |s, cx| {
            s.update_settings(|p| edit(&mut p.dev_panel), cx)
        });
    }

    fn width(&self, edge: Edge, cx: &App) -> f32 {
        match edge {
            Edge::Column(i) => self.column_widths(cx)[i],
            edge => self.set_width(edge, cx),
        }
    }

    /// The content list's columns as set, shrunk to the list's last drawn width; set widths return with the room.
    pub(super) fn column_widths(&self, cx: &App) -> [f32; 5] {
        let set = std::array::from_fn(|i| self.set_width(Edge::Column(i), cx));
        fit_columns(set, self.columns_width.get() - content::COLUMN_GAPS)
    }

    /// The width being dragged or saved, before the columns are fitted to the list.
    fn set_width(&self, edge: Edge, cx: &App) -> f32 {
        if let Some(w) = self.live_widths.get(&edge) {
            return *w;
        }
        let prefs = self.layout(cx);
        let (saved, fallback) = match edge {
            Edge::Sidebar => (prefs.sidebar_width, sidebar::SIDEBAR_WIDTH),
            Edge::Inspector => (prefs.inspector_width, inspector::INSPECTOR_WIDTH),
            Edge::Column(i) => (prefs.columns[i], COLUMN_DEFAULTS[i]),
        };
        if saved == 0 { fallback } else { saved as f32 }
    }

    /// A thin strip on a border that resizes `edge` when dragged; place it in a `relative` parent.
    fn edge_handle(&self, edge: Edge, cx: &mut Context<Self>) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id(SharedString::from(format!("dev-edge-{edge:?}")))
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(6.))
            .cursor_col_resize()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    let width = this.width(edge, cx);
                    this.resizing = Some((edge, f32::from(event.position.x), width));
                }),
            )
            .on_drag(EdgeDrag, |_, _, _, cx| cx.new(|_| gpui_kit::EmptyView))
    }

    fn drag_edge(&mut self, event: &DragMoveEvent<EdgeDrag>, cx: &mut Context<Self>) {
        let Some((edge, start, from)) = self.resizing else {
            return;
        };
        let delta = f32::from(event.event.position.x) - start;
        let layout = self.layout(cx);
        let side = |hidden: bool, edge: Edge| if hidden { 0. } else { self.width(edge, cx) };
        let (sidebar, inspector) = (
            side(layout.sidebar_hidden, Edge::Sidebar),
            side(layout.inspector_hidden, Edge::Inspector),
        );
        let total = f32::from(event.bounds.size.width) - shell::ACTIVITY_WIDTH;
        let (width, min, max) = match edge {
            Edge::Sidebar => (from + delta, 180., total - inspector - CENTER_MIN),
            Edge::Inspector => (from - delta, 240., total - sidebar - CENTER_MIN),
            Edge::Column(i) => {
                // Widening stops where the columns would leave the list; the others keep their widths.
                let others: f32 = (0..COLUMN_DEFAULTS.len())
                    .filter(|j| *j != i)
                    .map(|j| self.width(Edge::Column(j), cx))
                    .sum();
                let room = self.columns_width.get() - others - content::COLUMN_GAPS;
                let max = if room > 0. {
                    room.min(COLUMN_MAX)
                } else {
                    COLUMN_MAX
                };
                (from + delta, COLUMN_MIN, max.max(COLUMN_MIN))
            }
        };
        self.live_widths.insert(edge, width.min(max).max(min));
        cx.notify();
    }

    fn drop_edge(&mut self, cx: &mut Context<Self>) {
        let Some((edge, ..)) = self.resizing.take() else {
            return;
        };
        // A column set in a narrow window keeps the others as they were shown, not as once saved.
        let shown = self.column_widths(cx).map(|w| w.round() as u32);
        let Some(width) = self.live_widths.remove(&edge) else {
            return;
        };
        let width = width.round() as u32;
        self.set_layout(
            |p| match edge {
                Edge::Sidebar => p.sidebar_width = width,
                Edge::Inspector => p.inspector_width = width,
                Edge::Column(i) => {
                    p.columns = shown;
                    p.columns[i] = width;
                }
            },
            cx,
        );
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        placeholder(
            IconName::Code,
            t!("dev.empty_title").into(),
            t!("dev.empty_hint").into(),
            cx,
        )
        .child(
            h_flex()
                .mt(px(8.))
                .gap(px(8.))
                .child(
                    Button::new("dev-empty-open")
                        .size(ButtonSize::Md)
                        .icon(IconName::Folder)
                        .label(t!("dev.open"))
                        .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                )
                .child(
                    Button::new("dev-empty-create")
                        .primary()
                        .size(ButtonSize::Md)
                        .icon(IconName::Plus)
                        .label(t!("dev.create"))
                        .on_click(move |_, window, cx| {
                            dialogs::open_create(view.clone(), window, cx)
                        }),
                ),
        )
        .into_any_element()
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.active.clone() {
            Tab::Pack => self.render_overview(cx),
            Tab::List(view) => self.render_content(&view, cx),
            Tab::Entry(id) => self.render_entry_page(&id, cx),
            Tab::File(path) => self.render_file(&path, cx),
            Tab::Releases => self.render_releases(cx),
            Tab::History => self.render_history(cx),
        }
    }
}

impl Render for DevView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_editor(window, cx);
        self.settle_git(window, cx);
        self.flush_messages(cx);
        self.reload_open_files(window, cx);
        self.follow_stream(cx);
        if self.active == Tab::Releases {
            self.ensure_changes(cx);
        }
        let c = cx.theme().colors;
        let layout = self.layout(cx);
        let root =
            v_flex()
                .id("dev-view")
                .key_context(KEY_CONTEXT)
                .track_focus(&self.focus)
                .on_action(
                    cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)),
                )
                .on_drag_move(cx.listener(|this, event: &DragMoveEvent<EdgeDrag>, _, cx| {
                    this.drag_edge(event, cx)
                }))
                .on_drop(cx.listener(|this, _: &EdgeDrag, _, cx| this.drop_edge(cx)))
                .size_full()
                .bg(c.bg);
        if self.project.is_none() {
            return root
                .child(self.render_top(cx))
                .child(div().flex_1().min_h_0().child(self.render_empty(cx)));
        }
        let body = self.render_body(cx);
        let tab = SharedString::from(format!("dev-tab-body-{}", self.active.key()));
        let center = v_flex()
            .id("dev-center")
            .flex_1()
            .min_w(px(CENTER_MIN))
            .h_full()
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<panel::PanelResize>, _, cx| {
                    this.drag_panel(event, cx)
                }),
            )
            .on_drop(cx.listener(|this, _: &panel::PanelResize, _, cx| this.drop_panel(cx)))
            .child(self.render_tabs(cx))
            .child(motion::enter(
                tab,
                v_flex().flex_1().min_h_0().child(body),
                6.,
                window,
                cx,
            ))
            .child(self.render_panel(cx));
        root.child(self.render_top(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_activity(cx))
                    .when(!layout.sidebar_hidden, |row| {
                        row.child(self.render_sidebar(window, cx))
                    })
                    .child(center)
                    .when(!layout.inspector_hidden, |row| {
                        row.child(self.render_inspector(cx))
                    }),
            )
            .child(self.render_status(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_widened_in_a_big_window_fit_a_small_one() {
        let wide = [291., 640., 244., 211., 110.];
        assert_eq!(fit_columns(wide, 2000.), wide);
        assert_eq!(fit_columns(wide, 0.), wide);

        let fitted = fit_columns(wide, 800.);
        assert!((fitted.iter().sum::<f32>() - 800.).abs() < 0.01);
        assert!(
            fitted
                .iter()
                .zip(COLUMN_DEFAULTS)
                .all(|(w, d)| *w >= d - 0.01)
        );

        let tight = fit_columns(wide, 300.);
        assert!((tight.iter().sum::<f32>() - 300.).abs() < 0.01);
        assert!(tight.iter().all(|w| *w >= COLUMN_MIN - 0.01));
    }
}
