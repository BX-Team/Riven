use std::path::PathBuf;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, div, px, relative,
};
use riven_build::author::{self, AddRequest, AuthorError};
use riven_build::workspace::Workspace;
use riven_format::{LoaderKind, Reason, Side, SourceKind};
use riven_resolve::Plan;
use rust_i18n::t;

use super::DevView;
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
                view.error.clone()
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
