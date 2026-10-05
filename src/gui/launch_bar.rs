use gpui_kit::base::{Popover, box_shadow};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, App, Entity, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, hsla, px,
};
use riven_format::{Account, AccountKind, Instance};
use rust_i18n::t;

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

/// The bar under every instance: what will start, its pack status, the account and Play.
pub fn render(
    state: &Entity<AppState>,
    id: &str,
    _: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let c = cx.theme().colors;
    let st = state.read(cx);
    let Some(instance) = st.instance(id).cloned() else {
        return div().into_any_element();
    };
    let pack = st.packs.get(id).cloned();
    let account = selected_account(st);

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
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .truncate()
                .map(|d| match &pack {
                    Some(version) => d
                        .text_color(c.ok)
                        .child(format!("{version} — {}", t!("launch.pack_current"))),
                    None => d
                        .text_color(c.muted)
                        .child(t!("launch.no_pack").to_string()),
                }),
        )
        .child(
            h_flex()
                .flex_none()
                .gap(px(10.))
                .child(account_button(state, account.as_ref(), cx))
                .child(
                    Button::new("play")
                        .primary()
                        .size(ButtonSize::Lg)
                        .icon(IconName::Play)
                        .label(t!("launch.play")),
                ),
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
