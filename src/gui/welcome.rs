use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px, relative,
};
use riven_format::ThemeMode;
use rust_i18n::t;

use super::java_picker::JavaPicker;
use super::memory_slider::MemorySlider;
use super::new_instance::Mode;
use super::settings::{prefs, reapply_theme, theme_picker, write};
use super::state::AppState;
use super::theme::{self, ActiveTheme as _};
use super::ui::{
    Button, ButtonSize, Dropdown, IconName, MenuItem, W_SEMIBOLD, h_flex, icon, motion, v_flex,
};

const STEPS: usize = 4;
const WIDTH: f32 = 640.;

/// The first-run setup: language and look, Java and memory, an account, then where to start.
pub struct Welcome {
    step: usize,
    java: Entity<JavaPicker>,
    memory: Entity<MemorySlider>,
}

impl Welcome {
    fn new(cx: &mut Context<Self>) -> Self {
        let java = JavaPicker::new(
            "welcome-java",
            prefs(cx).launch.java.clone(),
            None,
            |choice, cx| write(cx, |s| s.launch.java = choice),
            cx,
        );
        let memory = MemorySlider::new(
            prefs(cx).launch.memory,
            |m, cx| write(cx, |s| s.launch.memory = m),
            cx,
        );
        Self {
            step: 0,
            java,
            memory,
        }
    }

    fn finish(cx: &mut App) {
        write(cx, |s| s.onboarded = true);
        AppState::global(cx).update(cx, |s, cx| s.close_modal(cx));
    }

    /// Brings the setup back after an account dialog, on the account step.
    fn back_here(this: Entity<Self>) -> super::dialogs::Back {
        Rc::new(move |cx| super::dialogs::open(this.clone().into(), WIDTH, cx))
    }

    fn render_look(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = prefs(cx).clone();
        let system = t!("settings.system").to_string();
        let languages = vec![
            MenuItem::new("system", system.clone()),
            MenuItem::new("en-US", "English"),
            MenuItem::new("ru-RU", "Русский"),
        ];
        let mode = s.appearance.mode;
        let dark = match mode {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::System => cx.theme().dark,
        };
        let mode_button = |id: &'static str, label: SharedString, value: ThemeMode| {
            Button::new(id)
                .label(label)
                .when(mode == value, |b| b.primary())
                .on_click(move |_, _, cx| {
                    write(cx, |s| s.appearance.mode = value);
                    reapply_theme(cx);
                })
        };
        v_flex()
            .gap(px(16.))
            .child(super::dialogs::field(
                t!("settings.language"),
                Dropdown::new(
                    "welcome-language",
                    languages,
                    Some(s.language.clone().unwrap_or("system".into()).into()),
                    |v, _, cx| {
                        let lang = (v.as_ref() != "system").then(|| v.to_string());
                        super::set_language(lang.as_deref());
                        write(cx, |s| s.language = lang);
                        cx.refresh_windows();
                    },
                )
                .width(px(260.)),
                cx,
            ))
            .child(super::dialogs::field(
                t!("settings.theme_mode"),
                h_flex()
                    .gap(px(8.))
                    .child(mode_button(
                        "welcome-system",
                        system.into(),
                        ThemeMode::System,
                    ))
                    .child(mode_button(
                        "welcome-light",
                        t!("settings.theme_light").into(),
                        ThemeMode::Light,
                    ))
                    .child(mode_button(
                        "welcome-dark",
                        t!("settings.theme_dark").into(),
                        ThemeMode::Dark,
                    )),
                cx,
            ))
            .child(if dark {
                theme_picker(
                    t!("settings.dark_theme").into(),
                    theme::DARK,
                    &s.appearance.dark,
                    true,
                    cx,
                )
            } else {
                theme_picker(
                    t!("settings.light_theme").into(),
                    theme::LIGHT,
                    &s.appearance.light,
                    false,
                    cx,
                )
            })
            .into_any_element()
    }

    fn render_java(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap(px(16.))
            .child(super::dialogs::field(
                t!("welcome.java"),
                div().child(self.java.clone()),
                cx,
            ))
            .child(super::dialogs::field(
                t!("welcome.memory"),
                div().child(self.memory.clone()),
                cx,
            ))
            .into_any_element()
    }

    fn render_account(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let accounts = AppState::global(cx).read(cx).accounts.accounts.clone();
        let microsoft = Self::back_here(cx.entity());
        let offline = microsoft.clone();
        v_flex()
            .gap(px(12.))
            .child(
                div()
                    .text_color(c.text2)
                    .line_height(relative(1.5))
                    .child(t!("welcome.account_hint").to_string()),
            )
            .children(accounts.iter().map(|a| {
                h_flex()
                    .gap(px(10.))
                    .w(px(300.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.bg)
                    .child(
                        super::launch_bar::account_tile(&a.name, Some(&a.id), 26., 5., cx)
                            .text_size(px(11.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(W_SEMIBOLD)
                            .child(a.name.clone()),
                    )
                    .child(icon(IconName::Check, c.ok).size(px(14.)))
            }))
            .child(
                Button::new("welcome-microsoft")
                    .when(accounts.is_empty(), |b| b.primary())
                    .when(!accounts.is_empty(), |b| b.outline())
                    .size(ButtonSize::Md)
                    .w(px(300.))
                    .label(t!("accounts.add_microsoft"))
                    .on_click(move |_, _, cx| {
                        super::dialogs::open_add_microsoft_then(Some(microsoft.clone()), cx)
                    }),
            )
            .child(
                Button::new("welcome-offline")
                    .outline()
                    .size(ButtonSize::Md)
                    .w(px(300.))
                    .label(t!("accounts.add_offline"))
                    .on_click(move |_, window, cx| {
                        super::dialogs::open_add_offline_then(Some(offline.clone()), window, cx)
                    }),
            )
            .into_any_element()
    }

    fn render_ready(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let st = AppState::global(cx).read(cx);
        let name = super::launch_bar::selected_account(st).map(|a| a.name);
        let cards = [
            (
                Mode::Modrinth,
                IconName::Compass,
                t!("welcome.start_modrinth"),
                t!("welcome.start_modrinth_hint"),
            ),
            (
                Mode::Custom,
                IconName::Plus,
                t!("welcome.start_custom"),
                t!("welcome.start_custom_hint"),
            ),
            (
                Mode::Import,
                IconName::Package,
                t!("welcome.start_import"),
                t!("welcome.start_import_hint"),
            ),
        ];
        v_flex()
            .gap(px(14.))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::BOLD)
                            .child(match name {
                                Some(name) => t!("welcome.hello", name = name).to_string(),
                                None => t!("welcome.hello_anon").to_string(),
                            }),
                    )
                    .child(
                        div()
                            .text_color(c.text2)
                            .child(t!("welcome.where_to_start").to_string()),
                    ),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap(px(10.))
                    .children(cards.into_iter().enumerate().map(
                        |(i, (mode, glyph, title, hint))| {
                            let hover = motion::hover(("welcome-card", i), window, cx);
                            let border = motion::animate(
                                ("welcome-card-border", i),
                                if hover.on { c.accent } else { c.border },
                                window,
                                cx,
                            );
                            hover
                                .track(div().id(("welcome-start", i)))
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .p(px(14.))
                                .rounded(px(10.))
                                .border_1()
                                .border_color(border)
                                .bg(c.bg)
                                .cursor_pointer()
                                .on_click(move |_, window, cx| {
                                    write(cx, |s| s.onboarded = true);
                                    super::new_instance::open_mode(mode, window, cx);
                                })
                                .child(
                                    v_flex()
                                        .gap(px(8.))
                                        .child(icon(glyph, c.accent).size(px(20.)))
                                        .child(
                                            div().font_weight(W_SEMIBOLD).child(title.to_string()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(c.muted)
                                                .line_height(relative(1.45))
                                                .child(hint.to_string()),
                                        ),
                                )
                        },
                    )),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .line_height(relative(1.45))
                    .child(t!("welcome.developer_tip").to_string()),
            )
            .into_any_element()
    }
}

impl Render for Welcome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let (title, body) = match self.step {
            0 => (t!("welcome.look"), self.render_look(cx)),
            1 => (t!("welcome.java_title"), self.render_java(cx)),
            2 => (t!("welcome.account"), self.render_account(cx)),
            _ => (t!("welcome.ready"), self.render_ready(window, cx)),
        };
        let dots = h_flex().gap(px(6.)).children((0..STEPS).map(|i| {
            div()
                .w(px(if i == self.step { 18. } else { 6. }))
                .h(px(6.))
                .rounded(px(3.))
                .bg(if i <= self.step { c.accent } else { c.border })
        }));
        let last = self.step + 1 == STEPS;
        let mut buttons = Vec::new();
        if !last {
            buttons.push(
                Button::new("welcome-skip")
                    .ghost()
                    .size(ButtonSize::Md)
                    .label(t!("welcome.skip"))
                    .on_click(|_, _, cx| Self::finish(cx)),
            );
        }
        if self.step > 0 {
            buttons.push(
                Button::new("welcome-back")
                    .outline()
                    .size(ButtonSize::Md)
                    .label(t!("welcome.back"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.step -= 1;
                        cx.notify();
                    })),
            );
        }
        buttons.push(if last {
            Button::new("welcome-done")
                .primary()
                .size(ButtonSize::Md)
                .label(t!("welcome.done"))
                .on_click(|_, _, cx| Self::finish(cx))
        } else {
            Button::new("welcome-next")
                .primary()
                .size(ButtonSize::Md)
                .label(t!("welcome.next"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.step += 1;
                    cx.notify();
                }))
        });
        let head = h_flex()
            .gap(px(14.))
            .child(gpui_kit::img(super::assets::LOGO).size(px(40.)).flex_none())
            .child(
                v_flex()
                    .flex_1()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(16.))
                            .child(t!("welcome.title").to_string()),
                    )
                    .child(div().text_color(c.muted).child(title.to_string())),
            )
            .child(dots);
        v_flex()
            .child(
                v_flex()
                    .gap(px(18.))
                    .px(px(22.))
                    .pt(px(20.))
                    .pb(px(20.))
                    .child(head)
                    .child(body),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
                    .px(px(22.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(c.border)
                    .children(buttons),
            )
    }
}

/// Shows the setup on a launcher that has not finished it yet.
pub fn open_if_new(cx: &mut App) {
    if prefs(cx).onboarded {
        return;
    }
    let view = cx.new(Welcome::new);
    super::dialogs::open(view.into(), WIDTH, cx);
}
