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
use moonglass::theme::ActiveTheme as _;
use moonglass::{
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

/// An account's picture: the head of its skin once fetched, or its initials on a tile.
pub fn account_tile(
    name: &str,
    id: Option<&str>,
    size: f32,
    radius: f32,
    cx: &App,
) -> gpui_kit::Div {
    let skin = id.and_then(|id| AppState::global(cx).read(cx).skins.get(id).cloned());
    match skin {
        Some(path) => div()
            .size(px(size))
            .flex_none()
            .rounded(px(radius))
            .overflow_hidden()
            .child(gpui_kit::img(path).size_full()),
        None => tile(initials(name), size, radius, cx),
    }
}

/// An instance's picture: its icon, or its initials on a tile.
pub fn instance_tile(
    name: &str,
    icon: Option<&std::path::PathBuf>,
    size: f32,
    radius: f32,
    cx: &App,
) -> AnyElement {
    match icon {
        Some(path) => gpui_kit::img(path.clone())
            .size(px(size))
            .flex_none()
            .rounded(px(radius))
            .into_any_element(),
        None => tile(initials(name), size, radius, cx)
            .text_color(cx.theme().colors.accent)
            .text_size(px((size * 0.32).max(11.)))
            .into_any_element(),
    }
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

/// "3 h 12 min played · yesterday", or none for an instance never started.
pub fn played(instance: &Instance) -> Option<String> {
    if instance.play_seconds == 0 && instance.last_played.is_none() {
        return None;
    }
    let minutes = instance.play_seconds / 60;
    let total = match minutes {
        0 => t!("time.under_minute"),
        1..60 => t!("time.minutes", m = minutes),
        _ => t!("time.hours_minutes", h = minutes / 60, m = minutes % 60),
    };
    let last = instance
        .last_played
        .as_deref()
        .and_then(riven_launch::parse_rfc3339)
        .map(|secs| super::time::ago(std::time::UNIX_EPOCH + Duration::from_secs(secs)));
    Some(match last {
        Some(last) => t!("launch.played_last", total = total, last = last).into(),
        None => t!("launch.played", total = total).into(),
    })
}

pub(super) fn selected_account(st: &AppState) -> Option<Account> {
    st.settings
        .selected_account
        .as_ref()
        .and_then(|a| st.accounts.accounts.iter().find(|x| &x.id == a))
        .or_else(|| st.accounts.accounts.first())
        .cloned()
}

pub(super) fn stage_label(stage: Stage) -> String {
    match stage {
        Stage::SignIn => t!("launch.stage.sign_in"),
        Stage::Pack => t!("launch.stage.pack"),
        Stage::Convert => t!("launch.stage.convert"),
        Stage::PackDownload => t!("launch.stage.pack_download"),
        Stage::PackInstall => t!("launch.stage.pack_install"),
        Stage::Metadata => t!("launch.stage.metadata"),
        Stage::Java => t!("launch.stage.java"),
        Stage::Game => t!("launch.stage.game"),
        Stage::Loader => t!("launch.stage.loader"),
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
pub(super) fn progress_bar(
    id: &str,
    fraction: Option<f32>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
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
            let f = moonglass::motion::glide(
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

/// Content files being read, while nothing else runs.
fn scanning(id: &str, done: usize, total: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let c = cx.theme().colors;
    let fraction = (total > 0).then(|| done as f32 / total as f32);
    v_flex()
        .min_w_0()
        .gap(px(6.))
        .child(
            h_flex()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.))
                        .text_color(c.text2)
                        .truncate()
                        .child(t!("launch.scanning").to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .font_family(cx.theme().mono.clone())
                        .text_color(c.muted)
                        .child(format!("{done} / {total}")),
                ),
        )
        .child(progress_bar(id, fraction, window, cx))
        .into_any_element()
}

/// The install's progress as a card for the middle of an instance that has no content yet.
pub(super) fn progress_card(id: &str, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let c = cx.theme().colors;
    let Phase::Working { stage, done, total } = AppState::global(cx)
        .read(cx)
        .sessions
        .get(id)?
        .phase
        .clone()
    else {
        return None;
    };
    let fraction = (total > 0).then(|| done as f32 / total as f32);
    Some(
        v_flex()
            .w(px(420.))
            .gap(px(12.))
            .p(px(22.))
            .rounded(px(12.))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(moonglass::motion::spinner(
                        "install-card-spinner",
                        icon(IconName::Loader, c.accent).size(px(16.)),
                        cx,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(W_SEMIBOLD)
                            .text_size(px(15.))
                            .child(stage_label(stage)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .font_family(cx.theme().mono.clone())
                            .text_color(c.muted)
                            .child(amount(stage, done, total)),
                    ),
            )
            .child(progress_bar(id, fraction, window, cx))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .child(t!("launch.first_install_hint").to_string()),
            )
            .into_any_element(),
    )
}

/// What a stage got through, in its own unit.
fn amount(stage: Stage, done: u64, total: u64) -> String {
    match stage {
        _ if total == 0 => String::new(),
        Stage::Java | Stage::Game => format!("{} / {} MB", megabytes(done), megabytes(total)),
        Stage::PackDownload | Stage::PackInstall | Stage::Loader => format!("{done} / {total}"),
        _ => format!("{:.0}%", done as f32 / total as f32 * 100.),
    }
}

/// `pack` is the installed version, a newer one and what the last update changed;
/// `notice` and `crash` come from the last session.
fn status(
    id: &str,
    phase: Option<Phase>,
    (notice, crash): (Option<SharedString>, Option<riven_launch::crash::Diagnosis>),
    pack: Option<(String, Option<String>, Option<riven_format::Changes>)>,
    played: Option<String>,
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
    let idle = match (pack, played) {
        (Some((version, Some(newer), _)), _) => line(
            format!("{version} — {}", t!("launch.pack_newer", version = newer)),
            c.accent,
        )
        .into_any_element(),
        (Some((version, None, changes)), _) => h_flex()
            .min_w_0()
            .gap(px(8.))
            .child(line(
                format!("{version} — {}", t!("launch.pack_current")),
                c.ok,
            ))
            .when_some(changes, |row, changes| {
                row.child(
                    Button::new(SharedString::from(format!("pack-changes-{id}")))
                        .ghost()
                        .size(ButtonSize::Xs)
                        .label(t!("changes.open"))
                        .on_click(move |_, _, cx| {
                            super::changes::open(version.clone(), changes.clone(), cx)
                        }),
                )
            })
            .into_any_element(),
        (None, Some(played)) => line(played, c.muted).into_any_element(),
        (None, None) => div().into_any_element(),
    };
    let Some(phase) = phase else {
        return idle;
    };
    let busy = matches!(phase, Phase::Working { .. } | Phase::Running { .. });
    let body = match &phase {
        Phase::Working { stage, done, total } => {
            let fraction = (*total > 0).then(|| *done as f32 / *total as f32);
            let amount = amount(*stage, *done, *total);
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
        Phase::Finished { .. } if let Some(crash) = &crash => {
            super::crash::status_line(id, crash, cx)
        }
        Phase::Finished { code: Some(code) } if riven_launch::game::crashed(Some(*code)) => {
            line(t!("launch.crashed", code = code).into(), c.warn).into_any_element()
        }
        Phase::Finished { .. } => idle,
    };
    v_flex()
        .min_w_0()
        .gap(px(4.))
        .child(moonglass::motion::enter(
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
        Some(Phase::Working { .. }) if session.is_some_and(Session::cancellable) => {
            Button::new("cancel")
                .outline()
                .size(ButtonSize::Lg)
                .icon(IconName::Close)
                .label(t!("launch.cancel"))
                .on_click(move |_, _, cx| state.update(cx, |s, _| s.cancel(&id)))
                .into_any_element()
        }
        Some(Phase::Working { .. }) => Button::new("play")
            .primary()
            .size(ButtonSize::Lg)
            .disabled(true)
            .child(moonglass::motion::spinner(
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
            .on_click(move |_, _, cx| {
                let (state, id) = (state.clone(), id.clone());
                super::dialogs::confirm(
                    t!("confirm.stop_title"),
                    t!("confirm.stop_body"),
                    t!("launch.stop"),
                    move |_, cx| state.update(cx, |s, cx| s.stop(&id, cx)),
                    cx,
                );
            })
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
/// `centered` leaves the install's progress to the instance view, which shows it larger.
pub fn render(
    state: &Entity<AppState>,
    id: &str,
    centered: bool,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let c = cx.theme().colors;
    let st = state.read(cx);
    let Some(instance) = st.instance(id).cloned() else {
        return div().into_any_element();
    };
    let pack = st.packs.get(id).cloned();
    let changes = st.pack_changes.get(id).cloned();
    let newer = st.modpack_updates.get(id).map(|v| v.number.clone());
    let account = selected_account(st);
    let session = st.sessions.get(id);
    let phase = session
        .map(|s| s.phase.clone())
        .filter(|p| !(centered && matches!(p, Phase::Working { .. })));
    let notice = session.and_then(|s| s.notice.clone());
    let crash = session.and_then(|s| s.crash.clone());
    let scan = st.scans.get(id).copied();
    let busy = phase
        .as_ref()
        .is_some_and(|p| matches!(p, Phase::Working { .. } | Phase::Running { .. }));
    let status = match scan {
        Some((done, total)) if !busy => scanning(id, done, total, window, cx),
        _ => status(
            id,
            phase,
            (notice, crash),
            pack.map(|p| (p, newer.clone(), changes)),
            played(&instance),
            window,
            cx,
        ),
    };
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
                .child(instance_tile(&instance.name, st.icons.get(id), 44., 9., cx))
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
                .when(newer.is_some() && !busy, |row| {
                    let state = state.clone();
                    let id = id.to_owned();
                    row.child(
                        Button::new("update-pack")
                            .outline()
                            .h(px(40.))
                            .rounded(px(8.))
                            .icon(IconName::Refresh)
                            .label(t!("launch.update_pack"))
                            .on_click(move |_, _, cx| {
                                state.update(cx, |s, cx| s.update_modpack(&id, cx))
                            }),
                    )
                })
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
            account_tile(&name, account.map(|a| a.id.as_str()), 22., 5., cx)
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
            moonglass::motion::enter("accounts-in", panel, 10., window, cx)
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
                    let remove_name = a.name.clone();
                    let dismiss = popover.clone();
                    let picked = popover.clone();
                    let hover = moonglass::motion::hover(("account-hover", i), window, cx);
                    let bg = moonglass::motion::animate(
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
                        .on_click(move |_, window, cx| {
                            let id = id.clone();
                            select.update(cx, |s, cx| {
                                s.update_settings(|set| set.selected_account = Some(id), cx)
                            });
                            picked.update(cx, |p, cx| p.dismiss(window, cx));
                        })
                        .child(account_tile(&a.name, Some(&a.id), 36., 6., cx).map(|t| {
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
                                        .on_click(move |_, window, cx| {
                                            cx.stop_propagation();
                                            dismiss.update(cx, |p, cx| p.dismiss(window, cx));
                                            super::dialogs::confirm_remove_account(
                                                remove_id.clone(),
                                                remove_name.clone(),
                                                cx,
                                            );
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
                        .label(t!("accounts.add_microsoft"))
                        .on_click({
                            let popover = popover.clone();
                            move |_, window, cx| {
                                popover.update(cx, |p, cx| p.dismiss(window, cx));
                                super::dialogs::open_add_microsoft(cx)
                            }
                        }),
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
