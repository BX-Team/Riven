use std::time::Duration;

use gpui_kit::base::box_shadow;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div, hsla, px, relative,
};

use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{Button, ButtonSize, IconName, UiText as _, h_flex, icon, motion, v_flex};

/// How many notifications stay on screen; older ones go first.
const VISIBLE: usize = 4;
const WIDTH: f32 = 360.;
/// The height of the buttons, which text and icon line up with.
const LINE: f32 = 28.;
const COPIED: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Info,
    Error,
}

impl ToastKind {
    fn lifetime(self) -> Duration {
        match self {
            ToastKind::Success => Duration::from_secs(4),
            ToastKind::Info => Duration::from_secs(6),
            ToastKind::Error => Duration::from_secs(10),
        }
    }
}

/// A notification shown in the corner of the window for a while.
#[derive(Debug, Clone)]
pub struct Toast {
    id: u64,
    kind: ToastKind,
    text: SharedString,
    /// The text was just copied: the copy button shows a check for a moment.
    copied: bool,
}

impl AppState {
    /// Shows a notification that goes away by itself.
    pub fn toast(
        &mut self,
        kind: ToastKind,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        if self.toasts.iter().any(|t| t.text == text && t.kind == kind) {
            return;
        }
        self.toast_serial += 1;
        let id = self.toast_serial;
        self.toasts.push(Toast {
            id,
            kind,
            text,
            copied: false,
        });
        if self.toasts.len() > VISIBLE {
            self.toasts.remove(0);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(kind.lifetime()).await;
            let _ = this.update(cx, |s, cx| s.dismiss_toast(id, cx));
        })
        .detach();
        cx.notify();
    }

    /// Copies a notification's text and confirms it on its copy button.
    pub fn copy_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(toast) = self.toasts.iter_mut().find(|t| t.id == id) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(toast.text.to_string()));
        toast.copied = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED).await;
            let _ = this.update(cx, |s, cx| {
                if let Some(t) = s.toasts.iter_mut().find(|t| t.id == id) {
                    t.copied = false;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toasts.retain(|t| t.id != id);
        cx.notify();
    }
}

/// Shows a notification from anywhere in the app.
pub fn show(kind: ToastKind, text: impl Into<SharedString>, cx: &mut App) {
    let text = text.into();
    AppState::global(cx).update(cx, |s, cx| s.toast(kind, text, cx));
}

/// The notification stack, laid over the bottom right corner; `bottom` clears the launch bar.
pub fn render(
    state: &Entity<AppState>,
    bottom: f32,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let toasts = state.read(cx).toasts.clone();
    if toasts.is_empty() {
        return div().into_any_element();
    }
    let c = cx.theme().colors;
    let items: Vec<AnyElement> = toasts
        .into_iter()
        .map(|toast| {
            let (mark, color) = match toast.kind {
                ToastKind::Success => (IconName::Check, c.ok),
                ToastKind::Info => (IconName::Terminal, c.accent),
                ToastKind::Error => (IconName::Close, super::theme::danger()),
            };
            let id = toast.id;
            let close = state.clone();
            let copy = state.clone();
            let card = h_flex()
                .w(px(WIDTH))
                .items_start()
                .gap(px(10.))
                .pl(px(12.))
                .pr(px(6.))
                .py(px(6.))
                .rounded(px(10.))
                .border_1()
                .border_color(c.border)
                .bg(c.panel)
                .shadow(vec![box_shadow(0., 8., 24., 0., hsla(0., 0., 0., 0.25))])
                .child(
                    h_flex()
                        .h(px(LINE))
                        .flex_none()
                        .child(icon(mark, color).size(px(14.))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .min_h(px(LINE))
                        .justify_center()
                        .child(
                            div()
                                .py(px(4.))
                                .line_height(relative(1.45))
                                .child(toast.text.clone()),
                        ),
                )
                .when(toast.kind == ToastKind::Error, |row| {
                    row.child(
                        Button::new(("toast-copy", id as usize))
                            .ghost()
                            .size(ButtonSize::Xs)
                            .icon(if toast.copied {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .on_click(move |_, _, cx| {
                                copy.update(cx, |s, cx| s.copy_toast(id, cx))
                            }),
                    )
                })
                .child(
                    Button::new(("toast-close", id as usize))
                        .ghost()
                        .size(ButtonSize::Xs)
                        .icon(IconName::Close)
                        .on_click(move |_, _, cx| {
                            close.update(cx, |s, cx| s.dismiss_toast(id, cx))
                        }),
                );
            motion::enter(("toast", id as usize), card, -6., window, cx)
        })
        .collect();
    v_flex()
        .id("toasts")
        .absolute()
        .right(px(16.))
        .bottom(px(bottom))
        .gap(px(8.))
        .items_end()
        .ui_text(cx)
        .occlude()
        .children(items)
        .into_any_element()
}
