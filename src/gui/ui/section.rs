use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Div, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div, px, relative,
};

use super::{W_SEMIBOLD, h_flex, v_flex};
use crate::gui::theme::ActiveTheme as _;

/// A bordered card on the panel color whose rows are split by hairlines.
#[derive(IntoElement, Default)]
pub struct Section {
    rows: Vec<AnyElement>,
}

impl Section {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn row(mut self, row: impl IntoElement) -> Self {
        self.rows.push(row.into_any_element());
        self
    }
}

impl RenderOnce for Section {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.theme().colors;
        v_flex()
            .rounded(px(10.))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .children(self.rows.into_iter().enumerate().map(|(i, row)| {
                div()
                    .when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(row)
            }))
    }
}

/// A launcher setting: a title, an optional explanation, and its control on the right.
pub fn setting_row(
    title: impl Into<SharedString>,
    hint: Option<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> Div {
    let c = cx.theme().colors;
    h_flex()
        .gap(px(16.))
        .px(px(18.))
        .py(px(16.))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .font_weight(W_SEMIBOLD)
                        .text_size(px(14.))
                        .child(title.into()),
                )
                .when_some(hint, |col, hint| {
                    col.child(
                        div()
                            .mt(px(4.))
                            .text_color(c.muted)
                            .line_height(relative(1.5))
                            .child(hint),
                    )
                }),
        )
        .child(div().flex_none().child(control))
}

/// The small monospace heading over a list: "INSTANCES", "SETTINGS".
pub fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .px(px(8.))
        .pt(px(8.))
        .pb(px(4.))
        .font_family(theme.mono.clone())
        .text_size(px(11.))
        .text_color(theme.colors.muted)
        .child(text.into().to_uppercase())
}
