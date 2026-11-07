use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, WindowAppearance, div, img, px,
};
use gpui_base::input::{InputState, TextareaState};
use moonglass::theme::{self, ActiveTheme as _, ThemeMode};
use moonglass::toast::ToastKind;
use moonglass::{
    Button, Dropdown, IconName, MenuItem, Section, Switch, TextArea, TextField, W_SEMIBOLD,
    caption, h_flex, icon, motion, nav_row, setting_block, setting_row, v_flex,
};

use crate::app::{DOWNLOAD_URL, Demo, Route, SIDEBAR_WIDTH};
use crate::assets;
use crate::memory::MemorySlider;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Java,
    Launch,
    Accounts,
    About,
}

const PAGES: [Page; 5] = [
    Page::General,
    Page::Java,
    Page::Launch,
    Page::Accounts,
    Page::About,
];

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Java => "Java & memory",
            Page::Launch => "Launch",
            Page::Accounts => "Accounts",
            Page::About => "About",
        }
    }
}

/// The launcher settings the demo keeps while the page is open.
pub struct Prefs {
    mode: ThemeMode,
    dark: &'static str,
    light: &'static str,
    language: SharedString,
    java: SharedString,
    fullscreen: bool,
    memory: Entity<MemorySlider>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    jvm_args: Entity<TextareaState>,
}

impl Prefs {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let field = |value: &str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).default_value(value))
        };
        Self {
            mode: ThemeMode::Dark,
            dark: theme::DARK[0].id,
            light: theme::LIGHT[0].id,
            language: "en-US".into(),
            java: "auto".into(),
            fullscreen: false,
            memory: cx.new(MemorySlider::new),
            width: field("1280", window, cx),
            height: field("720", window, cx),
            jvm_args: moonglass::textarea(
                "-XX:+UseG1GC -XX:+ParallelRefProcEnabled -XX:MaxGCPauseMillis=200".into(),
                (2, 6),
                window,
                cx,
            ),
        }
    }
}

/// Switches palettes at once: moonglass fades through `Instant`, which the browser build lacks.
fn apply_theme(prefs: &Prefs, window: &Window, cx: &mut App) {
    let dark = match prefs.mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    let theme = cx.global_mut::<theme::Theme>();
    let (list, id) = if dark {
        (&theme.dark_themes, prefs.dark)
    } else {
        (&theme.light_themes, prefs.light)
    };
    let colors = list
        .iter()
        .find(|s| s.id == id)
        .unwrap_or(&list[0])
        .palette();
    theme.colors = colors;
    theme.dark = dark;
    gpui_base::Theme::global_mut(cx).appearance = if dark {
        gpui_base::ThemeAppearance::Dark
    } else {
        gpui_base::ThemeAppearance::Light
    };
    cx.refresh_windows();
}

fn group_title(text: &'static str) -> AnyElement {
    div()
        .mb(px(-12.))
        .text_size(px(15.))
        .font_weight(W_SEMIBOLD)
        .child(text)
        .into_any_element()
}

impl Demo {
    pub fn render_settings_nav(
        &self,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let back = nav_row("settings-back", false, window, cx)
            .mb(px(10.))
            .text_color(c.muted)
            .child(icon(IconName::ArrowLeft, c.muted))
            .child("Back")
            .on_click(cx.listener(|this, _, window, cx| {
                let home = Route::Instance(this.last_instance);
                this.navigate(home, window, cx)
            }));
        let rows: Vec<AnyElement> = PAGES
            .iter()
            .map(|&p| {
                nav_row(("page", p as usize), p == page, window, cx)
                    .child(p.title())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.navigate(Route::Settings(p), window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        let selected = PAGES.iter().position(|p| *p == page);
        let pages = motion::highlighted(
            "settings-nav",
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
            .child(back)
            .child(caption("SETTINGS", cx))
            .child(pages)
            .child(div().flex_1())
            .child(
                div()
                    .px(px(8.))
                    .font_family(cx.theme().mono.clone())
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .child(format!("Riven {} · demo", env!("CARGO_PKG_VERSION"))),
            )
    }

    pub fn render_settings(
        &self,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let cards = match page {
            Page::General => self.general(cx),
            Page::Java => self.java(cx),
            Page::Launch => self.launch_page(cx),
            Page::Accounts => self.accounts(cx),
            Page::About => self.about(cx),
        };
        let id = page as usize;
        let title = motion::enter_nth(
            ("settings-title", id),
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::BOLD)
                .child(page.title()),
            6.,
            0,
            window,
            cx,
        );
        let cards: Vec<_> = cards
            .into_iter()
            .enumerate()
            .map(|(i, card)| {
                motion::enter_nth(
                    ("settings-card", id * 100 + i),
                    div().child(card),
                    10.,
                    i + 1,
                    window,
                    cx,
                )
            })
            .collect();
        div()
            .id("settings-page")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .max_w(px(820.))
                    .px(px(36.))
                    .py(px(28.))
                    .gap(px(22.))
                    .child(title)
                    .children(cards),
            )
    }

    fn general(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let view = cx.entity().downgrade();
        let languages = vec![
            MenuItem::new("en-US", "English"),
            MenuItem::new("ru-RU", "Русский"),
        ];
        let modes = vec![
            MenuItem::new("system", "System"),
            MenuItem::new("light", "Light"),
            MenuItem::new("dark", "Dark"),
        ];
        let mode = match self.prefs.mode {
            ThemeMode::System => "system",
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        };
        let (lang_view, mode_view) = (view.clone(), view.clone());
        vec![
            Section::new()
                .row(setting_row(
                    "Language",
                    None,
                    Dropdown::new(
                        "language",
                        languages,
                        Some(self.prefs.language.clone()),
                        move |v, _, cx| {
                            if v.as_ref() != "en-US" {
                                moonglass::toast::show(
                                    ToastKind::Info,
                                    "The demo is in English; the app also speaks Russian.",
                                    cx,
                                );
                            }
                            let _ = lang_view.update(cx, |this, cx| {
                                this.prefs.language = v;
                                cx.notify();
                            });
                        },
                    ),
                    cx,
                ))
                .row(setting_row(
                    "Appearance",
                    Some("Light, dark, or the system's choice".into()),
                    Dropdown::new("mode", modes, Some(mode.into()), move |v, window, cx| {
                        let _ = mode_view.update(cx, |this, cx| {
                            this.prefs.mode = match v.as_ref() {
                                "light" => ThemeMode::Light,
                                "dark" => ThemeMode::Dark,
                                _ => ThemeMode::System,
                            };
                            apply_theme(&this.prefs, window, cx);
                        });
                    }),
                    cx,
                ))
                .row(setting_row(
                    "Animations",
                    Some("Transitions, list highlights and fades".into()),
                    Switch::new("animations", !cx.reduce_motion())
                        .large()
                        .accessible("Animations")
                        .on_change(|on, _, cx| cx.set_reduce_motion(!on)),
                    cx,
                ))
                .into_any_element(),
            self.theme_picker("Dark theme", theme::DARK, true, cx),
            self.theme_picker("Light theme", theme::LIGHT, false, cx),
        ]
    }

    fn theme_picker(
        &self,
        title: &'static str,
        list: &'static [theme::Spec],
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let current = if dark {
            self.prefs.dark
        } else {
            self.prefs.light
        };
        let cards = list.iter().map(|spec| {
            let p = spec.palette();
            let selected = spec.id == current;
            let id = spec.id;
            let bar = |w: f32, h: f32, color| div().w(px(w)).h(px(h)).rounded(px(3.)).bg(color);
            v_flex()
                .id(SharedString::from(format!("theme-{id}")))
                .gap(px(8.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if dark {
                        this.prefs.dark = id;
                        this.prefs.mode = ThemeMode::Dark;
                    } else {
                        this.prefs.light = id;
                        this.prefs.mode = ThemeMode::Light;
                    }
                    apply_theme(&this.prefs, window, cx);
                }))
                .child(
                    div()
                        .w(px(150.))
                        .h(px(88.))
                        .p(px(2.))
                        .rounded(px(10.))
                        .bg(if selected { c.accent } else { c.border })
                        .child(
                            h_flex()
                                .size_full()
                                .rounded(px(8.))
                                .bg(p.bg)
                                .child(
                                    v_flex()
                                        .w(px(46.))
                                        .h_full()
                                        .rounded_l(px(8.))
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

    fn java(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let view = cx.entity().downgrade();
        let javas = vec![
            MenuItem::new("auto", "Automatic: the Java each version needs"),
            MenuItem::new("25", "Java 25 · Temurin 25.0.1"),
            MenuItem::new("21", "Java 21 · Temurin 21.0.9"),
            MenuItem::new("17", "Java 17 · Temurin 17.0.17"),
        ];
        vec![
            Section::new()
                .row(setting_block(
                    "Java",
                    Some(
                        "Riven downloads the right Java on its own, or uses the one you pick"
                            .into(),
                    ),
                    Dropdown::new(
                        "java",
                        javas,
                        Some(self.prefs.java.clone()),
                        move |v, _, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.prefs.java = v;
                                cx.notify();
                            });
                        },
                    )
                    .width(px(420.)),
                    cx,
                ))
                .row(setting_block("Memory", None, self.prefs.memory.clone(), cx))
                .row(setting_block(
                    "JVM arguments",
                    Some("Added to every launch; an instance can override them".into()),
                    TextArea::new(&self.prefs.jvm_args),
                    cx,
                ))
                .into_any_element(),
        ]
    }

    fn launch_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let view = cx.entity().downgrade();
        vec![
            group_title("Game window"),
            Section::new()
                .row(setting_row(
                    "Width",
                    None,
                    TextField::new(&self.prefs.width).w(px(120.)),
                    cx,
                ))
                .row(setting_row(
                    "Height",
                    None,
                    TextField::new(&self.prefs.height).w(px(120.)),
                    cx,
                ))
                .row(setting_row(
                    "Fullscreen",
                    None,
                    Switch::new("fullscreen", self.prefs.fullscreen)
                        .large()
                        .accessible("Fullscreen")
                        .on_change(move |on, _, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.prefs.fullscreen = on;
                                cx.notify();
                            });
                        }),
                    cx,
                ))
                .into_any_element(),
        ]
    }

    fn accounts(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors;
        let account = h_flex()
            .gap(px(12.))
            .px(px(18.))
            .py(px(12.))
            .child(
                div()
                    .size(px(36.))
                    .flex_none()
                    .rounded(px(6.))
                    .bg(c.sel)
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_weight(FontWeight::BOLD)
                    .text_color(c.accent)
                    .child("S"),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(W_SEMIBOLD).child("Steve"))
                    .child(
                        div()
                            .mt(px(2.))
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .child("Offline account"),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_color(c.ok)
                    .child(icon(IconName::Check, c.ok).size(px(14.)))
                    .child("Active"),
            );
        vec![
            Section::new().row(account).into_any_element(),
            h_flex()
                .gap(px(10.))
                .child(
                    Button::new("add-microsoft")
                        .primary()
                        .icon(IconName::Plus)
                        .label("Sign in with Microsoft")
                        .on_click(|_, _, cx| {
                            moonglass::toast::show(
                                ToastKind::Info,
                                "Signing in with Microsoft happens in the app; tokens stay in your system keychain.",
                                cx,
                            )
                        }),
                )
                .child(
                    Button::new("add-offline")
                        .outline()
                        .icon(IconName::Plus)
                        .label("Offline account")
                        .on_click(|_, _, cx| {
                            moonglass::toast::show(
                                ToastKind::Info,
                                "Offline accounts are for playing without a license, in the app.",
                                cx,
                            )
                        }),
                )
                .into_any_element(),
        ]
    }

    fn about(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let c = theme.colors;
        let mono = theme.mono.clone();
        let facts = [
            ("Version", env!("CARGO_PKG_VERSION").to_owned()),
            ("Build", "web demo".to_owned()),
            ("UI stack", format!("moonglass {}", moonglass::VERSION)),
            ("License", "GPL-3.0-or-later".to_owned()),
        ];
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
                            .child(value),
                    ),
            );
        }
        vec![
            h_flex()
                .gap(px(16.))
                .child(img(assets::LOGO).size(px(56.)).flex_none())
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
                                .child("Minecraft launcher and modpack toolkit by BX Team"),
                        ),
                )
                .into_any_element(),
            section.into_any_element(),
            h_flex()
                .gap(px(10.))
                .child(
                    Button::new("about-download")
                        .primary()
                        .icon(IconName::Download)
                        .label("Download Riven")
                        .on_click(|_, _, cx| cx.open_url(DOWNLOAD_URL)),
                )
                .child(
                    Button::new("about-source")
                        .outline()
                        .icon(IconName::ExternalLink)
                        .label("Source code")
                        .on_click(|_, _, cx| cx.open_url("https://github.com/BX-Team/Riven")),
                )
                .into_any_element(),
        ]
    }
}
