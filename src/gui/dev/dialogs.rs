use std::path::PathBuf;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, div, px, relative,
};
use riven_build::author::{self, AddRequest, AuthorError};
use riven_build::import::{ImportKind, ImportStage, Scope, import_pack};
use riven_build::pull::Drift;
use riven_build::workspace::Workspace;
use riven_format::{LoaderKind, Reason, Side, SourceKind};
use riven_resolve::Plan;
use rust_i18n::t;

use super::DevView;
use super::git::GitOp;
use crate::gui::dialogs::{dialog_shell, field, open};
use crate::gui::runtime;
use crate::gui::state::AppState;
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{Button, ButtonSize, Dropdown, IconName, MenuItem, TextField, h_flex, v_flex};

fn close(cx: &mut App) {
    AppState::global(cx).update(cx, |s, cx| s.close_modal(cx));
}

fn cancel() -> Button {
    Button::new("cancel")
        .outline()
        .size(ButtonSize::Md)
        .label(t!("common.cancel"))
        .on_click(|_, _, cx| close(cx))
}

/// The lines of a plan the way `riven` prints them: `+` added, `~` updated, `-` removed.
fn plan_view(plan: &Plan, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let c = theme.colors;
    let line = |mark: &'static str, color, text: String, note: Option<String>| {
        h_flex()
            .items_start()
            .gap(px(8.))
            .child(div().flex_none().text_color(color).child(mark))
            .child(div().flex_1().min_w_0().child(text))
            .children(note.map(|n| div().flex_none().text_color(c.muted).child(n)))
    };
    let dependency = t!("dev.dependency").to_string();
    let mut lines: Vec<AnyElement> = Vec::new();
    for e in &plan.add {
        let note = (e.reason == Reason::Dependency).then(|| dependency.clone());
        let text = format!("{} {}", e.id, e.file.path.file_name());
        lines.push(line("+", c.ok, text, note).into_any_element());
    }
    for (old, new) in &plan.update {
        let text = format!(
            "{} {} → {}",
            new.id,
            old.file.path.file_name(),
            new.file.path.file_name()
        );
        lines.push(line("~", c.warn, text, None).into_any_element());
    }
    for e in &plan.remove {
        let text = format!("{} {}", e.id, e.file.path.file_name());
        lines.push(line("-", crate::gui::theme::danger(), text, None).into_any_element());
    }
    for p in &plan.problems {
        lines.push(line("!", c.warn, p.to_string(), None).into_any_element());
    }
    for n in &plan.notes {
        lines.push(line("·", c.muted, n.clone(), None).into_any_element());
    }
    v_flex()
        .id("plan-lines")
        .max_h(px(320.))
        .overflow_y_scroll()
        .gap(px(4.))
        .p(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(c.border)
        .bg(c.bg)
        .font_family(theme.mono.clone())
        .text_size(px(12.))
        .text_color(c.text2)
        .children(lines)
        .into_any_element()
}

/// Shows a planned change of `riven.json` and applies it on confirmation.
pub struct PlanPreview {
    title: SharedString,
    plan: Plan,
    view: WeakEntity<DevView>,
}

impl Render for PlanPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let changes = !self.plan.is_empty();
        let (plan, view) = (self.plan.clone(), self.view.clone());
        let apply = Button::new("apply")
            .primary()
            .size(ButtonSize::Md)
            .label(t!("dev.apply"))
            .disabled(!changes)
            .on_click(move |_, _, cx| {
                let _ = view.update(cx, |this, cx| this.apply_plan(&plan, cx));
                close(cx);
            });
        dialog_shell(
            self.title.clone(),
            t!("dev.preview_hint"),
            plan_view(&self.plan, cx),
            vec![cancel(), apply],
            cx,
        )
    }
}

pub fn open_preview(
    title: SharedString,
    plan: Plan,
    view: WeakEntity<DevView>,
    _: &mut Window,
    cx: &mut App,
) {
    let preview = cx.new(|_| PlanPreview { title, plan, view });
    open(preview.into(), 560., cx);
}

const SOURCES: [Option<SourceKind>; 4] = [
    None,
    Some(SourceKind::Modrinth),
    Some(SourceKind::GitHub),
    Some(SourceKind::Url),
];

const SIDES: [Option<Side>; 4] = [
    None,
    Some(Side::Client),
    Some(Side::Server),
    Some(Side::Both),
];

fn source_label(source: Option<SourceKind>) -> SharedString {
    match source {
        None => t!("dev.source_auto").into(),
        Some(kind) => author::source_name(kind).into(),
    }
}

fn side_choice(side: Option<Side>) -> SharedString {
    match side {
        None => t!("dev.side_default").into(),
        Some(Side::Client) => t!("dev.side_client").into(),
        Some(Side::Server) => t!("dev.side_server").into(),
        Some(Side::Both) => t!("dev.side_both").into(),
    }
}

/// "Add": a query like `riven add` takes, resolved into a plan before anything is written.
pub struct AddContent {
    view: WeakEntity<DevView>,
    workspace: Option<Workspace>,
    query: Entity<InputState>,
    source: usize,
    side: usize,
    group: Option<String>,
    plan: Option<Plan>,
    busy: bool,
    error: Option<SharedString>,
    /// `(slug, title)` choices when the query matched several projects.
    choices: Vec<(String, String)>,
    _query: Subscription,
}

impl AddContent {
    fn new(view: WeakEntity<DevView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = view.upgrade().and_then(|v| v.read(cx).project.clone());
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("dev.add_hint").to_string()));
        query.update(cx, |s, cx| s.focus(window, cx));
        let _query = cx.subscribe(&query, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.plan = None;
                this.error = None;
                this.choices.clear();
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.resolve(cx),
            _ => {}
        });
        Self {
            view,
            workspace,
            query,
            source: 0,
            side: 0,
            group: None,
            plan: None,
            busy: false,
            error: None,
            choices: Vec::new(),
            _query,
        }
    }

    fn request(&self, query: String) -> AddRequest {
        AddRequest {
            query,
            source: SOURCES[self.source],
            asset: None,
            side: SIDES[self.side],
            group: self.group.clone(),
            pin: false,
        }
    }

    fn resolve(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().trim().to_string();
        self.resolve_query(query, cx);
    }

    fn resolve_query(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(ws) = self.workspace.clone() else {
            return;
        };
        if query.is_empty() || self.busy {
            return;
        }
        let request = self.request(query);
        self.busy = true;
        self.error = None;
        self.choices.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::spawn(async move { author::plan_add(&ws, &request).await })
                .await
                .unwrap_or_else(|_| Err(AuthorError::NoDataDir));
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(plan) => this.plan = Some(plan),
                    Err(AuthorError::Ambiguous { options, .. }) => {
                        this.error = Some(t!("dev.ambiguous").into());
                        this.choices = options;
                    }
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A jar or zip from disk; files outside the project are copied into `local/`.
    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("add_mods.pick").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let text = path.display().to_string();
                this.query
                    .update(cx, |s, cx| s.set_value(text.clone(), window, cx));
                this.resolve_query(text, cx);
            });
        })
        .detach();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let Some(plan) = self.plan.take() else {
            return self.resolve(cx);
        };
        let _ = self.view.update(cx, |this, cx| this.apply_plan(&plan, cx));
        close(cx);
    }
}

impl Render for AddContent {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let view = cx.entity().downgrade();
        let pick_source = view.clone();
        let pick_side = view.clone();
        let pick_group = view.clone();
        let sources = SOURCES
            .iter()
            .enumerate()
            .map(|(i, s)| MenuItem::new(i.to_string(), source_label(*s)))
            .collect();
        let sides = SIDES
            .iter()
            .enumerate()
            .map(|(i, s)| MenuItem::new(i.to_string(), side_choice(*s)))
            .collect();
        let groups: Vec<MenuItem> = self
            .workspace
            .as_ref()
            .map(|ws| ws.project.groups.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|g| MenuItem::new(g.id, g.name))
            .collect();
        let has_groups = !groups.is_empty();
        let groups: Vec<MenuItem> = std::iter::once(MenuItem::new("", t!("dev.no_group")))
            .chain(groups)
            .collect();
        let options = h_flex()
            .gap(px(10.))
            .child(field(
                t!("dev.source"),
                Dropdown::new(
                    "add-source",
                    sources,
                    Some(self.source.to_string().into()),
                    move |v, _, cx| {
                        let _ = pick_source.update(cx, |this, cx| {
                            this.source = v.parse().unwrap_or(0);
                            this.plan = None;
                            cx.notify();
                        });
                    },
                )
                .width(px(150.)),
                cx,
            ))
            .child(field(
                t!("dev.side"),
                Dropdown::new(
                    "add-side",
                    sides,
                    Some(self.side.to_string().into()),
                    move |v, _, cx| {
                        let _ = pick_side.update(cx, |this, cx| {
                            this.side = v.parse().unwrap_or(0);
                            this.plan = None;
                            cx.notify();
                        });
                    },
                )
                .width(px(150.)),
                cx,
            ))
            .when(has_groups, |row| {
                row.child(field(
                    t!("dev.group"),
                    Dropdown::new(
                        "add-group",
                        groups,
                        Some(self.group.clone().unwrap_or_default().into()),
                        move |v, _, cx| {
                            let _ = pick_group.update(cx, |this, cx| {
                                this.group = (!v.is_empty()).then(|| v.to_string());
                                this.plan = None;
                                cx.notify();
                            });
                        },
                    )
                    .width(px(150.)),
                    cx,
                ))
            });
        let choices = self.choices.iter().enumerate().map(|(i, (slug, title))| {
            let slug = slug.clone();
            Button::new(("choice", i))
                .label(format!("{title} ({slug})"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    let slug = slug.clone();
                    this.query
                        .update(cx, |s, cx| s.set_value(slug.clone(), window, cx));
                    this.resolve_query(slug, cx);
                }))
        });
        let body = v_flex()
            .gap(px(14.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(div().flex_1().child(TextField::new(&self.query)))
                    .child(
                        Button::new("add-file")
                            .icon(IconName::Folder)
                            .tooltip(t!("add_mods.from_file"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_file(window, cx)),
                            ),
                    ),
            )
            .child(options)
            .when_some(self.error.clone(), |col, e| {
                col.child(div().text_color(c.warn).line_height(relative(1.5)).child(e))
            })
            .when(!self.choices.is_empty(), |col| {
                col.child(h_flex().flex_wrap().gap(px(6.)).children(choices))
            })
            .when_some(self.plan.as_ref(), |col, plan| {
                col.child(plan_view(plan, cx))
            });
        let label = match (&self.plan, self.busy) {
            (_, true) => t!("dev.resolving"),
            (Some(plan), false) => t!("dev.add_n", n = plan.add.len()),
            (None, false) => t!("dev.resolve"),
        };
        let action = Button::new("add-apply")
            .primary()
            .size(ButtonSize::Md)
            .label(label)
            .disabled(self.busy)
            .on_click(cx.listener(|this, _, _, cx| this.apply(cx)));
        dialog_shell(
            t!("dev.add_title"),
            t!("dev.add_description"),
            body,
            vec![cancel(), action],
            cx,
        )
    }
}

pub fn open_add(view: WeakEntity<DevView>, window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| AddContent::new(view, window, cx));
    open(dialog.into(), 560., cx);
}

const LOADERS: [LoaderKind; 4] = [
    LoaderKind::NeoForge,
    LoaderKind::Fabric,
    LoaderKind::Quilt,
    LoaderKind::Forge,
];

/// "Create project": `riven init` in a chosen folder.
pub struct CreateProject {
    view: WeakEntity<DevView>,
    dir: Option<PathBuf>,
    search: Entity<InputState>,
    releases: Vec<SharedString>,
    minecraft: Option<SharedString>,
    loader: usize,
    busy: bool,
    error: Option<SharedString>,
}

impl CreateProject {
    fn new(view: WeakEntity<DevView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("new_instance.find_version").to_string())
        });
        cx.spawn(async move |this, cx| {
            let releases = runtime::spawn(async {
                crate::gui::dialogs::game_meta().minecraft_releases().await
            })
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.releases = releases.into_iter().map(Into::into).collect();
                this.minecraft = this.releases.first().cloned();
                cx.notify();
            });
        })
        .detach();
        Self {
            view,
            dir: None,
            search,
            releases: Vec::new(),
            minecraft: None,
            loader: 0,
            busy: false,
            error: None,
        }
    }

    fn pick_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("dev.pick_folder").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.dir = paths.into_iter().next();
                this.error = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.dir.clone() else {
            self.error = Some(t!("dev.pick_folder_first").into());
            cx.notify();
            return;
        };
        let minecraft = self.minecraft.as_ref().map(ToString::to_string);
        let loader = LOADERS[self.loader];
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let created = runtime::spawn(async move {
                riven_build::workspace::init(&dir, minecraft, loader, None).await
            })
            .await
            .unwrap_or_else(|_| Err(AuthorError::NoDataDir));
            let _ = this.update(cx, |this, cx| match created {
                Ok(ws) => {
                    let _ = this.view.update(cx, |view, cx| view.adopt(ws, cx));
                    close(cx);
                }
                Err(e) => {
                    this.busy = false;
                    this.error = Some(e.to_string().into());
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Render for CreateProject {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let view = cx.entity().downgrade();
        let pick_loader = view.clone();
        let versions = self
            .releases
            .iter()
            .map(|r| MenuItem::new(r.clone(), r.clone()))
            .collect();
        let loaders = LOADERS
            .iter()
            .enumerate()
            .map(|(i, l)| MenuItem::new(i.to_string(), crate::gui::launch_bar::loader_display(*l)))
            .collect();
        let width = px(432.);
        let folder = self
            .dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| t!("dev.no_folder").to_string());
        let body = v_flex()
            .gap(px(14.))
            .child(field(
                t!("dev.folder"),
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if self.dir.is_some() { c.text } else { c.muted })
                            .child(folder),
                    )
                    .child(
                        Button::new("pick-folder")
                            .icon(IconName::Folder)
                            .label(t!("dev.choose"))
                            .on_click(cx.listener(|this, _, window, cx| this.pick_dir(window, cx))),
                    ),
                cx,
            ))
            .child(field(
                t!("new_instance.minecraft"),
                Dropdown::new(
                    "create-minecraft",
                    versions,
                    self.minecraft.clone(),
                    move |v, _, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.minecraft = Some(v);
                            cx.notify();
                        });
                    },
                )
                .width(width)
                .placeholder(t!("new_instance.loading").to_string())
                .searchable(&self.search),
                cx,
            ))
            .child(field(
                t!("new_instance.loader"),
                Dropdown::new(
                    "create-loader",
                    loaders,
                    Some(self.loader.to_string().into()),
                    move |v, _, cx| {
                        let _ = pick_loader.update(cx, |this, cx| {
                            this.loader = v.parse().unwrap_or(0);
                            cx.notify();
                        });
                    },
                )
                .width(width),
                cx,
            ))
            .when_some(self.error.clone(), |col, e| {
                col.child(div().text_color(c.warn).child(e))
            });
        let action = Button::new("create-project")
            .primary()
            .size(ButtonSize::Md)
            .disabled(self.busy)
            .label(if self.busy {
                t!("new_instance.creating")
            } else {
                t!("new_instance.create")
            })
            .on_click(cx.listener(|this, _, _, cx| this.create(cx)));
        dialog_shell(
            t!("dev.create"),
            t!("dev.create_description"),
            body,
            vec![cancel(), action],
            cx,
        )
    }
}

pub fn open_create(view: WeakEntity<DevView>, window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| CreateProject::new(view, window, cx));
    open(dialog.into(), 472., cx);
}

/// "New file" / "New folder": a path inside `overrides/`, prefilled with the folder it starts in.
pub struct NewPath {
    view: WeakEntity<DevView>,
    folder: bool,
    input: Entity<InputState>,
    error: Option<SharedString>,
    _input: Subscription,
}

impl NewPath {
    fn create(&mut self, cx: &mut Context<Self>) {
        let path = self.input.read(cx).value().trim().to_string();
        let folder = self.folder;
        let result = self
            .view
            .update(cx, |view, cx| view.create_path(&path, folder, cx))
            .unwrap_or(Ok(()));
        match result {
            Ok(()) => close(cx),
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
            }
        }
    }
}

impl Render for NewPath {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let body = v_flex()
            .gap(px(12.))
            .child(TextField::new(&self.input))
            .when_some(self.error.clone(), |col, e| {
                col.child(div().text_color(c.warn).child(e))
            });
        let action = Button::new("new-path-create")
            .primary()
            .size(ButtonSize::Md)
            .label(t!("new_instance.create"))
            .on_click(cx.listener(|this, _, _, cx| this.create(cx)));
        let title = if self.folder {
            t!("dev.new_folder")
        } else {
            t!("dev.new_file")
        };
        dialog_shell(
            title,
            t!("dev.new_path_hint"),
            body,
            vec![cancel(), action],
            cx,
        )
    }
}

pub fn open_new_path(
    view: WeakEntity<DevView>,
    start: String,
    folder: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let dialog = cx.new(|cx| {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(start));
        input.update(cx, |s, cx| s.focus(window, cx));
        let _input =
            cx.subscribe(
                &input,
                |this: &mut NewPath, _, event: &InputEvent, cx| match event {
                    InputEvent::PressEnter { .. } => this.create(cx),
                    InputEvent::Change => {
                        this.error = None;
                        cx.notify();
                    }
                    _ => {}
                },
            );
        NewPath {
            view,
            folder,
            input,
            error: None,
            _input,
        }
    });
    open(dialog.into(), 480., cx);
}

/// Creating a group, or changing an existing one's name, description and default.
pub struct GroupForm {
    view: WeakEntity<DevView>,
    existing: Option<String>,
    id: Entity<InputState>,
    name: Entity<InputState>,
    description: Entity<InputState>,
    default: bool,
    error: Option<SharedString>,
}

fn valid_group_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

impl GroupForm {
    fn submit(&mut self, cx: &mut Context<Self>) {
        let id = self.id.read(cx).value().trim().to_string();
        let name = self.name.read(cx).value().trim().to_string();
        let description = self.description.read(cx).value().trim().to_string();
        if !valid_group_id(&id) {
            self.error = Some(t!("dev.group_bad_id").into());
            cx.notify();
            return;
        }
        let group = riven_format::Group {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            description: (!description.is_empty()).then_some(description),
            default: self.default,
        };
        let existing = self.existing.is_some();
        let failed = self
            .view
            .update(cx, |view, cx| {
                view.edit(
                    |p| {
                        if existing {
                            author::update_group(p, group)
                        } else {
                            author::add_group(p, group)
                        }
                    },
                    cx,
                );
                view.error.take()
            })
            .ok()
            .flatten();
        match failed {
            Some(e) => {
                self.error = Some(e);
                cx.notify();
            }
            None => close(cx),
        }
    }
}

impl Render for GroupForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let view = cx.entity().downgrade();
        let body = v_flex()
            .gap(px(14.))
            .when(self.existing.is_none(), |col| {
                col.child(field(t!("dev.group_id"), TextField::new(&self.id), cx))
            })
            .child(field(t!("dev.group_name"), TextField::new(&self.name), cx))
            .child(field(
                t!("dev.group_description"),
                TextField::new(&self.description),
                cx,
            ))
            .child(
                crate::gui::ui::Switch::new("group-form-default", self.default)
                    .label(t!("dev.group_default").to_string())
                    .on_change(move |on, _, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.default = on;
                            cx.notify();
                        });
                    }),
            )
            .when_some(self.error.clone(), |col, e| {
                col.child(div().text_color(c.warn).child(e))
            });
        let action = Button::new("group-save")
            .primary()
            .size(ButtonSize::Md)
            .label(if self.existing.is_some() {
                t!("dev.save")
            } else {
                t!("dev.group_add")
            })
            .on_click(cx.listener(|this, _, _, cx| this.submit(cx)));
        let title = if self.existing.is_some() {
            t!("dev.group_edit")
        } else {
            t!("dev.group_add")
        };
        dialog_shell(
            title,
            t!("dev.groups_hint"),
            body,
            vec![cancel(), action],
            cx,
        )
    }
}

pub fn open_group(
    view: WeakEntity<DevView>,
    group: Option<riven_format::Group>,
    window: &mut Window,
    cx: &mut App,
) {
    let dialog = cx.new(|cx| {
        let values = [
            group.as_ref().map(|g| g.id.clone()).unwrap_or_default(),
            group.as_ref().map(|g| g.name.clone()).unwrap_or_default(),
            group
                .as_ref()
                .and_then(|g| g.description.clone())
                .unwrap_or_default(),
        ];
        let [id, name, description] =
            values.map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value)));
        let first = if group.is_some() { &name } else { &id };
        first.update(cx, |s, cx| s.focus(window, cx));
        GroupForm {
            view,
            existing: group.as_ref().map(|g| g.id.clone()),
            default: group.as_ref().is_some_and(|g| g.default),
            id,
            name,
            description,
            error: None,
        }
    });
    open(dialog.into(), 460., cx);
}

/// "Take configs": files the test instance changed or created, copied back into `overrides/`.
pub struct PullConfigs {
    view: WeakEntity<DevView>,
    workspace: Option<Workspace>,
    game_dir: Option<PathBuf>,
    found: Option<Result<Vec<Drift>, SharedString>>,
    picked: Vec<bool>,
    /// Where new files go: `common` or `client`.
    new_scope: Scope,
}

impl PullConfigs {
    fn new(view: WeakEntity<DevView>, cx: &mut Context<Self>) -> Self {
        let workspace = view.upgrade().and_then(|v| v.read(cx).project.clone());
        let game_dir = view
            .upgrade()
            .and_then(|v| v.read(cx).test_instance())
            .and_then(|id| {
                let state = AppState::global(cx);
                let store = state.read(cx).store.clone()?;
                Some(store.game_dir(&id))
            });
        if let (Some(ws), Some(dir)) = (workspace.clone(), game_dir.clone()) {
            cx.spawn(async move |this, cx| {
                let found = runtime::blocking(move || riven_build::pull::drift(&ws, &dir))
                    .await
                    .unwrap_or_else(|_| Err(AuthorError::NoDataDir));
                let _ = this.update(cx, |this, cx| {
                    match found {
                        Ok(found) => {
                            this.picked = found.iter().map(|d| !d.new).collect();
                            this.found = Some(Ok(found));
                        }
                        Err(e) => this.found = Some(Err(e.to_string().into())),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        Self {
            view,
            workspace,
            game_dir,
            found: None,
            picked: Vec::new(),
            new_scope: Scope::Common,
        }
    }

    fn take(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(dir), Some(Ok(found))) = (&self.workspace, &self.game_dir, &self.found)
        else {
            return;
        };
        let picked: Vec<Drift> = found
            .iter()
            .zip(&self.picked)
            .filter(|(_, on)| **on)
            .map(|(d, _)| Drift {
                scope: if d.new && d.scope == Scope::Common {
                    self.new_scope
                } else {
                    d.scope
                },
                ..d.clone()
            })
            .collect();
        match riven_build::pull::take(ws, dir, &picked) {
            Ok(n) => {
                let _ = self.view.update(cx, |view, cx| {
                    view.refresh_tree(cx);
                    view.refresh_git(cx);
                    view.notice = Some(t!("dev.pulled", n = n).into());
                    cx.notify();
                });
                close(cx);
            }
            Err(e) => {
                self.found = Some(Err(e.to_string().into()));
                cx.notify();
            }
        }
    }
}

impl Render for PullConfigs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let c = theme.colors;
        let mono = theme.mono.clone();
        let count = self.picked.iter().filter(|p| **p).count();
        let body: AnyElement = match &self.found {
            None => div()
                .text_color(c.muted)
                .child(t!("dev.pull_reading").to_string())
                .into_any_element(),
            Some(Err(e)) => div().text_color(c.warn).child(e.clone()).into_any_element(),
            Some(Ok(found)) if found.is_empty() => div()
                .text_color(c.muted)
                .child(t!("dev.pull_none").to_string())
                .into_any_element(),
            Some(Ok(found)) => {
                let rows = found.iter().enumerate().map(|(i, d)| {
                    let on = self.picked.get(i).copied().unwrap_or(false);
                    let badge = if d.new {
                        t!("dev.pull_new").to_string()
                    } else {
                        d.scope.dir().to_owned()
                    };
                    h_flex()
                        .id(("pull-row", i))
                        .h(px(26.))
                        .px(px(8.))
                        .gap(px(10.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.row))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = this.picked.get_mut(i) {
                                *p = !*p;
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .size(px(15.))
                                .flex_none()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(if on { c.accent } else { c.border })
                                .when(on, |d| d.bg(c.accent))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(on, |d| {
                                    d.child(
                                        crate::gui::ui::icon(IconName::Check, c.on_accent)
                                            .size(px(11.)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(mono.clone())
                                .text_size(px(12.))
                                .child(d.path.to_string()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(if d.new { c.ok } else { c.warn })
                                .child(badge),
                        )
                });
                let all = found.len();
                let any_new = found.iter().any(|d| d.new);
                let view = cx.entity().downgrade();
                v_flex()
                    .gap(px(10.))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(
                                Button::new("pull-all")
                                    .size(ButtonSize::Xs)
                                    .label(t!("dev.pull_all"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.picked = vec![true; all];
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("pull-changed")
                                    .size(ButtonSize::Xs)
                                    .label(t!("dev.pull_changed"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(Ok(found)) = &this.found {
                                            this.picked = found.iter().map(|d| !d.new).collect();
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1())
                            .when(any_new, |row| {
                                row.child(
                                    div()
                                        .text_color(c.muted)
                                        .child(t!("dev.pull_new_to").to_string()),
                                )
                                .child(
                                    Dropdown::new(
                                        "pull-scope",
                                        vec![
                                            MenuItem::new("common", "common"),
                                            MenuItem::new("client", "client"),
                                        ],
                                        Some(self.new_scope.dir().into()),
                                        move |v, _, cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.new_scope = if v.as_ref() == "client" {
                                                    Scope::Client
                                                } else {
                                                    Scope::Common
                                                };
                                                cx.notify();
                                            });
                                        },
                                    )
                                    .width(px(120.)),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .id("pull-list")
                            .max_h(px(360.))
                            .overflow_y_scroll()
                            .gap(px(1.))
                            .children(rows),
                    )
                    .into_any_element()
            }
        };
        let action = Button::new("pull-take")
            .primary()
            .size(ButtonSize::Md)
            .label(t!("dev.pull_take", n = count))
            .disabled(count == 0)
            .on_click(cx.listener(|this, _, _, cx| this.take(cx)));
        dialog_shell(
            t!("dev.pull"),
            t!("dev.pull_description"),
            body,
            vec![cancel(), action],
            cx,
        )
    }
}

pub fn open_pull(view: WeakEntity<DevView>, _: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| PullConfigs::new(view, cx));
    open(dialog.into(), 620., cx);
}

/// Where a new project comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Mrpack,
    Packwiz,
    Clone,
}

/// "Import mrpack/packwiz" and "Clone with git": a source and the folder it becomes.
pub struct NewFrom {
    view: WeakEntity<DevView>,
    origin: Origin,
    source: Entity<InputState>,
    name: Entity<InputState>,
    /// The name was typed by hand, so the source no longer suggests one.
    named: bool,
    parent: Option<PathBuf>,
    stage: Option<SharedString>,
    error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

fn suggested_name(origin: Origin, source: &str) -> Option<String> {
    let source = source.trim().trim_end_matches('/');
    match origin {
        Origin::Clone => riven_build::git::clone_name(source),
        Origin::Mrpack => {
            let file = source.rsplit(['/', '\\']).next()?;
            Some(file.trim_end_matches(".mrpack").to_owned()).filter(|n| !n.is_empty())
        }
        Origin::Packwiz => {
            let source = source
                .trim_end_matches("pack.toml")
                .trim_end_matches(['/', '\\']);
            source
                .rsplit(['/', '\\'])
                .next()
                .map(str::to_owned)
                .filter(|n| !n.is_empty())
        }
    }
}

impl NewFrom {
    fn new(
        view: WeakEntity<DevView>,
        origin: Origin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hint = match origin {
            Origin::Mrpack => t!("dev.from_mrpack_hint"),
            Origin::Packwiz => t!("dev.from_packwiz_hint"),
            Origin::Clone => t!("dev.from_clone_hint"),
        };
        let source = cx.new(|cx| InputState::new(window, cx).placeholder(hint.to_string()));
        source.update(cx, |s, cx| s.focus(window, cx));
        let name = cx.new(|cx| InputState::new(window, cx));
        let subs = vec![
            cx.subscribe_in(
                &source,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    if let InputEvent::Change = event {
                        this.error = None;
                        if !this.named {
                            let text = input.read(cx).value().to_string();
                            let name = suggested_name(this.origin, &text).unwrap_or_default();
                            this.name.update(cx, |s, cx| s.set_value(name, window, cx));
                            this.named = false;
                        }
                        cx.notify();
                    }
                },
            ),
            cx.subscribe(&name, |this, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let typed = input.read(cx).value().to_string();
                    let suggested =
                        suggested_name(this.origin, this.source.read(cx).value().as_ref())
                            .unwrap_or_default();
                    this.named = typed != suggested;
                    this.error = None;
                    cx.notify();
                }
            }),
        ];
        let parent = view
            .upgrade()
            .and_then(|v| v.read(cx).project.as_ref()?.dir.parent().map(Into::into))
            .or_else(dirs::home_dir);
        Self {
            view,
            origin,
            source,
            name,
            named: false,
            parent,
            stage: None,
            error: None,
            _subs: subs,
        }
    }

    fn browse_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: self.origin == Origin::Mrpack,
            directories: self.origin == Origin::Packwiz,
            multiple: false,
            prompt: Some(t!("dev.choose").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let text = path.display().to_string();
                this.source
                    .update(cx, |s, cx| s.set_value(text, window, cx));
            });
        })
        .detach();
    }

    fn pick_parent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("dev.pick_parent").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.parent = paths.into_iter().next();
                cx.notify();
            });
        })
        .detach();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let source = self.source.read(cx).value().trim().to_string();
        let name = self.name.read(cx).value().trim().to_string();
        let Some(parent) = self.parent.clone() else {
            return;
        };
        if source.is_empty() || name.is_empty() || name.contains(['/', '\\']) {
            self.error = Some(t!("dev.from_incomplete").into());
            cx.notify();
            return;
        }
        let dest = parent.join(&name);
        if std::fs::read_dir(&dest).is_ok_and(|mut d| d.next().is_some()) {
            self.error = Some(t!("dev.from_not_empty", path = dest.display()).into());
            cx.notify();
            return;
        }
        let kind = match self.origin {
            Origin::Clone => {
                let _ = self.view.update(cx, |view, cx| {
                    let open = dest.clone();
                    view.run_git(
                        GitOp::Clone { url: source, dest },
                        move |view, cx| view.open(open, cx),
                        cx,
                    );
                });
                close(cx);
                return;
            }
            Origin::Mrpack => ImportKind::Mrpack,
            Origin::Packwiz => ImportKind::Packwiz,
        };
        self.stage = Some(t!("dev.import_reading").into());
        self.error = None;
        cx.notify();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ImportStage>();
        let finished = runtime::spawn(async move {
            import_pack(&dest, kind, &source, move |stage| {
                let _ = tx.send(stage);
            })
            .await
        });
        cx.spawn(async move |this, cx| {
            while let Some(stage) = rx.recv().await {
                let label: SharedString = match stage {
                    ImportStage::Reading => t!("dev.import_reading"),
                    ImportStage::Identifying { files } => t!("dev.import_identifying", n = files),
                    ImportStage::Downloading => t!("dev.import_downloading"),
                    ImportStage::Writing => t!("dev.import_writing"),
                }
                .into();
                if this
                    .update(cx, |this, cx| {
                        this.stage = Some(label);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = finished.await;
            let _ = this.update(cx, |this, cx| {
                this.stage = None;
                match result {
                    Ok(Ok(imported)) => {
                        let skipped = imported.unmatched.len();
                        let _ = this.view.update(cx, |view, cx| {
                            view.adopt(imported.workspace, cx);
                            if skipped > 0 {
                                view.error = Some(t!("dev.import_skipped", n = skipped).into());
                            }
                        });
                        close(cx);
                    }
                    Ok(Err(e)) => this.error = Some(e.to_string().into()),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for NewFrom {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let (title, description) = match self.origin {
            Origin::Mrpack => (t!("dev.from_mrpack"), t!("dev.from_mrpack_description")),
            Origin::Packwiz => (t!("dev.from_packwiz"), t!("dev.from_packwiz_description")),
            Origin::Clone => (t!("dev.from_clone"), t!("dev.from_clone_description")),
        };
        let busy = self.stage.is_some();
        let parent = self
            .parent
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| t!("dev.no_folder").to_string());
        let body = v_flex()
            .gap(px(14.))
            .child(field(
                t!("dev.from_source"),
                h_flex()
                    .gap(px(8.))
                    .child(div().flex_1().child(TextField::new(&self.source)))
                    .when(self.origin != Origin::Clone, |row| {
                        row.child(
                            Button::new("from-browse")
                                .icon(IconName::Folder)
                                .tooltip(t!("dev.choose"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.browse_source(window, cx)
                                })),
                        )
                    }),
                cx,
            ))
            .child(field(
                t!("dev.from_into"),
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c.muted)
                            .child(parent),
                    )
                    .child(
                        Button::new("from-parent")
                            .icon(IconName::Folder)
                            .label(t!("dev.choose"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_parent(window, cx)),
                            ),
                    ),
                cx,
            ))
            .child(field(t!("dev.from_name"), TextField::new(&self.name), cx))
            .when_some(self.stage.clone(), |col, stage| {
                col.child(div().text_color(c.muted).child(stage))
            })
            .when_some(self.error.clone(), |col, e| {
                col.child(div().text_color(c.warn).line_height(relative(1.5)).child(e))
            });
        let action = Button::new("from-run")
            .primary()
            .size(ButtonSize::Md)
            .disabled(busy)
            .label(match self.origin {
                Origin::Clone => t!("dev.from_clone_run"),
                _ => t!("dev.from_import_run"),
            })
            .on_click(cx.listener(|this, _, _, cx| this.run(cx)));
        let title = title.trim_end_matches('…').to_owned();
        dialog_shell(title, description, body, vec![cancel(), action], cx)
    }
}

pub fn open_new_from(view: WeakEntity<DevView>, origin: Origin, window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| NewFrom::new(view, origin, window, cx));
    open(dialog.into(), 520., cx);
}
