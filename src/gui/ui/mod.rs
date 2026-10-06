mod button;
mod dialog;
mod field;
mod icon;
mod menu;
pub mod motion;
mod section;
mod switch;
mod tabs;

pub use button::{Button, ButtonSize};
pub use dialog::{Modal, modal};
pub use field::{TextArea, TextField, textarea};
pub use icon::{IconName, icon};
pub use menu::{ActionMenu, Dropdown, MenuEntry, MenuItem};
pub use section::{Section, caption, setting_block, setting_row};
pub use switch::Switch;
pub use tabs::Tabs;

pub use gpui_kit::base::{h_flex, v_flex};

use gpui_kit::base::{Scrollbar, ScrollbarHandle};
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Div, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled, Window, div, px, relative,
};

use super::theme::ActiveTheme as _;

pub const W_SEMIBOLD: FontWeight = FontWeight(650.);
pub const W_HEAVY: FontWeight = FontWeight(750.);

/// The window's text style; popovers, dialogs and tooltips paint outside the tree and set it again.
pub trait UiText: Styled + Sized {
    fn ui_text(self, cx: &App) -> Self {
        let theme = cx.theme();
        self.font_family(theme.font.clone())
            .text_size(px(13.))
            .line_height(relative(1.4))
            .text_color(theme.colors.text)
    }
}

impl<T: Styled> UiText for T {}

struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let tip = gpui_kit::base::Tooltip::new("tip")
            .ui_text(cx)
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .text_size(px(12.))
            .child(self.0.clone());
        // gpui puts a tooltip 1 px from the pointer, under the cursor arrow; the gap clears it.
        div()
            .pt(px(18.))
            .pl(px(6.))
            .child(motion::enter("tip-in", tip, 3., window, cx))
    }
}

/// A builder for gpui's `.tooltip(...)` showing one line of text.
pub fn tooltip(text: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}

/// An avatar-like square with a few letters: instance tiles, account icons.
pub fn tile(text: SharedString, size: f32, radius: f32, cx: &App) -> gpui_kit::Div {
    let c = cx.theme().colors;
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(radius))
        .bg(c.sel)
        .flex()
        .items_center()
        .justify_center()
        .font_weight(FontWeight::EXTRA_BOLD)
        .child(text)
}

/// A vertical scrollbar laid over a scrolling element; place it in a `relative` parent.
pub fn scrollbar<H: ScrollbarHandle + Clone>(handle: &H) -> Div {
    div()
        .absolute()
        .inset_0()
        .child(Scrollbar::vertical(handle).viewport_from_layout())
}
