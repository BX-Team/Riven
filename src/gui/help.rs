use gpui_kit::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, div, px,
};
use rust_i18n::t;

use moonglass::theme::ActiveTheme as _;

const DOCS: &str = "https://bxteam.org/docs/riven";

/// A round "?" that opens `page` of the documentation, such as `cli/authoring#riven-add`.
pub fn button(id: &'static str, page: &'static str, cx: &App) -> impl IntoElement + use<> {
    let c = cx.theme().colors;
    div()
        .id(id)
        .flex_none()
        .size(px(18.))
        .rounded_full()
        .border_1()
        .border_color(c.border)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .text_color(c.muted)
        .cursor_pointer()
        .hover(|s| s.bg(c.row).text_color(c.text))
        .tooltip(moonglass::tooltip(t!("help.open_docs").into()))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.open_url(&format!("{DOCS}/{page}"));
        })
        .child("?")
}
