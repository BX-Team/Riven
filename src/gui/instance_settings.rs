use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    px,
};
use riven_format::{GameWindow, Instance, JavaChoice, LaunchCommands, LaunchSettings, MemoryMb};
use riven_launch::instances::Instances;
use rust_i18n::t;

use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{Switch, TextField, W_SEMIBOLD, h_flex, v_flex};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Memory,
    Java,
    JvmArgs,
    Window,
    Commands,
}

/// One text field of an override group.
struct Field {
    group: Group,
    label: SharedString,
    input: Entity<InputState>,
}

/// The instance's own launch settings, each group inheriting the launcher's until overridden.
pub struct InstanceSettings {
    id: String,
    store: Instances,
    instance: Instance,
    fields: Vec<Field>,
    _subs: Vec<Subscription>,
}

fn java_text(java: &JavaChoice) -> String {
    match java {
        JavaChoice::Auto => String::new(),
        JavaChoice::Path { path } => path.clone(),
    }
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
        let global = AppState::global(cx).read(cx).settings.launch.clone();
        let effective = instance.overrides.resolve(&global);
        let commands = &effective.commands;
        let specs: Vec<(Group, String, String)> = vec![
            (
                Group::Memory,
                t!("settings.memory_min").into(),
                effective.memory.min.to_string(),
            ),
            (
                Group::Memory,
                t!("settings.memory_max").into(),
                effective.memory.max.to_string(),
            ),
            (
                Group::Java,
                t!("settings.java_path").into(),
                java_text(&effective.java),
            ),
            (
                Group::JvmArgs,
                t!("settings.jvm_args").into(),
                effective.jvm_args.join(" "),
            ),
            (
                Group::Window,
                t!("settings.window_width").into(),
                effective.window.width.to_string(),
            ),
            (
                Group::Window,
                t!("settings.window_height").into(),
                effective.window.height.to_string(),
            ),
            (
                Group::Commands,
                t!("instance_settings.pre_launch").into(),
                commands.pre_launch.clone().unwrap_or_default(),
            ),
            (
                Group::Commands,
                t!("instance_settings.wrapper").into(),
                commands.wrapper.clone().unwrap_or_default(),
            ),
            (
                Group::Commands,
                t!("instance_settings.post_exit").into(),
                commands.post_exit.clone().unwrap_or_default(),
            ),
        ];
        let mut fields = Vec::new();
        let mut subs = Vec::new();
        for (group, label, value) in specs {
            let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
            subs.push(cx.subscribe(&input, |this, _, event, cx| {
                if let InputEvent::Blur | InputEvent::PressEnter { .. } = event {
                    this.store_fields(cx);
                }
            }));
            fields.push(Field {
                group,
                label: label.into(),
                input,
            });
        }
        Self {
            id,
            store,
            instance,
            fields,
            _subs: subs,
        }
    }

    fn texts(&self, group: Group, cx: &Context<Self>) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| f.group == group)
            .map(|f| f.input.read(cx).value().trim().to_string())
            .collect()
    }

    /// Reads the fields of every overridden group into the instance and saves it.
    fn store_fields(&mut self, cx: &mut Context<Self>) {
        let number = |s: &str, fallback: u32| s.parse().unwrap_or(fallback);
        let o = &mut self.instance.overrides;
        if let Some(memory) = o.memory {
            let t = self
                .fields
                .iter()
                .filter(|f| f.group == Group::Memory)
                .map(|f| f.input.read(cx).value().trim().to_string())
                .collect::<Vec<_>>();
            self.instance.overrides.memory = Some(MemoryMb {
                min: number(&t[0], memory.min),
                max: number(&t[1], memory.max),
            });
        }
        let java = self.texts(Group::Java, cx);
        if self.instance.overrides.java.is_some() {
            self.instance.overrides.java = Some(if java[0].is_empty() {
                JavaChoice::Auto
            } else {
                JavaChoice::Path {
                    path: java[0].clone(),
                }
            });
        }
        let args = self.texts(Group::JvmArgs, cx);
        if self.instance.overrides.jvm_args.is_some() {
            self.instance.overrides.jvm_args =
                Some(args[0].split_whitespace().map(str::to_owned).collect());
        }
        let window = self.texts(Group::Window, cx);
        if let Some(current) = self.instance.overrides.window {
            self.instance.overrides.window = Some(GameWindow {
                width: number(&window[0], current.width),
                height: number(&window[1], current.height),
                fullscreen: current.fullscreen,
            });
        }
        let commands = self.texts(Group::Commands, cx);
        if self.instance.overrides.commands.is_some() {
            let opt = |s: &String| (!s.is_empty()).then(|| s.clone());
            self.instance.overrides.commands = Some(LaunchCommands {
                pre_launch: opt(&commands[0]),
                wrapper: opt(&commands[1]),
                post_exit: opt(&commands[2]),
            });
        }
        self.save(cx);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.store.save(&self.id, &self.instance) {
            tracing::error!("{e}");
        }
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
        let inputs: Vec<_> = self
            .fields
            .iter()
            .filter(|f| f.group == group)
            .map(|f| {
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
                    .child(TextField::new(&f.input))
            })
            .collect();
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
                        h.child(div().text_color(c.muted).truncate().child(format!(
                            "{} {}",
                            t!("instance_settings.inherited"),
                            Self::inherited(group, global)
                        )))
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
                this.child(
                    h_flex()
                        .flex_wrap()
                        .items_start()
                        .gap(px(16.))
                        .px(px(16.))
                        .py(px(12.))
                        .children(inputs),
                )
            })
    }
}

impl Render for InstanceSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let global = AppState::global(cx).read(cx).settings.launch.clone();
        let groups = [
            (Group::Memory, t!("settings.memory")),
            (Group::Java, t!("instance_settings.java")),
            (Group::JvmArgs, t!("settings.jvm_args")),
            (Group::Window, t!("settings.window")),
            (Group::Commands, t!("instance_settings.commands")),
        ];
        div()
            .id("instance-settings")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex().p(px(18.)).gap(px(14.)).max_w(px(760.)).children(
                    groups
                        .into_iter()
                        .map(|(group, title)| self.render_group(group, title.into(), &global, cx)),
                ),
            )
    }
}
