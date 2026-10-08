use std::collections::BTreeMap;

use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, relative,
};
use riven_format::{
    GameWindow, Instance, JavaChoice, LaunchCommands, LaunchSettings, Loader, LoaderKind, Release,
    State,
};
use riven_launch::instances::Instances;
use riven_sync::remote;
use riven_sync::update::Request;
use rust_i18n::t;

use super::java_picker::JavaPicker;
use super::memory_slider::MemorySlider;
use super::runtime;
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{
    self, Button, ButtonSize, Dropdown, IconName, MenuItem, Section, Switch, TextArea, TextField,
    W_SEMIBOLD, h_flex, icon, motion, setting_row, v_flex,
};

const WIDTH: f32 = 940.;
const NAV_WIDTH: f32 = 210.;
const CHANNELS: [&str; 2] = ["stable", "beta"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Pack,
    /// Java, memory and JVM arguments.
    Java,
    /// The game window and launch commands.
    Launch,
}

impl Page {
    /// The override groups a page shows, in order.
    fn groups(self) -> &'static [Group] {
        match self {
            Page::Java => &[Group::Java, Group::Memory, Group::JvmArgs],
            Page::Launch => &[Group::Window, Group::Commands],
            Page::General | Page::Pack => &[],
        }
    }
}

/// What the pack's channel serves now, looked up when the Pack page is first shown.
enum Remote {
    Unchecked,
    Checking,
    Ready(Box<Release>),
    Failed(SharedString),
}

/// The pack an instance was installed from, and the changes to it waiting to be applied.
struct PackPanel {
    state: State,
    channel: String,
    groups: BTreeMap<String, bool>,
    remote: Remote,
}

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

/// The settings window of an instance: what it is, its pack, and its launch overrides.
pub struct InstanceSettings {
    id: String,
    page: Page,
    pack: Option<PackPanel>,
    store: Instances,
    instance: Instance,
    /// Instances installed from a pack take their versions from it.
    from_pack: bool,
    name: Entity<InputState>,
    search: Entity<InputState>,
    releases: Vec<SharedString>,
    /// Builds of the instance's loader for its Minecraft, newest first.
    builds: Vec<SharedString>,
    changing_version: bool,
    version_error: Option<SharedString>,
    java: Entity<JavaPicker>,
    memory: Entity<MemorySlider>,
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
                own_mods: false,
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
        let memory = {
            let view = cx.entity().downgrade();
            MemorySlider::new(
                effective.memory,
                move |m, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if this.instance.overrides.memory.is_some() {
                            this.instance.overrides.memory = Some(m);
                            this.save(cx);
                        }
                    });
                },
                cx,
            )
        };
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
        let pack = riven_sync::install::load_state(&store.game_dir(&id))
            .ok()
            .flatten()
            .map(|state| PackPanel {
                channel: remote::channel_of(&state.source)
                    .unwrap_or_default()
                    .to_owned(),
                groups: state.groups.clone(),
                state,
                remote: Remote::Unchecked,
            });
        let mut view = Self {
            id,
            page: Page::General,
            pack,
            store,
            instance,
            from_pack,
            name,
            search,
            releases: Vec::new(),
            builds: Vec::new(),
            changing_version: false,
            version_error: None,
            java,
            memory,
            fields,
            _subs: subs,
        };
        if !from_pack {
            view.load_releases(cx);
            view.load_builds(cx);
        }
        view
    }

    fn load_builds(&mut self, cx: &mut Context<Self>) {
        self.builds.clear();
        let Some(loader) = self.instance.loader.clone() else {
            return;
        };
        let minecraft = self.instance.minecraft.clone();
        cx.spawn(async move |this, cx| {
            let builds = runtime::spawn(async move {
                super::dialogs::game_meta()
                    .loader_versions(loader.kind, &minecraft)
                    .await
            })
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.builds = builds.into_iter().map(Into::into).collect();
                cx.notify();
            });
        })
        .detach();
    }

    /// Replaces the instance's icon with a picked image, or removes it.
    fn pick_icon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let (store, id) = (self.store.clone(), self.id.clone());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let saved = runtime::blocking(move || {
                let png = std::fs::read(&path)
                    .ok()
                    .and_then(|b| super::mods::instance_icon(&b))
                    .ok_or_else(|| t!("new_instance.bad_icon").to_string())?;
                store.set_icon(&id, &png).map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|_| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| match saved {
                Ok(_) => AppState::global(cx).update(cx, |s, cx| s.reload_instances(cx)),
                Err(e) => {
                    this.version_error = Some(e.into());
                    cx.notify();
                }
            });
        })
        .detach();
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
                        this.load_builds(cx);
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
        let args = self.texts(Group::JvmArgs, cx);
        let window = self.texts(Group::Window, cx);
        let commands = self.texts(Group::Commands, cx);
        let o = &mut self.instance.overrides;
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
    fn toggle(
        &mut self,
        group: Group,
        on: bool,
        global: &LaunchSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let o = &mut self.instance.overrides;
        match group {
            Group::Memory => {
                o.memory = on.then_some(global.memory);
                let shown = o.memory.unwrap_or(global.memory);
                self.memory.update(cx, |m, cx| m.set(shown, window, cx));
            }
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

    fn render_general(&self, cx: &mut Context<Self>) -> AnyElement {
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
            let pick_build = pick_mc.clone();
            let builds: Vec<MenuItem> = self
                .builds
                .iter()
                .map(|b| MenuItem::new(b.clone(), b.clone()))
                .collect();
            let build = self.instance.loader.as_ref().map(|l| l.version.clone());
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
                .when_some(build, |row, build| {
                    row.child(
                        Dropdown::new(
                            "instance-loader-build",
                            builds,
                            Some(build.clone().into()),
                            move |v, _, cx| {
                                let _ = pick_build.update(cx, |this, cx| {
                                    if let Some(loader) = &mut this.instance.loader {
                                        loader.version = v.to_string();
                                        this.save(cx);
                                    }
                                });
                            },
                        )
                        .width(px(150.))
                        .placeholder(build),
                    )
                })
                .into_any_element()
        };
        let loader_hint = match (&self.instance.loader, self.changing_version) {
            (_, true) => Some(t!("instance_settings.looking_up").into()),
            (Some(_), false) if !self.from_pack => Some(t!("instance_settings.loader_pick").into()),
            _ => None,
        };
        let icon_path = AppState::global(cx).read(cx).icons.get(&self.id).cloned();
        let has_icon = icon_path.is_some();
        let pack_icon = self.store.has_pack_icon(&self.id);
        let icon_row = h_flex()
            .gap(px(10.))
            .child(super::launch_bar::instance_tile(
                &self.instance.name,
                icon_path.as_ref(),
                40.,
                8.,
                cx,
            ))
            .when(pack_icon, |row| {
                row.child(
                    div()
                        .text_color(c.muted)
                        .child(t!("instance_settings.icon_from_pack").to_string()),
                )
            })
            .when(!pack_icon, |row| {
                row.child(
                    Button::new("instance-icon-pick")
                        .icon(IconName::Image)
                        .label(t!("instance_settings.icon_change"))
                        .on_click(cx.listener(|this, _, window, cx| this.pick_icon(window, cx))),
                )
            })
            .when(has_icon && !pack_icon, |row| {
                let (store, id) = (self.store.clone(), self.id.clone());
                row.child(
                    Button::new("instance-icon-clear")
                        .ghost()
                        .icon(IconName::Close)
                        .tooltip(t!("new_instance.clear_icon"))
                        .on_click(move |_, _, cx| {
                            if let Err(e) = store.clear_icon(&id) {
                                tracing::warn!("{e}");
                            }
                            AppState::global(cx).update(cx, |s, cx| s.reload_instances(cx));
                        }),
                )
            });
        let id = self.id.clone();
        let dir = self.store.dir(&self.id);
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
                t!("new_instance.icon").to_string(),
                None,
                icon_row,
                cx,
            ))
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
                    }),
            )
            .into_any_element()
    }

    /// The single-line fields of a group side by side.
    fn field_row(&self, group: Group, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        h_flex()
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
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .child(f.label.clone()),
                    )
                    .child(input)
            }))
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
            Group::Memory => self.memory.clone().into_any_element(),
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
            Group::Window => {
                let fullscreen = self.instance.overrides.window.is_some_and(|w| w.fullscreen);
                let toggle = cx.entity().downgrade();
                v_flex()
                    .gap(px(14.))
                    .child(self.field_row(group, cx))
                    .child(
                        Switch::new("override-fullscreen", fullscreen)
                            .label(t!("settings.window_fullscreen").to_string())
                            .on_change(move |on, _, cx| {
                                let _ = toggle.update(cx, |this, cx| {
                                    if let Some(w) = &mut this.instance.overrides.window {
                                        w.fullscreen = on;
                                    }
                                    this.save(cx);
                                });
                            }),
                    )
                    .into_any_element()
            }
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
                            .on_change(move |on, window, cx| {
                                let global = global_for_toggle.clone();
                                let _ = view.update(cx, |this, cx| {
                                    this.toggle(group, on, &global, window, cx)
                                });
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

/// Asks before letting the player add mods to a pack instance.
pub fn confirm_unlock(id: String, cx: &mut App) {
    super::dialogs::confirm(
        t!("confirm.unlock_title"),
        t!("confirm.unlock_body"),
        t!("confirm.unlock_action"),
        move |_, cx| set_own_mods(&id, true, cx),
        cx,
    );
}

fn set_own_mods(id: &str, on: bool, cx: &mut App) {
    let state = AppState::global(cx);
    let Some(store) = state.read(cx).store.clone() else {
        return;
    };
    match store.load(id) {
        Ok(mut instance) => {
            instance.own_mods = on;
            state.update(cx, |s, cx| s.save_instance(id, &instance, cx));
        }
        Err(e) => tracing::error!("{e}"),
    }
}

impl InstanceSettings {
    fn pages(&self) -> Vec<(Page, SharedString)> {
        let mut pages = vec![(Page::General, t!("instance_settings.general").into())];
        if self.pack.is_some() {
            pages.push((Page::Pack, t!("instance_settings.pack").into()));
        }
        pages.extend([
            (Page::Java, t!("instance_settings.java").into()),
            (Page::Launch, t!("instance_settings.launch").into()),
        ]);
        pages
    }

    fn group_title(group: Group) -> SharedString {
        match group {
            Group::Memory => t!("settings.memory"),
            Group::Java => t!("instance_settings.java_runtime"),
            Group::JvmArgs => t!("settings.jvm_args"),
            Group::Window => t!("settings.window"),
            Group::Commands => t!("instance_settings.commands"),
        }
        .into()
    }

    fn show(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        if page == Page::Pack
            && self
                .pack
                .as_ref()
                .is_some_and(|p| matches!(p.remote, Remote::Unchecked))
        {
            self.check_pack(cx);
        }
        cx.notify();
    }

    /// The pointer the pack is followed through, on the picked channel.
    fn pointer(pack: &PackPanel) -> String {
        remote::with_channel(&pack.state.source, &pack.channel)
            .unwrap_or_else(|| pack.state.source.clone())
    }

    /// Reads what the picked channel serves, for its version and optional groups.
    fn check_pack(&mut self, cx: &mut Context<Self>) {
        let Some(pack) = &mut self.pack else {
            return;
        };
        pack.remote = Remote::Checking;
        let source = Self::pointer(pack);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = runtime::spawn(async move {
                let store = riven_sync::Store::default_location()
                    .ok_or_else(|| "no data directory".to_owned())?;
                riven_sync::update::preview(&store, &riven_sources::client(), &source, None)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|_| Err(String::new()));
            let _ = this.update(cx, |this, cx| {
                if let Some(pack) = &mut this.pack {
                    pack.remote = match found {
                        Ok(release) => {
                            for g in &release.groups {
                                pack.groups.entry(g.id.clone()).or_insert(g.default);
                            }
                            Remote::Ready(Box::new(release))
                        }
                        Err(e) => Remote::Failed(e.into()),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Group switches that differ from what is installed, as `+id` / `-id`.
    fn group_changes(pack: &PackPanel) -> Vec<String> {
        pack.groups
            .iter()
            .filter(|(id, on)| pack.state.groups.get(*id) != Some(on))
            .map(|(id, on)| format!("{}{id}", if *on { "+" } else { "-" }))
            .collect()
    }

    /// Installs the pack again with the picked channel and groups, or its newer version.
    fn apply_pack(&mut self, cx: &mut Context<Self>) {
        let Some(pack) = &self.pack else {
            return;
        };
        let request = Request {
            source: Some(Self::pointer(pack)),
            groups: Self::group_changes(pack),
            ..Request::default()
        };
        let id = self.id.clone();
        AppState::global(cx).update(cx, |s, cx| {
            s.install_pack(&id, request, cx);
            s.toast(
                super::toast::ToastKind::Info,
                t!("instance_settings.pack_applying"),
                cx,
            );
            s.close_modal(cx);
        });
    }

    fn render_pack(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let Some(pack) = &self.pack else {
            return div().into_any_element();
        };
        let busy = AppState::global(cx)
            .read(cx)
            .sessions
            .get(&self.id)
            .is_some_and(super::session::Session::busy);
        let view = cx.entity().downgrade();
        let source = pack.state.source.clone();
        let copy = source.clone();
        let channels: Vec<MenuItem> = CHANNELS
            .iter()
            .map(|c| (*c).to_owned())
            .chain(
                (!CHANNELS.contains(&pack.channel.as_str()) && !pack.channel.is_empty())
                    .then(|| pack.channel.clone()),
            )
            .map(|c| MenuItem::new(c.clone(), c))
            .collect();
        let has_channels = remote::channel_of(&pack.state.source).is_some();
        let latest = match &pack.remote {
            Remote::Ready(release) => Some(release.version.clone()),
            _ => None,
        };
        let newer = latest.as_ref().is_some_and(|v| *v != pack.state.version);
        let changes = !Self::group_changes(pack).is_empty()
            || remote::channel_of(&pack.state.source).is_some_and(|c| c != pack.channel);
        let status: AnyElement = match &pack.remote {
            Remote::Unchecked => div().into_any_element(),
            Remote::Checking => h_flex()
                .gap(px(8.))
                .text_color(c.muted)
                .child(motion::spinner(
                    "pack-check",
                    icon(IconName::Loader, c.muted).size(px(13.)),
                    cx,
                ))
                .child(t!("instance_settings.pack_checking").to_string())
                .into_any_element(),
            Remote::Failed(e) => div()
                .text_color(c.warn)
                .line_height(relative(1.4))
                .child(e.clone())
                .into_any_element(),
            Remote::Ready(_) if newer => div()
                .text_color(c.accent)
                .child(
                    t!(
                        "instance_settings.pack_newer",
                        version = latest.clone().unwrap_or_default()
                    )
                    .to_string(),
                )
                .into_any_element(),
            Remote::Ready(_) => div()
                .text_color(c.ok)
                .child(t!("instance_settings.pack_current").to_string())
                .into_any_element(),
        };
        let groups: Vec<AnyElement> = match &pack.remote {
            Remote::Ready(release) => release
                .groups
                .iter()
                .enumerate()
                .map(|(i, g)| {
                    let on = pack.groups.get(&g.id).copied().unwrap_or(g.default);
                    let (id, view) = (g.id.clone(), view.clone());
                    h_flex()
                        .gap(px(12.))
                        .px(px(18.))
                        .py(px(12.))
                        .when(i > 0, |r| r.border_t_1().border_color(c.row))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().font_weight(W_SEMIBOLD).child(g.name.clone()))
                                .when_some(g.description.clone(), |col, d| {
                                    col.child(div().text_size(px(12.)).text_color(c.muted).child(d))
                                }),
                        )
                        .child(
                            Switch::new(("pack-group", i), on)
                                .accessible(g.name.clone())
                                .on_change(move |on, _, cx| {
                                    let id = id.clone();
                                    let _ = view.update(cx, |this, cx| {
                                        if let Some(pack) = &mut this.pack {
                                            pack.groups.insert(id, on);
                                        }
                                        cx.notify();
                                    });
                                }),
                        )
                        .into_any_element()
                })
                .collect(),
            _ => Vec::new(),
        };
        let groups_ready = matches!(pack.remote, Remote::Ready(_));
        let own_mods = AppState::global(cx)
            .read(cx)
            .instance(&self.id)
            .is_some_and(|i| i.own_mods);
        let unlock_id = self.id.clone();
        let pick = view.clone();
        v_flex()
            .gap(px(14.))
            .child(
                Section::new()
                    .row(setting_row(
                        t!("instance_settings.pack_source").to_string(),
                        None,
                        h_flex()
                            .gap(px(6.))
                            .max_w(px(380.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(mono)
                                    .text_size(px(12.))
                                    .text_color(c.text2)
                                    .child(source),
                            )
                            .child(
                                Button::new("pack-copy")
                                    .ghost()
                                    .size(ButtonSize::Xs)
                                    .icon(IconName::Copy)
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            ),
                        cx,
                    ))
                    .row(setting_row(
                        t!("instance_settings.pack_version").to_string(),
                        Some(t!("instance_settings.pack_version_hint").into()),
                        h_flex()
                            .gap(px(12.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(pack.state.version.clone()),
                            )
                            .child(status)
                            .child(
                                Button::new("pack-check-now")
                                    .icon(IconName::Refresh)
                                    .label(t!("instance_settings.pack_check"))
                                    .disabled(matches!(pack.remote, Remote::Checking))
                                    .on_click(cx.listener(|this, _, _, cx| this.check_pack(cx))),
                            ),
                        cx,
                    ))
                    .when(has_channels, |section| {
                        section.row(setting_row(
                            t!("instance_settings.pack_channel").to_string(),
                            Some(t!("instance_settings.pack_channel_hint").into()),
                            Dropdown::new(
                                "pack-channel",
                                channels,
                                Some(pack.channel.clone().into()),
                                move |v, _, cx| {
                                    let _ = pick.update(cx, |this, cx| {
                                        if let Some(pack) = &mut this.pack {
                                            pack.channel = v.to_string();
                                        }
                                        this.check_pack(cx);
                                    });
                                },
                            )
                            .width(px(160.)),
                            cx,
                        ))
                    })
                    .row(setting_row(
                        t!("instance_settings.own_mods").to_string(),
                        Some(t!("instance_settings.own_mods_hint").into()),
                        Switch::new("own-mods", own_mods)
                            .accessible(t!("instance_settings.own_mods"))
                            .on_change(move |on, _, cx| {
                                if on {
                                    confirm_unlock(unlock_id.clone(), cx)
                                } else {
                                    set_own_mods(&unlock_id, false, cx)
                                }
                            }),
                        cx,
                    )),
            )
            .child(
                div()
                    .pt(px(4.))
                    .font_weight(W_SEMIBOLD)
                    .child(t!("instance_settings.pack_groups").to_string()),
            )
            .child(
                v_flex()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.panel)
                    .map(|card| {
                        if !groups_ready {
                            card.child(
                                div()
                                    .px(px(18.))
                                    .py(px(12.))
                                    .text_color(c.muted)
                                    .child(t!("instance_settings.pack_groups_wait").to_string()),
                            )
                        } else if groups.is_empty() {
                            card.child(
                                div()
                                    .px(px(18.))
                                    .py(px(12.))
                                    .text_color(c.muted)
                                    .child(t!("instance_settings.pack_no_groups").to_string()),
                            )
                        } else {
                            card.children(groups)
                        }
                    }),
            )
            .when(newer || changes, |col| {
                col.child(
                    h_flex()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .text_color(c.muted)
                                .child(t!("instance_settings.pack_apply_hint").to_string()),
                        )
                        .child(
                            Button::new("pack-apply")
                                .primary()
                                .icon(IconName::Refresh)
                                .label(if changes {
                                    t!("instance_settings.pack_apply")
                                } else {
                                    t!(
                                        "instance_settings.pack_update",
                                        version = latest.unwrap_or_default()
                                    )
                                })
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.apply_pack(cx))),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let rows = self
            .pages()
            .into_iter()
            .enumerate()
            .map(|(i, (page, label))| {
                let on = self.page == page;
                let overridden = page.groups().iter().any(|g| self.is_overridden(*g));
                h_flex()
                    .id(("settings-page", i))
                    .h(px(32.))
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .map(|r| {
                        if on {
                            r.bg(c.sel).font_weight(W_SEMIBOLD).text_color(c.text)
                        } else {
                            r.text_color(c.text2).hover(|s| s.bg(c.row))
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.show(page, cx)))
                    .child(div().flex_1().truncate().child(label))
                    .when(overridden, |r| {
                        r.child(div().size(px(6.)).rounded_full().bg(c.accent))
                    })
            });
        v_flex()
            .w(px(NAV_WIDTH))
            .flex_none()
            .h_full()
            .p(px(10.))
            .gap(px(2.))
            .rounded_l(px(12.))
            .bg(c.bg)
            .border_r_1()
            .border_color(c.border)
            .child(
                div()
                    .px(px(10.))
                    .pt(px(6.))
                    .pb(px(10.))
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(15.))
                    .truncate()
                    .child(self.instance.name.clone()),
            )
            .children(rows)
            .child(div().flex_1())
            .child(self.render_delete(cx))
            .into_any_element()
    }

    /// Deleting sits apart at the bottom of the side bar, away from everyday settings.
    fn render_delete(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let danger = super::theme::danger();
        let busy = AppState::global(cx)
            .read(cx)
            .sessions
            .get(&self.id)
            .is_some_and(super::session::Session::busy);
        let (id, name) = (self.id.clone(), self.instance.name.clone());
        h_flex()
            .id("instance-delete")
            .h(px(32.))
            .px(px(10.))
            .gap(px(8.))
            .rounded(px(6.))
            .text_color(if busy { c.muted } else { danger })
            .child(icon(IconName::Trash, if busy { c.muted } else { danger }).size(px(14.)))
            .child(t!("instance.delete").to_string())
            .map(|row| {
                if busy {
                    row.tooltip(ui::tooltip(t!("instance.delete_running").into()))
                } else {
                    row.cursor_pointer()
                        .hover(move |s| s.bg(danger.opacity(0.12)))
                        .on_click(move |_, _, cx| confirm_delete(id.clone(), name.clone(), cx))
                }
            })
            .into_any_element()
    }
}

impl Render for InstanceSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let height = (f32::from(window.viewport_size().height) - 120.).clamp(320., 640.);
        let global = AppState::global(cx).read(cx).settings.launch.clone();
        let (title, hint): (SharedString, Option<SharedString>) = match self.page {
            Page::General => (t!("instance_settings.general").into(), None),
            Page::Pack => (
                t!("instance_settings.pack").into(),
                Some(t!("instance_settings.pack_hint").into()),
            ),
            Page::Java | Page::Launch => (
                self.pages()
                    .into_iter()
                    .find(|(p, _)| *p == self.page)
                    .map(|(_, l)| l)
                    .unwrap_or_default(),
                Some(t!("instance_settings.launch_hint").into()),
            ),
        };
        let body = match self.page {
            Page::General => self.render_general(cx),
            Page::Pack => self.render_pack(cx),
            page => {
                let cards: Vec<AnyElement> = page
                    .groups()
                    .iter()
                    .map(|&group| {
                        self.render_group(group, Self::group_title(group), &global, cx)
                            .into_any_element()
                    })
                    .collect();
                v_flex().gap(px(12.)).children(cards).into_any_element()
            }
        };
        let page_key = match self.page {
            Page::General => 0usize,
            Page::Pack => 1,
            Page::Java => 2usize,
            Page::Launch => 3,
        };
        h_flex()
            .h(px(height))
            .items_start()
            .rounded(px(12.))
            .overflow_hidden()
            .child(self.render_nav(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(12.))
                            .px(px(22.))
                            .pt(px(18.))
                            .pb(px(12.))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::BOLD)
                                            .text_size(px(17.))
                                            .child(title),
                                    )
                                    .when_some(hint, |col, hint| {
                                        col.child(
                                            div()
                                                .text_color(c.muted)
                                                .line_height(relative(1.5))
                                                .child(hint),
                                        )
                                    }),
                            )
                            .child(
                                Button::new("settings-close")
                                    .ghost()
                                    .icon(IconName::Close)
                                    .tooltip(t!("common.close"))
                                    .on_click(|_, _, cx| {
                                        AppState::global(cx).update(cx, |s, cx| s.close_modal(cx))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("instance-settings-page")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(motion::enter(
                                ("settings-page-in", page_key),
                                v_flex().px(px(22.)).pb(px(22.)).child(body),
                                6.,
                                window,
                                cx,
                            )),
                    ),
            )
    }
}

/// Opens the settings window of an instance, optionally on its Pack page.
pub fn open(id: String, pack: bool, window: &mut Window, cx: &mut App) {
    let Some(store) = AppState::global(cx).read(cx).store.clone() else {
        return;
    };
    let view = cx.new(|cx| {
        let mut view = InstanceSettings::new(id, store, window, cx);
        if pack && view.pack.is_some() {
            view.show(Page::Pack, cx);
        }
        view
    });
    super::dialogs::open(view.into(), WIDTH, cx);
}
