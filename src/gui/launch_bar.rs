use std::time::Duration;

use gpui_kit::base::{Popover, box_shadow};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, ease_in_out, hsla, px, relative,
};
use riven_format::{Account, AccountKind, Instance};
use riven_launch::game::Stage;
use rust_i18n::t;

use super::session::{Phase, Session};
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{
    self, Button, ButtonSize, IconName, UiText as _, W_SEMIBOLD, h_flex, icon, tile, v_flex,
};

/// Initials for an instance tile: "VideCraft: Create" → "VC".
pub fn initials(name: &str) -> SharedString {
    let letters: String = name
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect();
    letters.to_uppercase().into()
}

pub fn loader_display(kind: riven_format::LoaderKind) -> &'static str {
    match kind {
        riven_format::LoaderKind::Fabric => "Fabric",
        riven_format::LoaderKind::Quilt => "Quilt",
        riven_format::LoaderKind::Forge => "Forge",
        riven_format::LoaderKind::NeoForge => "NeoForge",
    }
}

pub fn loader_label(instance: &Instance) -> String {
    match &instance.loader {
        Some(loader) => format!(
            "{} · {} {}",
            instance.minecraft,
            loader_display(loader.kind),
            loader.version
        ),
        None => format!("{} · {}", instance.minecraft, t!("instance.vanilla")),
    }
}

fn selected_account(st: &AppState) -> Option<Account> {
    st.settings
        .selected_account
        .as_ref()
        .and_then(|a| st.accounts.accounts.iter().find(|x| &x.id == a))
        .or_else(|| st.accounts.accounts.first())
        .cloned()
}

fn stage_label(stage: Stage) -> String {
    match stage {
        Stage::Pack => t!("launch.stage.pack"),
        Stage::Metadata => t!("launch.stage.metadata"),
        Stage::Java => t!("launch.stage.java"),
        Stage::Game => t!("launch.stage.game"),
        Stage::Starting => t!("launch.stage.starting"),
    }
    .into()
}

fn megabytes(bytes: u64) -> String {
    format!("{:.0}", bytes as f64 / 1_048_576.)
}

fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A thin bar filling to `fraction`; with no total yet a short segment sweeps across instead.
fn progress_bar(id: &str, fraction: Option<f32>, window: &mut Window, cx: &mut App) -> AnyElement {
    let c = cx.theme().colors;
    let track = div()
        .relative()
        .w_full()
        .h(px(4.))
        .rounded(px(2.))
        .bg(c.sel)
        .overflow_hidden();
    match fraction {
        Some(f) => {
            let f = ui::motion::glide(
                SharedString::from(format!("progress:{id}")),
                f.clamp(0., 1.),
                window,
                cx,
            );
            track
                .child(div().h_full().w(relative(f)).rounded(px(2.)).bg(c.accent))
                .into_any_element()
        }
        None if cx.reduce_motion() => track
            .child(div().h_full().w(relative(0.3)).bg(c.accent.opacity(0.6)))
            .into_any_element(),
        None => track
            .child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .w(relative(0.3))
                    .rounded(px(2.))
                    .bg(c.accent)
                    .with_animation(
                        "sweep",
                        Animation::new(Duration::from_millis(1100))
                            .repeat()
                            .with_easing(ease_in_out),
                        |d, t| d.left(relative(-0.3 + 1.3 * t)),
                    ),
            )
            .into_any_element(),
    }
}

fn status(
    id: &str,
    phase: Option<Phase>,
    notice: Option<SharedString>,
    pack: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let c = cx.theme().colors;
    let line = |text: String, color| {
        div()
            .text_size(px(12.))
            .text_color(color)
            .truncate()
            .child(text)
    };
    let idle = match &pack {
        Some(version) => line(format!("{version} — {}", t!("launch.pack_current")), c.ok),
        None => line(t!("launch.no_pack").to_string(), c.muted),
    };
    let Some(phase) = phase else {
        return idle.into_any_element();
    };
    let busy = matches!(phase, Phase::Working { .. } | Phase::Running { .. });
    let body = match &phase {
        Phase::Working { stage, done, total } => {
            let fraction = (*total > 0).then(|| *done as f32 / *total as f32);
            let amount = match (stage, fraction) {
                (Stage::Java | Stage::Game, Some(_)) => {
                    format!("{} / {} MB", megabytes(*done), megabytes(*total))
                }
                (_, Some(f)) => format!("{:.0}%", f * 100.),
                _ => String::new(),
            };
            v_flex()
                .gap(px(6.))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(line(stage_label(*stage), c.text2).flex_1())
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .font_family(cx.theme().mono.clone())
                                .text_color(c.muted)
                                .child(amount),
                        ),
                )
                .child(progress_bar(id, fraction, window, cx))
                .into_any_element()
        }
        Phase::Running { since, .. } => line(
            t!("launch.running", time = clock(since.elapsed().as_secs())).into(),
            c.ok,
        )
        .into_any_element(),
        Phase::Failed(e) => line(e.to_string(), c.warn).into_any_element(),
        Phase::Finished { code: Some(code) } if !matches!(code, 0 | 130 | 137 | 143) => {
            line(t!("launch.crashed", code = code).into(), c.warn).into_any_element()
        }
        Phase::Finished { .. } => idle.into_any_element(),
    };
    v_flex()
        .min_w_0()
        .gap(px(4.))
        .child(ui::motion::enter(
            SharedString::from(format!("status:{id}:{}", phase_key(&phase))),
            div().child(body),
            4.,
            window,
            cx,
        ))
        .when_some(notice.filter(|_| !busy), |col, n| {
            col.child(line(n.to_string(), c.muted))
        })
        .into_any_element()
}

fn phase_key(phase: &Phase) -> &'static str {
    match phase {
        Phase::Working { .. } => "working",
        Phase::Running { .. } => "running",
        Phase::Finished { .. } => "finished",
        Phase::Failed(_) => "failed",
    }
}

fn play_button(
    state: &Entity<AppState>,
    id: &str,
    session: Option<&Session>,
    account: Option<Account>,
    cx: &App,
) -> AnyElement {
    let c = cx.theme().colors;
    let id = id.to_owned();
    let state = state.clone();
    match session.map(|s| &s.phase) {
        Some(Phase::Working { .. }) => Button::new("play")
            .primary()
            .size(ButtonSize::Lg)
            .disabled(true)
            .child(ui::motion::spinner(
                "play-spinner",
                icon(IconName::Loader, c.on_accent),
                cx,
            ))
            .label(t!("launch.preparing"))
            .into_any_element(),
        Some(Phase::Running { .. }) => Button::new("stop")
            .outline()
            .size(ButtonSize::Lg)
            .icon(IconName::Stop)
            .label(t!("launch.stop"))
            .on_click(move |_, _, cx| state.update(cx, |s, cx| s.stop(&id, cx)))
            .into_any_element(),
        _ => Button::new("play")
            .primary()
            .size(ButtonSize::Lg)
            .icon(IconName::Play)
            .label(t!("launch.play"))
            .on_click(move |_, window, cx| match account.clone() {
                Some(account) => state.update(cx, |s, cx| s.play(&id, account, cx)),
                None => super::dialogs::open_add_offline(window, cx),
            })
            .into_any_element(),
    }
}

/// The bar under every instance: what will start, its pack status, the account and Play.
pub fn render(
    state: &Entity<AppState>,
    id: &str,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let c = cx.theme().colors;
    let st = state.read(cx);
    let Some(instance) = st.instance(id).cloned() else {
        return div().into_any_element();
    };
    let pack = st.packs.get(id).cloned();
    let account = selected_account(st);
    let session = st.sessions.get(id);
    let phase = session.map(|s| s.phase.clone());
    let notice = session.and_then(|s| s.notice.clone());
    let status = status(id, phase, notice, pack, window, cx);
    let st = state.read(cx);
    let session = st.sessions.get(id);
    let button = play_button(state, id, session, account.clone(), cx);

    h_flex()
        .flex_none()
        .min_h(px(72.))
        .gap(px(20.))
        .px(px(18.))
        .py(px(12.))
        .border_t_1()
        .border_color(c.border)
        .bg(c.panel)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(px(12.))
                .child(
                    tile(initials(&instance.name), 44., 9., cx)
                        .text_color(c.accent)
                        .text_size(px(14.)),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(W_SEMIBOLD)
                                .text_size(px(14.))
                                .truncate()
                                .child(instance.name.clone()),
                        )
                        .child(
                            div()
                                .mt(px(2.))
                                .text_size(px(12.))
                                .text_color(c.muted)
                                .truncate()
                                .child(loader_label(&instance)),
                        ),
                ),
        )
        .child(div().flex_1().min_w_0().child(status))
        .child(
            h_flex()
                .flex_none()
                .gap(px(10.))
                .child(account_button(state, account.as_ref(), cx))
                .child(button),
        )
        .into_any_element()
}

fn account_button(
    state: &Entity<AppState>,
    account: Option<&Account>,
    cx: &App,
) -> impl IntoElement + use<> {
    let c = cx.theme().colors;
    let name: SharedString = match account {
        Some(a) => a.name.clone().into(),
        None => t!("accounts.add").into(),
    };
    let state = state.clone();
    let trigger = Button::new("account")
        .outline()
        .h(px(40.))
        .rounded(px(8.))
        .gap(px(8.))
        .child(
            tile(initials(&name), 22., 5., cx)
                .text_size(px(11.))
                .font_weight(gpui_kit::FontWeight::BOLD),
        )
        .child(div().child(name))
        .child(icon(IconName::ChevronUp, c.muted).size(px(12.)));
    Popover::new("accounts")
        .anchor(Anchor::BottomRight)
        .offset(px(10.))
        .trigger(trigger)
        .content(move |_, window, cx| {
            let popover = cx.entity();
            let panel = accounts_panel(&state, popover, window, cx);
            ui::motion::enter("accounts-in", panel, 10., window, cx)
        })
}

fn accounts_panel(
    state: &Entity<AppState>,
    popover: Entity<gpui_kit::base::PopoverState>,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::Div {
    let c = cx.theme().colors;
    let st = state.read(cx);
    let active_id = selected_account(st).map(|a| a.id);
    let accounts = st.accounts.accounts.clone();
    let close = popover.clone();
    v_flex()
        .ui_text(cx)
        .w(px(380.))
        .rounded(px(12.))
        .border_1()
        .border_color(c.border)
        .bg(c.row)
        .text_color(c.text)
        .overflow_hidden()
        .shadow(vec![box_shadow(0., 20., 50., 0., hsla(0., 0., 0., 0.5))])
        .child(
            h_flex()
                .px(px(16.))
                .py(px(14.))
                .border_b_1()
                .border_color(c.border)
                .child(
                    div()
                        .flex_1()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_size(px(15.))
                        .child(t!("accounts.title").to_string()),
                )
                .child(
                    Button::new("close-accounts")
                        .ghost()
                        .size(ButtonSize::Xs)
                        .icon(IconName::Close)
                        .tooltip(t!("common.close"))
                        .on_click(move |_, window, cx| {
                            close.update(cx, |p, cx| p.dismiss(window, cx))
                        }),
                ),
        )
        .child(
            v_flex()
                .p(px(8.))
                .when(accounts.is_empty(), |list| {
                    list.child(
                        div()
                            .p(px(10.))
                            .text_color(c.muted)
                            .child(t!("accounts.none").to_string()),
                    )
                })
                .children(accounts.iter().enumerate().map(|(i, a)| {
                    let active = active_id.as_deref() == Some(a.id.as_str());
                    let id = a.id.clone();
                    let select = state.clone();
                    let remove_id = a.id.clone();
                    let remove = state.clone();
                    let hover = ui::motion::hover(("account-hover", i), window, cx);
                    let bg = ui::motion::animate(
                        ("account-bg", i),
                        match (active, hover.on) {
                            (true, _) => c.sel,
                            (false, true) => c.bg,
                            _ => c.bg.opacity(0.),
                        },
                        window,
                        cx,
                    );
                    hover
                        .track(h_flex().id(SharedString::from(format!("account-{i}"))))
                        .gap(px(12.))
                        .p(px(10.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .bg(bg)
                        .on_click(move |_, _, cx| {
                            let id = id.clone();
                            select.update(cx, |s, cx| {
                                s.update_settings(|set| set.selected_account = Some(id), cx)
                            });
                        })
                        .child(tile(initials(&a.name), 36., 6., cx).map(|t| {
                            if active {
                                t.bg(c.muted.opacity(0.3))
                            } else {
                                t.text_color(c.text2)
                            }
                        }))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .font_weight(W_SEMIBOLD)
                                        .truncate()
                                        .child(a.name.clone()),
                                )
                                .child(
                                    div()
                                        .mt(px(2.))
                                        .text_size(px(12.))
                                        .text_color(c.muted)
                                        .child(match a.kind {
                                            AccountKind::Microsoft => {
                                                t!("accounts.microsoft").to_string()
                                            }
                                            AccountKind::Offline => {
                                                t!("accounts.offline").to_string()
                                            }
                                        }),
                                ),
                        )
                        .map(|r| {
                            if active {
                                r.child(icon(IconName::Check, c.ok).size(px(16.)))
                            } else {
                                r.child(
                                    Button::new(SharedString::from(format!("remove-{i}")))
                                        .ghost()
                                        .size(ButtonSize::Xs)
                                        .icon(IconName::Trash)
                                        .tooltip(t!("accounts.remove"))
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            let id = remove_id.clone();
                                            remove.update(cx, |s, cx| s.remove_account(&id, cx));
                                        }),
                                )
                            }
                        })
                })),
        )
        .child(
            v_flex()
                .gap(px(8.))
                .px(px(16.))
                .pt(px(12.))
                .pb(px(16.))
                .border_t_1()
                .border_color(c.border)
                .child(
                    Button::new("add-microsoft")
                        .primary()
                        .size(ButtonSize::Md)
                        .w_full()
                        .disabled(true)
                        .tooltip(t!("accounts.microsoft_pending"))
                        .label(t!("accounts.add_microsoft")),
                )
                .child(
                    Button::new("add-offline")
                        .outline()
                        .size(ButtonSize::Md)
                        .w_full()
                        .label(t!("accounts.add_offline"))
                        .on_click(move |_, window, cx| {
                            popover.update(cx, |p, cx| p.dismiss(window, cx));
                            super::dialogs::open_add_offline(window, cx)
                        }),
                ),
        )
}
