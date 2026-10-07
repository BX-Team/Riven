use std::path::{Path, PathBuf};

use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use riven_format::{AccountKind, Settings as Prefs, ThemeMode};
use rust_i18n::t;

use super::app::{SIDEBAR_WIDTH, nav_row};
use super::java_picker::JavaPicker;
use super::launch_bar::initials;
use super::runtime;
use super::state::AppState;
use super::theme::{self, ActiveTheme as _};
use super::ui::{
    self, Button, ButtonSize, Dropdown, IconName, MenuItem, Section, Switch, TextArea, TextField,
    W_SEMIBOLD, caption, h_flex, icon, setting_block, setting_row, tile, v_flex,
};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Java,
    Launch,
    Accounts,
    Folders,
    Advanced,
    About,
}

const PAGES: [Page; 7] = [
    Page::General,
    Page::Java,
    Page::Launch,
    Page::Accounts,
    Page::Folders,
    Page::Advanced,
    Page::About,
];

impl Page {
    fn title(self) -> String {
        match self {
            Page::General => t!("settings.general"),
            Page::Java => t!("settings.java"),
            Page::Launch => t!("settings.launch"),
            Page::Accounts => t!("settings.accounts"),
            Page::Folders => t!("settings.folders"),
            Page::Advanced => t!("settings.advanced"),
            Page::About => t!("settings.about"),
        }
        .into()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Text {
    JvmArgs,
    MemoryMin,
    MemoryMax,
    Width,
    Height,
    PreLaunch,
    Wrapper,
    PostExit,
}

const FIELDS: [Text; 4] = [Text::MemoryMin, Text::MemoryMax, Text::Width, Text::Height];

/// Settings that can grow long, edited in multi-line fields.
const AREAS: [Text; 4] = [
    Text::JvmArgs,
    Text::PreLaunch,
    Text::Wrapper,
    Text::PostExit,
];

impl Text {
    fn read(self, s: &Prefs) -> String {
        let l = &s.launch;
        let opt = |v: &Option<String>| v.clone().unwrap_or_default();
        match self {
            Text::JvmArgs => l.jvm_args.join(" "),
            Text::MemoryMin => l.memory.min.to_string(),
            Text::MemoryMax => l.memory.max.to_string(),
            Text::Width => l.window.width.to_string(),
            Text::Height => l.window.height.to_string(),
            Text::PreLaunch => opt(&l.commands.pre_launch),
            Text::Wrapper => opt(&l.commands.wrapper),
            Text::PostExit => opt(&l.commands.post_exit),
        }
    }

    fn write(self, value: &str, s: &mut Prefs) {
        let l = &mut s.launch;
        let number = |fallback: u32| value.parse().unwrap_or(fallback);
        let opt = || (!value.is_empty()).then(|| value.to_owned());
        match self {
            Text::JvmArgs => l.jvm_args = value.split_whitespace().map(str::to_owned).collect(),
            Text::MemoryMin => l.memory.min = number(l.memory.min),
            Text::MemoryMax => l.memory.max = number(l.memory.max),
            Text::Width => l.window.width = number(l.window.width),
            Text::Height => l.window.height = number(l.window.height),
            Text::PreLaunch => l.commands.pre_launch = opt(),
            Text::Wrapper => l.commands.wrapper = opt(),
            Text::PostExit => l.commands.post_exit = opt(),
        }
    }
}

/// A folder the launcher keeps, shown with its size on the Folders page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Folder {
    Instances,
    Store,
    Runtime,
    Cache,
    Config,
}

const FOLDERS: [Folder; 5] = [
    Folder::Instances,
    Folder::Store,
    Folder::Runtime,
    Folder::Cache,
    Folder::Config,
];

/// What the Runtime row counts: shared game downloads and Java, all straight under the data folder.
const RUNTIME: [&str; 5] = ["java", "java-custom", "libraries", "assets", "packs"];

impl Folder {
    fn path(self) -> Option<PathBuf> {
        let data = riven_sync::data_dir();
        match self {
            Folder::Instances => data.map(|d| d.join("instances")),
            Folder::Store => data.map(|d| d.join("store")),
            Folder::Runtime => data,
            Folder::Cache => data.map(|d| d.join("cache")),
            Folder::Config => riven_sync::config_dir(),
        }
    }

    fn title(self) -> String {
        match self {
            Folder::Instances => t!("folders.instances"),
            Folder::Store => t!("folders.store"),
            Folder::Runtime => t!("folders.runtime"),
            Folder::Cache => t!("folders.cache"),
            Folder::Config => t!("folders.config"),
        }
        .into()
    }

    fn hint(self) -> String {
        match self {
            Folder::Instances => t!("folders.instances_hint"),
            Folder::Store => t!("folders.store_hint"),
            Folder::Runtime => t!("folders.runtime_hint"),
            Folder::Cache => t!("folders.cache_hint"),
            Folder::Config => t!("folders.config_hint"),
        }
        .into()
    }

    fn size(self) -> u64 {
        let Some(path) = self.path() else {
            return 0;
        };
        match self {
            Folder::Runtime => RUNTIME.iter().map(|d| dir_size(&path.join(d))).sum(),
            // Windows and macOS keep data and settings in one folder; only its own files are settings.
            Folder::Config if riven_sync::data_dir().as_ref() == Some(&path) => {
                std::fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                    .map(|e| e.metadata().map_or(0, |m| m.len()))
                    .sum()
            }
            _ => dir_size(&path),
        }
    }
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The launcher settings screen: its own page list on the left, cards on the right.
pub struct SettingsView {
    page: Page,
    fields: Vec<(Text, Entity<InputState>)>,
    areas: Vec<(Text, Entity<TextareaState>)>,
    java: Entity<JavaPicker>,
    sizes: Option<Vec<(Folder, u64)>>,
    _subs: Vec<Subscription>,
}

fn prefs(cx: &App) -> &Prefs {
    &AppState::global(cx).read(cx).settings
}

fn write(cx: &mut App, edit: impl FnOnce(&mut Prefs)) {
    AppState::global(cx).update(cx, |state, cx| state.update_settings(edit, cx));
}

fn reapply_theme(cx: &mut App) {
    let appearance = prefs(cx).appearance.clone();
    theme::apply(&appearance, None, cx);
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut fields = Vec::new();
        let mut subs = Vec::new();
        for kind in FIELDS {
            let value = kind.read(prefs(cx));
            let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
            subs.push(cx.subscribe(&input, move |_, input, event, cx| {
                if let InputEvent::Blur | InputEvent::PressEnter { .. } = event {
                    let value = input.read(cx).value().trim().to_string();
                    write(cx, |s| kind.write(&value, s));
                }
            }));
            fields.push((kind, input));
        }
        let mut areas = Vec::new();
        for kind in AREAS {
            let lines = if kind == Text::Wrapper {
                (1, 3)
            } else {
                (3, 12)
            };
            let area = ui::textarea(kind.read(prefs(cx)), lines, window, cx);
            subs.push(cx.subscribe(&area, move |_, area, event, cx| {
                if let InputEvent::Blur = event {
                    let value = area.read(cx).value().trim().to_string();
                    write(cx, |s| kind.write(&value, s));
                }
            }));
            areas.push((kind, area));
        }
        subs.push(cx.observe(&AppState::global(cx), |_, _, cx| cx.notify()));
        let java = JavaPicker::new(
            "settings-java",
            prefs(cx).launch.java.clone(),
            None,
            |choice, cx| write(cx, |s| s.launch.java = choice),
            cx,
        );
        Self {
            page: Page::General,
            fields,
            areas,
            java,
            sizes: None,
            _subs: subs,
        }
    }

    fn area(&self, kind: Text) -> &Entity<TextareaState> {
        &self
            .areas
            .iter()
            .find(|(k, _)| *k == kind)
            .expect("every long setting has an area")
            .1
    }

    fn field(&self, kind: Text) -> &Entity<InputState> {
        &self
            .fields
            .iter()
            .find(|(k, _)| *k == kind)
            .expect("every text setting has a field")
            .1
    }

    /// Back to the first page, for each visit to the settings.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.open(Page::General, cx);
    }

    fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        if page == Page::Folders {
            self.measure(cx);
        }
        cx.notify();
    }

    fn measure(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let sizes =
                runtime::blocking(|| FOLDERS.iter().map(|&f| (f, f.size())).collect::<Vec<_>>())
                    .await
                    .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.sizes = Some(sizes);
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_cache(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = Folder::Cache.path()
            && let Err(e) = std::fs::remove_dir_all(&dir)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("cannot clear {}: {e}", dir.display());
        }
        self.measure(cx);
    }

    fn render_nav(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let back = nav_row("settings-back", false, window, cx)
            .mb(px(10.))
            .text_color(c.muted)
            .child(icon(IconName::ArrowLeft, c.muted))
            .child(t!("settings.back").to_string())
            .on_click(|_, _, cx| {
                AppState::global(cx).update(cx, |s, cx| {
                    let home = s.home();
                    s.navigate(home, cx)
                });
            });
        let pages: Vec<_> = PAGES
            .iter()
            .map(|&page| {
                nav_row(
                    SharedString::from(format!("page-{}", page as u8)),
                    page == self.page,
                    window,
                    cx,
                )
                .child(page.title())
                .on_click(cx.listener(move |this, _, _, cx| this.open(page, cx)))
                .into_any_element()
            })
            .collect();
        let selected = PAGES.iter().position(|p| *p == self.page);
        let pages = ui::motion::highlighted(
            "settings-nav",
            v_flex().gap(px(2.)),
            pages,
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
            .child(back)
            .child(caption(t!("sidebar.settings"), cx))
            .child(pages)
            .child(div().flex_1())
            .child(
                div()
                    .px(px(8.))
                    .font_family(cx.theme().mono.clone())
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .child(format!(
                        "Riven {} · {}",
                        env!("CARGO_PKG_VERSION"),
                        env!("RIVEN_REV")
                    )),
            )
    }

    fn general(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = prefs(cx).clone();
        let system = t!("settings.system").to_string();
        let languages = vec![
            MenuItem::new("system", system.clone()),
            MenuItem::new("en-US", "English"),
            MenuItem::new("ru-RU", "Русский"),
        ];
        let modes = vec![
            MenuItem::new("system", system),
            MenuItem::new("light", t!("settings.theme_light").to_string()),
            MenuItem::new("dark", t!("settings.theme_dark").to_string()),
        ];
        let mode = match s.appearance.mode {
            ThemeMode::System => "system",
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        };
        vec![
            Section::new()
                .row(setting_row(
                    t!("settings.language").to_string(),
                    None,
                    Dropdown::new(
                        "language",
                        languages,
                        Some(s.language.clone().unwrap_or("system".into()).into()),
                        |v, _, cx| {
                            let lang = (v.as_ref() != "system").then(|| v.to_string());
                            super::set_language(lang.as_deref());
                            write(cx, |s| s.language = lang);
                            cx.refresh_windows();
                        },
                    ),
                    cx,
                ))
                .row(setting_row(
                    t!("settings.theme_mode").to_string(),
                    Some(t!("settings.theme_mode_hint").into()),
                    Dropdown::new("mode", modes, Some(mode.into()), |v, _, cx| {
                        let mode = match v.as_ref() {
                            "light" => ThemeMode::Light,
                            "dark" => ThemeMode::Dark,
                            _ => ThemeMode::System,
                        };
                        write(cx, |s| s.appearance.mode = mode);
                        reapply_theme(cx);
                    }),
                    cx,
                ))
                .row(setting_row(
                    t!("settings.animations").to_string(),
                    Some(t!("settings.animations_hint").into()),
                    Switch::new("animations", !s.reduce_motion)
                        .large()
                        .accessible(t!("settings.animations").to_string())
                        .on_change(|on, _, cx| {
                            write(cx, |s| s.reduce_motion = !on);
                            cx.set_reduce_motion(!on);
                        }),
                    cx,
                ))
                .into_any_element(),
            theme_picker(
                t!("settings.dark_theme").into(),
                theme::DARK,
                &s.appearance.dark,
                true,
                cx,
            ),
            theme_picker(
                t!("settings.light_theme").into(),
                theme::LIGHT,
                &s.appearance.light,
                false,
                cx,
            ),
        ]
    }

    fn java(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            Section::new()
                .row(setting_block(
                    t!("settings.java_path").to_string(),
                    Some(t!("settings.java_path_hint").into()),
                    self.java.clone(),
                    cx,
                ))
                .row(setting_block(
                    t!("settings.jvm_args").to_string(),
                    Some(t!("settings.jvm_args_hint").into()),
                    TextArea::new(self.area(Text::JvmArgs)),
                    cx,
                ))
                .into_any_element(),
            Section::new()
                .row(setting_row(
                    t!("settings.memory_min").to_string(),
                    Some(t!("settings.memory_hint").into()),
                    TextField::new(self.field(Text::MemoryMin)).w(px(120.)),
                    cx,
                ))
                .row(setting_row(
                    t!("settings.memory_max").to_string(),
                    Some(t!("settings.memory_max_hint").into()),
                    TextField::new(self.field(Text::MemoryMax)).w(px(120.)),
                    cx,
                ))
                .into_any_element(),
        ]
    }

    fn launch(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let fullscreen = prefs(cx).launch.window.fullscreen;
        vec![
            group_title(t!("settings.window").into(), cx),
            Section::new()
                .row(setting_row(
                    t!("settings.window_width").to_string(),
                    None,
                    TextField::new(self.field(Text::Width)).w(px(120.)),
                    cx,
                ))
                .row(setting_row(
                    t!("settings.window_height").to_string(),
                    None,
                    TextField::new(self.field(Text::Height)).w(px(120.)),
                    cx,
                ))
                .row(setting_row(
                    t!("settings.window_fullscreen").to_string(),
                    None,
                    Switch::new("fullscreen", fullscreen)
                        .large()
                        .accessible(t!("settings.window_fullscreen").to_string())
                        .on_change(|v, _, cx| write(cx, |s| s.launch.window.fullscreen = v)),
                    cx,
                ))
                .into_any_element(),
            group_title(t!("instance_settings.commands").into(), cx),
            Section::new()
                .row(setting_block(
                    t!("instance_settings.pre_launch").to_string(),
                    Some(t!("settings.pre_launch_hint").into()),
                    TextArea::new(self.area(Text::PreLaunch)),
                    cx,
                ))
                .row(setting_block(
                    t!("instance_settings.wrapper").to_string(),
                    Some(t!("settings.wrapper_hint").into()),
                    TextArea::new(self.area(Text::Wrapper)),
                    cx,
                ))
                .row(setting_block(
                    t!("instance_settings.post_exit").to_string(),
                    Some(t!("settings.post_exit_hint").into()),
                    TextArea::new(self.area(Text::PostExit)),
                    cx,
                ))
                .into_any_element(),
        ]
    }

    fn accounts(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors;
        let st = AppState::global(cx).read(cx);
        let accounts = st.accounts.accounts.clone();
        let active = st
            .settings
            .selected_account
            .clone()
            .or_else(|| accounts.first().map(|a| a.id.clone()));
        let mut section = Section::new();
        if accounts.is_empty() {
            section = section.row(
                div()
                    .px(px(18.))
                    .py(px(16.))
                    .text_color(c.muted)
                    .child(t!("accounts.none").to_string()),
            );
        }
        for (i, a) in accounts.iter().enumerate() {
            let is_active = active.as_deref() == Some(a.id.as_str());
            let select_id = a.id.clone();
            let remove_id = a.id.clone();
            let remove_name = a.name.clone();
            let kind = match a.kind {
                AccountKind::Microsoft => t!("accounts.microsoft"),
                AccountKind::Offline => t!("accounts.offline"),
            };
            section = section.row(
                h_flex()
                    .gap(px(12.))
                    .px(px(18.))
                    .py(px(12.))
                    .child(tile(initials(&a.name), 36., 6., cx).text_color(c.accent))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(W_SEMIBOLD).child(a.name.clone()))
                            .child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(12.))
                                    .text_color(c.muted)
                                    .child(kind.to_string()),
                            ),
                    )
                    .map(|row| {
                        if is_active {
                            row.child(
                                h_flex()
                                    .gap(px(6.))
                                    .text_color(c.ok)
                                    .child(icon(IconName::Check, c.ok).size(px(14.)))
                                    .child(t!("accounts.active").to_string()),
                            )
                        } else {
                            row.child(
                                Button::new(SharedString::from(format!("use-{i}")))
                                    .label(t!("accounts.use"))
                                    .on_click(move |_, _, cx| {
                                        let id = select_id.clone();
                                        write(cx, |s| s.selected_account = Some(id));
                                    }),
                            )
                        }
                    })
                    .child(
                        Button::new(SharedString::from(format!("remove-{i}")))
                            .ghost()
                            .icon(IconName::Trash)
                            .tooltip(t!("accounts.remove"))
                            .on_click(move |_, _, cx| {
                                super::dialogs::confirm_remove_account(
                                    remove_id.clone(),
                                    remove_name.clone(),
                                    cx,
                                )
                            }),
                    ),
            );
        }
        vec![
            section.into_any_element(),
            h_flex()
                .gap(px(8.))
                .child(
                    Button::new("settings-add-microsoft")
                        .primary()
                        .size(ButtonSize::Md)
                        .label(t!("accounts.add_microsoft"))
                        .on_click(|_, _, cx| super::dialogs::open_add_microsoft(cx)),
                )
                .child(
                    Button::new("settings-add-offline")
                        .size(ButtonSize::Md)
                        .label(t!("accounts.add_offline"))
                        .on_click(|_, window, cx| super::dialogs::open_add_offline(window, cx)),
                )
                .into_any_element(),
        ]
    }

    fn folders(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let c = theme.colors;
        let mono = theme.mono.clone();
        let view = cx.entity().downgrade();
        let mut section = Section::new();
        for folder in FOLDERS {
            let path = folder.path();
            let size = self
                .sizes
                .as_ref()
                .and_then(|s| s.iter().find(|(f, _)| *f == folder))
                .map(|(_, n)| human_size(*n));
            let open = path.clone();
            section = section.row(
                h_flex()
                    .gap(px(16.))
                    .px(px(18.))
                    .py(px(14.))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(
                                h_flex()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .font_weight(W_SEMIBOLD)
                                            .text_size(px(14.))
                                            .child(folder.title()),
                                    )
                                    .child(
                                        div()
                                            .text_color(c.muted)
                                            .child(size.unwrap_or_else(|| "…".into())),
                                    ),
                            )
                            .child(div().text_color(c.muted).child(folder.hint()))
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .text_color(c.text2)
                                    .truncate()
                                    .child(
                                        path.as_ref()
                                            .map(|p| p.display().to_string())
                                            .unwrap_or_default(),
                                    ),
                            ),
                    )
                    .when(folder == Folder::Cache, |row| {
                        let view = view.clone();
                        row.child(
                            Button::new("clear-cache")
                                .label(t!("folders.clear"))
                                .on_click(move |_, _, cx| {
                                    let view = view.clone();
                                    super::dialogs::confirm(
                                        t!("confirm.clear_cache_title"),
                                        t!("confirm.clear_cache_body"),
                                        t!("folders.clear"),
                                        move |_, cx| {
                                            let _ =
                                                view.update(cx, |this, cx| this.clear_cache(cx));
                                        },
                                        cx,
                                    );
                                }),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("open-{}", folder as u8)))
                            .label(t!("folders.open"))
                            .disabled(path.is_none())
                            .on_click(move |_, _, cx| {
                                if let Some(p) = &open {
                                    let _ = std::fs::create_dir_all(p);
                                    cx.open_with_system(p);
                                }
                            }),
                    ),
            );
        }
        vec![section.into_any_element()]
    }

    fn advanced(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let developer = prefs(cx).developer_mode;
        vec![
            Section::new()
                .row(setting_row(
                    t!("settings.developer_mode").to_string(),
                    Some(t!("settings.developer_mode_hint").into()),
                    Switch::new("developer", developer)
                        .large()
                        .accessible(t!("settings.developer_mode").to_string())
                        .on_change(|v, _, cx| write(cx, |s| s.developer_mode = v)),
                    cx,
                ))
                .into_any_element(),
        ]
    }

    fn about(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let c = theme.colors;
        let mono = theme.mono.clone();
        let facts = build_facts(window);
        let report = facts
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut section = Section::new();
        for (label, value) in facts {
            section = section.row(
                h_flex()
                    .gap(px(16.))
                    .px(px(18.))
                    .py(px(11.))
                    .child(div().flex_1().text_color(c.text2).child(label))
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_size(px(12.))
                            .truncate()
                            .child(value),
                    ),
            );
        }
        vec![
            h_flex()
                .gap(px(16.))
                .child(
                    tile("R".into(), 56., 12., cx)
                        .bg(c.accent)
                        .text_color(c.on_accent)
                        .text_size(px(24.)),
                )
                .child(
                    v_flex()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::BOLD)
                                .child("Riven Launcher"),
                        )
                        .child(
                            div()
                                .text_color(c.muted)
                                .child(t!("about.tagline").to_string()),
                        ),
                )
                .into_any_element(),
            section.into_any_element(),
            h_flex()
                .gap(px(8.))
                .child(
                    Button::new("source")
                        .size(ButtonSize::Md)
                        .icon(IconName::Code)
                        .label(t!("about.source"))
                        .on_click(|_, _, cx| cx.open_url(REPOSITORY)),
                )
                .child(
                    Button::new("copy-info")
                        .size(ButtonSize::Md)
                        .label(t!("about.copy"))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(report.clone()));
                            super::toast::show(
                                super::toast::ToastKind::Success,
                                t!("about.copied"),
                                cx,
                            );
                        }),
                )
                .into_any_element(),
        ]
    }
}

/// What a bug report needs to know about this build and machine.
fn build_facts(window: &Window) -> Vec<(String, String)> {
    let renderer = match window.gpu_specs() {
        Some(gpu) if gpu.is_software_emulated => {
            format!("{} ({})", gpu.device_name, t!("about.software"))
        }
        Some(gpu) => format!("{} · {}", gpu.device_name, gpu.driver_name),
        None if cfg!(target_os = "macos") => "Metal".into(),
        None if cfg!(target_os = "windows") => "Direct3D 11".into(),
        None => "wgpu".into(),
    };
    vec![
        (t!("about.version").into(), env!("CARGO_PKG_VERSION").into()),
        (t!("about.commit").into(), env!("RIVEN_REV").into()),
        (
            t!("about.build").into(),
            if cfg!(debug_assertions) {
                "debug".into()
            } else {
                "release".into()
            },
        ),
        (
            t!("about.system").into(),
            format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH),
        ),
        (
            t!("about.toolkit").into(),
            format!("gpui-kit {} (gpui-base)", env!("RIVEN_GPUI_KIT")),
        ),
        (t!("about.renderer").into(), renderer),
        (t!("about.license").into(), env!("CARGO_PKG_LICENSE").into()),
    ]
}

fn group_title(text: SharedString, cx: &App) -> AnyElement {
    div()
        .mb(px(-12.))
        .text_size(px(15.))
        .font_weight(W_SEMIBOLD)
        .text_color(cx.theme().colors.text)
        .child(text)
        .into_any_element()
}

/// One row of theme cards; picking a card also switches to its mode unless the system decides.
fn theme_picker(
    title: SharedString,
    list: &'static [theme::Spec],
    current: &str,
    dark: bool,
    cx: &App,
) -> AnyElement {
    let c = cx.theme().colors;
    let cards = list.iter().map(|spec| {
        let p = spec.palette();
        let selected = spec.id == current;
        let id = spec.id;
        let bar = |w: f32, h: f32, color| div().w(px(w)).h(px(h)).rounded(px(3.)).bg(color);
        v_flex()
            .id(SharedString::from(format!("theme-{id}")))
            .gap(px(8.))
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                write(cx, |s| {
                    if dark {
                        s.appearance.dark = id.into();
                    } else {
                        s.appearance.light = id.into();
                    }
                    if s.appearance.mode != ThemeMode::System {
                        s.appearance.mode = if dark {
                            ThemeMode::Dark
                        } else {
                            ThemeMode::Light
                        };
                    }
                });
                reapply_theme(cx);
            })
            .child(
                h_flex()
                    .w(px(150.))
                    .h(px(88.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .border_2()
                    .border_color(if selected { c.accent } else { c.border })
                    .bg(p.bg)
                    .child(
                        v_flex()
                            .w(px(46.))
                            .h_full()
                            .bg(p.panel)
                            .border_r_1()
                            .border_color(p.border)
                            .p(px(6.))
                            .gap(px(4.))
                            .child(bar(32., 6., p.sel))
                            .child(bar(24., 6., p.row))
                            .child(bar(28., 6., p.row)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .p(px(8.))
                            .gap(px(5.))
                            .child(bar(60., 7., p.text))
                            .child(bar(44., 5., p.muted))
                            .child(div().flex_1())
                            .child(
                                h_flex()
                                    .justify_end()
                                    .gap(px(4.))
                                    .child(div().size(px(8.)).rounded(px(4.)).bg(p.ok))
                                    .child(bar(30., 12., p.accent)),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_color(if selected { c.text } else { c.text2 })
                    .when(selected, |r| r.font_weight(W_SEMIBOLD))
                    .child(spec.name)
                    .when(selected, |r| {
                        r.child(icon(IconName::Check, c.accent).size(px(13.)))
                    }),
            )
    });
    v_flex()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(W_SEMIBOLD)
                .child(title),
        )
        .child(h_flex().flex_wrap().gap(px(14.)).children(cards))
        .into_any_element()
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cards = match self.page {
            Page::General => self.general(cx),
            Page::Java => self.java(cx),
            Page::Launch => self.launch(cx),
            Page::Accounts => self.accounts(cx),
            Page::Folders => self.folders(cx),
            Page::Advanced => self.advanced(cx),
            Page::About => self.about(window, cx),
        };
        let page_id = self.page as usize;
        let title = ui::motion::enter_nth(
            ("settings-title", page_id),
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::BOLD)
                .child(self.page.title()),
            6.,
            0,
            window,
            cx,
        );
        let cards: Vec<_> = cards
            .into_iter()
            .enumerate()
            .map(|(i, card)| {
                ui::motion::enter_nth(
                    ("settings-card", page_id * 100 + i),
                    div().child(card),
                    10.,
                    i + 1,
                    window,
                    cx,
                )
            })
            .collect();
        let page = v_flex()
            .max_w(px(820.))
            .px(px(36.))
            .py(px(28.))
            .gap(px(22.))
            .child(title)
            .children(cards);
        h_flex()
            .size_full()
            .items_start()
            .child(self.render_nav(window, cx))
            .child(
                div()
                    .id("settings-page")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .child(page),
            )
    }
}
