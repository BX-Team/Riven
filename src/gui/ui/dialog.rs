use std::rc::Rc;

use gpui_kit::base::{Dialog, DialogPopup, box_shadow};
use gpui_kit::{
    AnyView, App, FocusHandle, IntoElement, ParentElement as _, Pixels, Styled as _, Window, div,
    hsla, px,
};

use super::{UiText as _, motion};
use crate::gui::theme::ActiveTheme as _;

/// A view shown over the window until it closes itself or Escape or the backdrop dismisses it.
#[derive(Clone)]
pub struct Modal {
    pub view: AnyView,
    pub width: Pixels,
    pub focus: FocusHandle,
    /// Tells one opening from the next, so a late close cannot drop a newer modal.
    pub serial: u64,
    /// Set while the modal fades out.
    pub closing: bool,
}

/// The modal host: a dimmed backdrop and the view in a raised card.
pub fn modal(
    m: &Modal,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let c = cx.theme().colors;
    let t = motion::leave(("modal", m.serial as usize), m.closing, window, cx);
    let dismiss = Rc::new(on_dismiss);
    Dialog::new(cx)
        .open(true)
        .focus_handle(m.focus.clone())
        .close_on_escape(true)
        .close_on_backdrop_press(true)
        .backdrop(div().size_full().bg(hsla(0., 0., 0., 0.45)).opacity(t))
        .on_ok(|_, _, _| false)
        .on_cancel(|_, _, _| true)
        .on_close(move |_, window, cx| dismiss(window, cx))
        .child(
            DialogPopup::new()
                .ui_text(cx)
                .w(m.width.min(window.viewport_size().width - px(32.)))
                .rounded(px(12.))
                .border_1()
                .border_color(c.border)
                .bg(c.panel)
                .text_color(c.text)
                .shadow(vec![box_shadow(0., 20., 50., 0., hsla(0., 0., 0., 0.5))])
                .relative()
                .opacity(t)
                .top(px((1. - t) * 12.))
                .child(m.view.clone()),
        )
}
