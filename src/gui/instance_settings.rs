use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};
use riven_format::{
    GameWindow, Instance, JavaChoice, LaunchCommands, LaunchSettings, Loader, LoaderKind, MemoryMb,
};
use riven_launch::instances::Instances;
use rust_i18n::t;

use super::java_picker::JavaPicker;
use super::runtime;
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{
    self, Button, Dropdown, IconName, MenuItem, Section, Switch, TextArea, TextField, W_SEMIBOLD,
    h_flex, setting_row, v_flex,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Memory,
    Java,
    JvmArgs,
    Window,
    Commands,
}

enum Input {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

impl Input {
    fn value(&self, cx: &App) -> String {
        match self {
            Input::Line(s) => s.read(cx).value().trim().to_string(),
            Input::Area(s) => s.read(cx).value().trim().to_string(),
        }
    }
}

/// One text field of an override group.
struct Field {
    group: Group,
    label: SharedString,
    input: Input,
}

const LOADERS: [Option<LoaderKind>; 5] = [
    None,
    Some(LoaderKind::NeoForge),
    Some(LoaderKind::Fabric),
    Some(LoaderKind::Quilt),
    Some(LoaderKind::Forge),
];

/// The Settings tab of an instance: what it is, what to do with it, and its launch overrides.
pub struct InstanceSettings {
    id: String,
    store: Instances,
    instance: Instance,
    /// Instances installed from a pack take their versions from it.
    from_pack: bool,
    name: Entity<InputState>,
    search: Entity<InputState>,
    releases: Vec<SharedString>,
    changing_version: bool,
    version_error: Option<SharedString>,
    java: Entity<JavaPicker>,
    fields: Vec<Field>,
    _subs: Vec<Subscription>,
}

impl InstanceSettings {
    pub fn new(id: String, store: Instances, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instance = store.load(&id).unwrap_or_else(|e| {
            tracing::error!("{e}");
            Instance {
                name: id.clone(),
                minecraft: String::new(),
                loader: None,
                overrides: Default::default(),
                last_played: None,
                play_seconds: 0,
            }
        });
        let from_pack = AppState::global(cx).read(cx).packs.contains_key(&id);
        let global = AppState::global(cx).read(cx).settings.launch.clone();
        let effective = instance.overrides.resolve(&global);
        let mut subs = Vec::new();

        let name = cx.new(|cx| InputState::new(window, cx).default_value(instance.name.clone()));
        subs.push(cx.subscribe(&name, |this, input, event, cx| {
            if let InputEvent::Blur | InputEvent::PressEnter { .. } = event {
                let name = input.read(cx).value().trim().to_string();
                if !name.is_empty() && name != this.instance.name {
                    this.instance.name = name;
                    this.save(cx);
                }
            }
        }));
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("new_instance.find_version").to_string())
        });
        subs.push(cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        }));

        let view = cx.entity().downgrade();
        let java = JavaPicker::new(
            SharedString::from(format!("java-{id}")),
            effective.java.clone(),
            riven_launch::java::required_major(&instance.minecraft),
            move |choice, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.instance.overrides.java = Some(choice);
                    this.save(cx);
                });
            },
            cx,
        );

        let commands = &effective.commands;
        let line = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            Input::Line(cx.new(|cx| InputState::new(window, cx).default_value(value)))
        };
        let specs: Vec<(Group, String, Input)> = vec![
            (
                Group::Memory,
                t!("settings.memory_min").into(),
                line(effective.memory.min.to_string(), window, cx),
            ),
            (
                Group::Memory,
                t!("settings.memory_max").into(),
                line(effective.memory.max.to_string(), window, cx),
            ),
            (
                Group::JvmArgs,
                t!("settings.jvm_args").into(),
                Input::Area(ui::textarea(
                    effective.jvm_args.join(" "),
                    (3, 12),
                    window,
                    cx,
                )),
            ),
            (
                Group::Window,
                t!("settings.window_width").into(),
                line(effective.window.width.to_string(), window, cx),
            ),
            (
                Group::Window,
                t!("settings.window_height").into(),
                line(effective.window.height.to_string(), window, cx),
            ),
            (
                Group::Commands,
                t!("instance_settings.pre_launch").into(),
                Input::Area(ui::textarea(
                    commands.pre_launch.clone().unwrap_or_default(),
                    (3, 12),
                    window,
                    cx,
                )),
            ),
            (
                Group::Commands,
                t!("instance_settings.wrapper").into(),
                Input::Area(ui::textarea(
                    commands.wrapper.clone().unwrap_or_default(),
                    (1, 3),
                    window,
                    cx,
                )),
            ),
            (
                Group::Commands,
                t!("instance_settings.post_exit").into(),
                Input::Area(ui::textarea(
                    commands.post_exit.clone().unwrap_or_default(),
                    (3, 12),
                    window,
                    cx,
                )),
            ),
        ];
        let mut fields = Vec::new();
        for (group, label, input) in specs {
            let sub = match &input {
                Input::Line(s) => cx.subscribe(s, |this, _, event, cx| {
                    if let InputEvent::Blur | InputEvent::PressEnter { .. } = event {
                        this.store_fields(cx);
                    }
                }),
                Input::Area(s) => cx.subscribe(s, |this, _, event, cx| {
                    if let InputEvent::Blur = event {
                        this.store_fields(cx);
                    }
                }),
            };
            subs.push(sub);
            fields.push(Field {
                group,
                label: label.into(),
                input,
            });
        }
        let mut view = Self {
            id,
            store,
            instance,
            from_pack,
            name,
            search,
            releases: Vec::new(),
            changing_version: false,
            version_error: None,
            java,
            fields,
            _subs: subs,
        };
        if !from_pack {
            view.load_releases(cx);
        }
        view
    }

    fn load_releases(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let releases =
                runtime::spawn(async { super::dialogs::game_meta().minecraft_releases().await })
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.releases = releases.into_iter().map(Into::into).collect();
                cx.notify();
            });
        })
        .detach();
    }

    /// Switches Minecraft or the loader, looking up the loader's newest build for that version.
    fn change_version(
        &mut self,
        minecraft: String,
        loader: Option<LoaderKind>,
        cx: &mut Context<Self>,
    ) {
        self.changing_version = true;
        self.version_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mc = minecraft.clone();
            let found = runtime::spawn(async move {
                match loader {
                    Some(kind) => super::dialogs::game_meta()
                        .loader_version(kind, &mc)
                        .await
                        .map(|version| Some(Loader { kind, version }))
                        .map_err(|e| e.to_string()),
                    None => Ok(None),
                }
            })
            .await
            .unwrap_or_else(|_| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| {
                this.changing_version = false;
                match found {
                    Ok(loader) => {
                        this.instance.minecraft = minecraft;
                        this.instance.loader = loader;
                        this.save(cx);
                    }
                    Err(e) => {
                        this.version_error = Some(e.into());
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn texts(&self, group: Group, cx: &Context<Self>) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| f.group == group)
            .map(|f| f.input.value(cx))
            .collect()
    }

    /// Reads the fields of every overridden group into the instance and saves it.
    fn store_fields(&mut self, cx: &mut Context<Self>) {
        let number = |s: &str, fallback: u32| s.parse().unwrap_or(fallback);
        let memory = self.texts(Group::Memory, cx);
        let args = self.texts(Group::JvmArgs, cx);
        let window = self.texts(Group::Window, cx);
        let commands = self.texts(Group::Commands, cx);
        let o = &mut self.instance.overrides;
        if let Some(current) = o.memory {
            o.memory = Some(MemoryMb {
                min: number(&memory[0], current.min),
                max: number(&memory[1], current.max),
            });
        }
        if o.jvm_args.is_some() {
            o.jvm_args = Some(args[0].split_whitespace().map(str::to_owned).collect());
        }
        if let Some(current) = o.window {
            o.window = Some(GameWindow {
                width: number(&window[0], current.width),
                height: number(&window[1], current.height),
                fullscreen: current.fullscreen,
            });
        }
        if o.commands.is_some() {
            let opt = |s: &String| (!s.is_empty()).then(|| s.clone());
            o.commands = Some(LaunchCommands {
                pre_launch: opt(&commands[0]),
                wrapper: opt(&commands[1]),
                post_exit: opt(&commands[2]),
            });
        }
        self.save(cx);
    }

    /// Writes the edited fields over the instance on disk, keeping what launches changed.
    fn save(&mut self, cx: &mut Context<Self>) {
        let mut fresh = self
            .store
            .load(&self.id)
            .unwrap_or_else(|_| self.instance.clone());
        fresh.name = self.instance.name.clone();
        fresh.overrides = self.instance.overrides.clone();
        if !self.from_pack {
            fresh.minecraft = self.instance.minecraft.clone();
            fresh.loader = self.instance.loader.clone();
        }
        self.instance = fresh;
        let (id, instance) = (self.id.clone(), self.instance.clone());
        AppState::global(cx).update(cx, |s, cx| s.save_instance(&id, &instance, cx));
        cx.notify();
    }

    fn is_overridden(&self, group: Group) -> bool {
        let o = &self.instance.overrides;
        match group {
            Group::Memory => o.memory.is_some(),
            Group::Java => o.java.is_some(),
            Group::JvmArgs => o.jvm_args.is_some(),
            Group::Window => o.window.is_some(),
            Group::Commands => o.commands.is_some(),
        }
    }

    /// Starts overriding a group from the launcher's current value, or drops the override.
    fn toggle(&mut self, group: Group, on: bool, global: &LaunchSettings, cx: &mut Context<Self>) {
        let o = &mut self.instance.overrides;
        match group {
            Group::Memory => o.memory = on.then_some(global.memory),
            Group::Java => o.java = on.then(|| global.java.clone()),
            Group::JvmArgs => o.jvm_args = on.then(|| global.jvm_args.clone()),
            Group::Window => o.window = on.then_some(global.window),
            Group::Commands => o.commands = on.then(|| global.commands.clone()),
        }
        self.save(cx);
    }

    fn inherited(group: Group, global: &LaunchSettings) -> String {
        match group {
            Group::Memory => format!("{}–{} MB", global.memory.min, global.memory.max),
            Group::Java => match &global.java {
                JavaChoice::Auto => t!("instance_settings.java_auto").into(),
                JavaChoice::Path { path } => path.clone(),
            },
            Group::JvmArgs if global.jvm_args.is_empty() => t!("instance_settings.none").into(),
            Group::JvmArgs => global.jvm_args.join(" "),
            Group::Window => format!("{}×{}", global.window.width, global.window.height),
            Group::Commands => {
                let c = &global.commands;
                if c.pre_launch.is_none() && c.wrapper.is_none() && c.post_exit.is_none() {
                    t!("instance_settings.none").into()
                } else {
                    t!("instance_settings.custom").into()
                }
            }
        }
    }

    fn render_about(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let played = AppState::global(cx)
            .read(cx)
            .instance(&self.id)
            .and_then(super::launch_bar::played);
        let version: AnyElement = if self.from_pack {
            div()
                .text_color(c.muted)
                .child(format!(
                    "{} · {}",
                    super::launch_bar::loader_label(&self.instance),
                    t!("instance_settings.set_by_pack")
                ))
                .into_any_element()
        } else {
            let loader = LOADERS
                .iter()
                .position(|l| *l == self.instance.loader.as_ref().map(|l| l.kind))
                .unwrap_or(0);
            let versions = self
                .releases
                .iter()
                .map(|r| MenuItem::new(r.clone(), r.clone()))
                .collect();
            let loaders = LOADERS
                .iter()
                .enumerate()
                .map(|(i, l)| MenuItem::new(i.to_string(), super::dialogs::loader_label(*l)))
                .collect();
            let pick_mc = cx.entity().downgrade();
            let pick_loader = pick_mc.clone();
            let current_loader = self.instance.loader.as_ref().map(|l| l.kind);
            let current_mc = self.instance.minecraft.clone();
            h_flex()
                .gap(px(8.))
                .child(
                    Dropdown::new(
                        "instance-minecraft",
                        versions,
                        Some(self.instance.minecraft.clone().into()),
                        move |v, _, cx| {
                            let _ = pick_mc.update(cx, |this, cx| {
                                this.change_version(v.to_string(), current_loader, cx)
                            });
                        },
                    )
                    .width(px(150.))
                    .placeholder(self.instance.minecraft.clone())
                    .searchable(&self.search),
                )
                .child(
                    Dropdown::new(
                        "instance-loader",
                        loaders,
                        Some(loader.to_string().into()),
                        move |v, _, cx| {
                            let kind = LOADERS[v.parse().unwrap_or(0)];
                            let mc = current_mc.clone();
                            let _ = pick_loader
                                .update(cx, |this, cx| this.change_version(mc, kind, cx));
                        },
                    )
                    .width(px(150.)),
                )
                .into_any_element()
        };
        let loader_hint = match (&self.instance.loader, self.changing_version) {
            (_, true) => Some(t!("instance_settings.looking_up").into()),
            (Some(l), false) => Some(
                t!(
                    "instance_settings.loader_build",
                    loader = super::launch_bar::loader_display(l.kind),
                    version = l.version
                )
                .into(),
            ),
            (None, false) => None,
        };
        let id = self.id.clone();
        let dir = self.store.dir(&self.id);
        let busy = AppState::global(cx)
            .read(cx)
            .sessions
            .get(&self.id)
            .is_some_and(super::session::Session::busy);
        let delete_name = self.instance.name.clone();
        Section::new()
            .row(setting_row(
                t!("new_instance.name").to_string(),
                None,
                TextField::new(&self.name).w(px(308.)),
                cx,
            ))
            .row(
                setting_row(
                    t!("instance_settings.version").to_string(),
                    loader_hint,
                    version,
                    cx,
                )
                .when_some(self.version_error.clone(), |row, e| {
                    row.child(div().text_color(c.warn).child(e))
                }),
            )
            .row(setting_row(
                t!("instance_settings.played").to_string(),
                None,
                div().text_color(c.text2).child(
                    played.unwrap_or_else(|| t!("instance_settings.never_played").to_string()),
                ),
                cx,
            ))
            .row(
                h_flex()
                    .gap(px(8.))
                    .px(px(18.))
                    .py(px(14.))
                    .child(
                        Button::new("instance-open-folder")
                            .icon(IconName::Folder)
                            .label(t!("instance.open_folder"))
                            .on_click(move |_, _, cx| cx.open_with_system(&dir)),
                    )
                    .child({
                        let id = id.clone();
                        Button::new("instance-duplicate")
                            .icon(IconName::Copy)
                            .label(t!("instance.duplicate"))
                            .on_click(move |_, _, cx| {
                                AppState::global(cx)
                                    .update(cx, |s, cx| s.duplicate_instance(&id, cx))
                            })
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("instance-delete")
                            .icon(IconName::Trash)
                            .label(t!("instance.delete"))
                            .disabled(busy)
                            .when(busy, |b| b.tooltip(t!("instance.delete_running")))
                            .on_click(move |_, _, cx| {
                                confirm_delete(id.clone(), delete_name.clone(), cx)
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_group(
        &self,
        group: Group,
        title: SharedString,
        global: &LaunchSettings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let on = self.is_overridden(group);
        let global_for_toggle = global.clone();
        let view = cx.entity().downgrade();
        let label = |text: SharedString| div().text_size(px(12.)).text_color(c.muted).child(text);
        let body: AnyElement = match group {
            Group::Java => self.java.clone().into_any_element(),
            Group::JvmArgs | Group::Commands => v_flex()
                .gap(px(12.))
                .children(self.fields.iter().filter(|f| f.group == group).map(|f| {
                    let input = match &f.input {
                        Input::Area(s) => TextArea::new(s).into_any_element(),
                        Input::Line(s) => TextField::new(s).into_any_element(),
                    };
                    v_flex()
                        .gap(px(6.))
                        .child(label(f.label.clone()))
                        .child(input)
                }))
                .into_any_element(),
            _ => h_flex()
                .flex_wrap()
                .items_start()
                .gap(px(16.))
                .children(self.fields.iter().filter(|f| f.group == group).map(|f| {
                    let input = match &f.input {
                        Input::Line(s) => TextField::new(s).into_any_element(),
                        Input::Area(s) => TextArea::new(s).into_any_element(),
                    };
                    v_flex()
                        .flex_1()
                        .min_w(px(160.))
                        .gap(px(6.))
                        .child(label(f.label.clone()))
                        .child(input)
                }))
                .into_any_element(),
        };
        v_flex()
            .rounded(px(10.))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(
                h_flex()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(12.))
                    .when(on, |h| h.border_b_1().border_color(c.border))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(W_SEMIBOLD)
                            .text_size(px(14.))
                            .child(title.clone()),
                    )
                    .when(!on, |h| {
                        h.child(div().max_w(px(320.)).text_color(c.muted).truncate().child(
                            format!(
                                "{} {}",
                                t!("instance_settings.inherited"),
                                Self::inherited(group, global)
                            ),
                        ))
                    })
                    .child(
                        Switch::new(SharedString::from(format!("override-{}", group as u8)), on)
                            .label(t!("instance_settings.override").to_string())
                            .accessible(format!("{} {title}", t!("instance_settings.override")))
                            .on_change(move |on, _, cx| {
                                let global = global_for_toggle.clone();
                                let _ =
                                    view.update(cx, |this, cx| this.toggle(group, on, &global, cx));
                            }),
                    ),
            )
            .when(on, |this| {
                this.child(div().px(px(16.)).py(px(12.)).child(body))
            })
    }
}

pub fn confirm_delete(id: String, name: String, cx: &mut App) {
    super::dialogs::confirm(
        t!("confirm.delete_instance_title", name = name),
        t!("confirm.delete_instance_body"),
        t!("instance.delete"),
        move |_, cx| {
            let id = id.clone();
            AppState::global(cx).update(cx, |s, cx| s.delete_instance(&id, cx));
        },
        cx,
    );
}

impl Render for InstanceSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let global = AppState::global(cx).read(cx).settings.launch.clone();
        let groups = [
            (Group::Memory, t!("settings.memory")),
            (Group::Java, t!("instance_settings.java")),
            (Group::JvmArgs, t!("settings.jvm_args")),
            (Group::Window, t!("settings.window")),
            (Group::Commands, t!("instance_settings.commands")),
        ];
        let heading = |text: String| {
            div()
                .pt(px(6.))
                .text_size(px(15.))
                .font_weight(W_SEMIBOLD)
                .text_color(c.text)
                .child(text)
        };
        div()
            .id("instance-settings")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p(px(18.))
                    .gap(px(14.))
                    .max_w(px(760.))
                    .child(heading(t!("instance_settings.instance").into()))
                    .child(self.render_about(cx))
                    .child(heading(t!("instance_settings.launch").into()))
                    .children(
                        groups.into_iter().map(|(group, title)| {
                            self.render_group(group, title.into(), &global, cx)
                        }),
                    ),
            )
    }
}
