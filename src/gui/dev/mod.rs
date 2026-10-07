mod content;
mod dialogs;
mod files;
mod git;
mod highlight;
mod panel;
mod releases;
mod sections;
mod test;
mod watch;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui_kit::base::input::{EditorState, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, DragMoveEvent, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, Window, div, px,
};
use riven_build::author::{AuthorError, CheckReport};
use riven_build::workspace::{OVERRIDE_SIDES, OVERRIDES, PROJECT_FILE, TreeEntry, Workspace};
use riven_format::{PackPath, Project};
use riven_resolve::Plan;
use rust_i18n::t;

use super::app::{SIDEBAR_WIDTH, nav_row, placeholder};
use super::runtime;
use super::state::{AppState, Route};
use super::theme::ActiveTheme as _;
use super::ui::{
    ActionMenu, Button, ButtonSize, IconName, MenuEntry, caption, h_flex, icon, motion, v_flex,
};

const RECENT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Content,
    Dependencies,
    Groups,
    Releases,
    Git,
}

const SECTIONS: [Section; 5] = [
    Section::Content,
    Section::Dependencies,
    Section::Groups,
    Section::Releases,
    Section::Git,
];

impl Section {
    fn label(self) -> SharedString {
        match self {
            Section::Content => t!("dev.content"),
            Section::Dependencies => t!("dev.dependencies"),
            Section::Groups => t!("dev.groups"),
            Section::Releases => t!("dev.releases"),
            Section::Git => "Git".into(),
        }
        .into()
    }
}

/// An open tab: a section of the project or one of its files.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tab {
    Section(Section),
    File(PackPath),
}

impl Tab {
    fn label(&self) -> SharedString {
        match self {
            Tab::Section(section) => section.label(),
            Tab::File(path) => path.file_name().to_owned().into(),
        }
    }

    fn key(&self) -> String {
        match self {
            Tab::Section(section) => format!("{section:?}"),
            Tab::File(path) => path.as_str().to_owned(),
        }
    }
}

struct OpenFile {
    editor: Entity<EditorState>,
    /// The text on disk, to tell unsaved changes.
    saved: String,
    dirty: bool,
    _change: Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Check,
    Git,
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

/// The developer section: one pack project, its content and what `riven check` says about it.
pub struct DevView {
    project: Option<Workspace>,
    error: Option<SharedString>,
    /// The outcome of the last action that is not a failure.
    notice: Option<SharedString>,
    /// A long operation in progress, shown in the top bar.
    busy: Option<SharedString>,
    tabs: Vec<Tab>,
    active: Tab,
    /// The `overrides/` tree and the folders shown open in it.
    tree: Vec<TreeEntry>,
    expanded: HashSet<String>,
    files: HashMap<PackPath, OpenFile>,
    /// The entry the Dependencies section explains.
    explained: Option<String>,
    panel: Panel,
    check: Check,
    check_serial: u64,
    filter: Entity<InputState>,
    /// Indexes into the project's content, filtered and sorted by name.
    shown: Vec<usize>,
    scroll: UniformListScrollHandle,
    updates: Updates,
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
    _subs: Vec<Subscription>,
}

fn display_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

impl DevView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("dev.filter").to_string()));
        let subs = vec![
            cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.refresh_rows(cx);
                    cx.notify();
                }
            }),
            cx.observe(&AppState::global(cx), |_, _, cx| cx.notify()),
        ];
        let releases = releases::Releases::new(window, cx);
        let git = git::GitState::new(window, cx);
        let mut view = Self {
            project: None,
            error: None,
            notice: None,
            busy: None,
            tabs: vec![Tab::Section(Section::Content)],
            active: Tab::Section(Section::Content),
            tree: Vec::new(),
            expanded: HashSet::new(),
            files: HashMap::new(),
            explained: None,
            panel: Panel::Check,
            check: Check::Idle,
            check_serial: 0,
            filter,
            shown: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            updates: Updates::Unchecked,
            releases,
            git,
            panel_list: UniformListScrollHandle::new(),
            panel_seen: 0,
            panel_drag: None,
            watch: None,
            reload_files: false,
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
            self.files.clear();
            self.tabs = vec![Tab::Section(Section::Content)];
            self.active = Tab::Section(Section::Content);
            self.explained = None;
        }
        self.expanded = OVERRIDE_SIDES
            .iter()
            .map(|side| format!("{OVERRIDES}/{side}"))
            .collect();
        self.refresh_tree();
        self.refresh_dist();
        self.refresh_rows(cx);
        self.run_check(cx);
        self.git.repo = git::Repo::Unknown;
        self.refresh_git(cx);
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

    fn refresh_rows(&mut self, cx: &App) {
        let needle = self.filter.read(cx).value().trim().to_lowercase();
        let Some(ws) = &self.project else {
            self.shown.clear();
            return;
        };
        let content = &ws.project.content;
        self.shown = (0..content.len())
            .filter(|&i| {
                let e = &content[i];
                needle.is_empty()
                    || e.name.to_lowercase().contains(&needle)
                    || e.id.contains(&needle)
                    || e.file.path.file_name().to_lowercase().contains(&needle)
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
                self.updates = Updates::Unchecked;
                self.releases.changes = releases::Changes::Stale;
                self.refresh_rows(cx);
                self.run_check(cx);
                self.refresh_git(cx);
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
        cx.notify();
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
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, section: Section, cx: &mut Context<Self>) {
        self.show(Tab::Section(section), cx);
    }

    fn show(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if !self.tabs.contains(&tab) {
            self.tabs.push(tab.clone());
        }
        self.active = tab;
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
                .unwrap_or(Tab::Section(Section::Content));
            if self.tabs.is_empty() {
                self.tabs.push(self.active.clone());
            }
        }
        cx.notify();
    }

    fn render_nav(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let count = self.project.as_ref().map(|ws| ws.project.content.len());
        let rows: Vec<AnyElement> = SECTIONS
            .iter()
            .map(|&section| {
                let label = match (section, count) {
                    (Section::Content, Some(n)) => format!("{} · {n}", section.label()),
                    _ => section.label().to_string(),
                };
                nav_row(
                    SharedString::from(format!("dev-{section:?}")),
                    self.active == Tab::Section(section),
                    window,
                    cx,
                )
                .child(div().truncate().child(label))
                .when(self.project.is_some(), |row| {
                    row.on_click(cx.listener(move |this, _, _, cx| this.select(section, cx)))
                })
                .when(self.project.is_none(), |row| row.opacity(0.5))
                .into_any_element()
            })
            .collect();
        let selected = self.project.as_ref().and_then(|_| {
            SECTIONS
                .iter()
                .position(|s| Tab::Section(*s) == self.active)
        });
        let list = motion::highlighted(
            "dev-sections",
            v_flex().gap(px(2.)),
            rows,
            selected,
            |d| d.rounded(px(6.)).bg(c.sel),
            window,
            cx,
        );
        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .bg(c.panel)
            .border_r_1()
            .border_color(c.border)
            .px(px(8.))
            .py(px(12.))
            .gap(px(2.))
            .child(caption(t!("dev.sections"), cx))
            .child(div().flex_none().child(list))
            .child(self.render_files(window, cx))
            .child(
                nav_row("dev-open-settings", false, window, cx)
                    .child(icon(IconName::Settings, c.text2))
                    .child(t!("sidebar.settings").to_string())
                    .on_click(|_, _, cx| {
                        AppState::global(cx).update(cx, |s, cx| s.navigate(Route::Settings, cx))
                    }),
            )
    }

    /// The project menu: recent projects, opening a folder, creating a project.
    fn project_menu(&self, cx: &mut Context<Self>) -> ActionMenu {
        let name: SharedString = self
            .project
            .as_ref()
            .map(|ws| ws.project.id.clone().into())
            .unwrap_or_else(|| t!("dev.no_project").into());
        let trigger = Button::new("dev-project")
            .ghost()
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(14.))
                    .child(name),
            )
            .child(icon(IconName::ChevronDown, cx.theme().colors.muted).size(px(12.)));
        let current = self.project.as_ref().map(|ws| ws.dir.clone());
        let view = cx.entity().downgrade();
        let mut entries = Vec::new();
        let recent = AppState::global(cx)
            .read(cx)
            .settings
            .recent_projects
            .clone();
        if !recent.is_empty() {
            entries.push(MenuEntry::Caption(t!("dev.recent").into()));
        }
        for path in recent {
            let dir = PathBuf::from(&path);
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let open = current.as_ref() == Some(&dir);
            let view = view.clone();
            entries.push(
                MenuEntry::action(format!("{name}  {}", display_path(&dir)), move |_, cx| {
                    let dir = dir.clone();
                    let _ = view.update(cx, |this, cx| this.open(dir, cx));
                })
                .checked(open),
            );
        }
        entries.push(MenuEntry::Caption(t!("dev.new").into()));
        let browse = view.clone();
        entries.push(
            MenuEntry::action(t!("dev.open"), move |window, cx| {
                let _ = browse.update(cx, |this, cx| this.browse(window, cx));
            })
            .icon(IconName::Folder),
        );
        let create = view.clone();
        entries.push(
            MenuEntry::action(t!("dev.create"), move |window, cx| {
                dialogs::open_create(create.clone(), window, cx)
            })
            .icon(IconName::Plus),
        );
        for (origin, label, icon) in [
            (
                dialogs::Origin::Mrpack,
                t!("dev.from_mrpack"),
                IconName::Package,
            ),
            (
                dialogs::Origin::Packwiz,
                t!("dev.from_packwiz"),
                IconName::Package,
            ),
            (
                dialogs::Origin::Clone,
                t!("dev.from_clone"),
                IconName::Terminal,
            ),
        ] {
            let view = view.clone();
            let entry = MenuEntry::action(label, move |window, cx| {
                dialogs::open_new_from(view.clone(), origin, window, cx)
            })
            .icon(icon);
            entries.push(if origin == dialogs::Origin::Clone {
                entry.disabled(!self.git.installed)
            } else {
                entry
            });
        }
        ActionMenu::new("dev-project-menu", trigger, entries)
            .width(px(420.))
            .left()
    }

    fn render_top(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let meta = self.project.as_ref().map(|ws| {
            let p = &ws.project;
            format!(
                "{} · Minecraft {} · {} {}",
                p.version,
                p.minecraft,
                super::launch_bar::loader_display(p.loader.kind),
                p.loader.version
            )
        });
        h_flex()
            .flex_none()
            .h(px(46.))
            .gap(px(12.))
            .px(px(10.))
            .border_b_1()
            .border_color(c.border)
            .child(self.project_menu(cx))
            .children(meta.map(|m| {
                div()
                    .flex_none()
                    .font_family(cx.theme().mono.clone())
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .child(m)
            }))
            .child(div().flex_1())
            .when_some(self.busy.clone(), |row, text| {
                row.child(
                    h_flex()
                        .gap(px(8.))
                        .text_color(c.muted)
                        .child(motion::spinner(
                            "dev-busy",
                            icon(IconName::Loader, c.muted).size(px(14.)),
                            cx,
                        ))
                        .child(text),
                )
            })
            .child(self.render_test_controls(cx))
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let closable = self.tabs.len() > 1;
        h_flex()
            .id("dev-tabs")
            .flex_none()
            .overflow_x_scroll()
            .bg(c.panel)
            .border_b_1()
            .border_color(c.border)
            .children(self.tabs.iter().map(|tab| {
                let on = *tab == self.active;
                let dirty = match tab {
                    Tab::File(path) => self.files.get(path).is_some_and(|f| f.dirty),
                    Tab::Section(_) => false,
                };
                let key = tab.key();
                let (show, close) = (tab.clone(), tab.clone());
                h_flex()
                    .id(SharedString::from(format!("dev-tab-{key}")))
                    .flex_none()
                    .h(px(34.))
                    .pl(px(14.))
                    .pr(px(if closable { 6. } else { 14. }))
                    .gap(px(6.))
                    .border_r_1()
                    .border_color(c.border)
                    .cursor_pointer()
                    .map(|t| {
                        if on {
                            t.bg(c.bg)
                                .text_color(c.text)
                                .font_weight(FontWeight::SEMIBOLD)
                        } else {
                            t.text_color(c.muted)
                        }
                    })
                    .when_some(
                        match tab {
                            Tab::File(path) => Some(path.as_str().to_owned()),
                            Tab::Section(_) => None,
                        },
                        |t, path| t.tooltip(super::ui::tooltip(path.into())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.show(show.clone(), cx)))
                    .child(tab.label())
                    .when(dirty, |t| t.child(div().text_color(c.accent).child("●")))
                    .when(closable, |t| {
                        t.child(
                            Button::new(SharedString::from(format!("dev-close-{key}")))
                                .ghost()
                                .size(ButtonSize::Xs)
                                .icon(IconName::Close)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_tab(close.clone(), cx)
                                })),
                        )
                    })
            }))
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
}

impl Render for DevView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_editor(window, cx);
        self.settle_git(window, cx);
        self.flush_messages(cx);
        self.reload_open_files(window, cx);
        self.follow_stream(cx);
        if self.active == Tab::Section(Section::Releases) {
            self.ensure_changes(cx);
        }
        let main: AnyElement = if self.project.is_none() {
            self.render_empty(cx)
        } else {
            let body: AnyElement = match self.active.clone() {
                Tab::Section(Section::Content) => self.render_content(cx),
                Tab::Section(Section::Groups) => self.render_groups(cx),
                Tab::Section(Section::Dependencies) => self.render_dependencies(cx),
                Tab::Section(Section::Releases) => self.render_releases(cx),
                Tab::Section(Section::Git) => self.render_git(cx),
                Tab::File(path) => self.render_file(&path, cx),
            };
            let tab = SharedString::from(format!("dev-section-{}", self.active.key()));
            v_flex()
                .size_full()
                .child(self.render_top(cx))
                .child(self.render_tabs(cx))
                .child(motion::enter(
                    tab,
                    v_flex().flex_1().min_h_0().child(body),
                    8.,
                    window,
                    cx,
                ))
                .child(self.render_panel(cx))
                .into_any_element()
        };
        h_flex()
            .id("dev-view")
            .size_full()
            .items_start()
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<panel::PanelResize>, _, cx| {
                    this.drag_panel(event, cx)
                }),
            )
            .on_drop(cx.listener(|this, _: &panel::PanelResize, _, cx| this.drop_panel(cx)))
            .child(self.render_nav(window, cx))
            .child(div().flex_1().min_w_0().h_full().child(main))
    }
}
