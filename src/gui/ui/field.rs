use gpui_kit::base::StyledExt as _;
use gpui_kit::base::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Entity, Focusable as _, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, RenderOnce, StyleRefinement, Styled, Window, div, px,
};

use super::{IconName, h_flex, icon, motion};
use crate::gui::theme::ActiveTheme as _;

/// A 30 px bordered text field around a gpui-base input.
#[derive(IntoElement)]
pub struct TextField {
    state: Entity<InputState>,
    leading: Option<IconName>,
    style: StyleRefinement,
}

impl TextField {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            leading: None,
            style: StyleRefinement::default(),
        }
    }

    pub fn leading(mut self, icon: IconName) -> Self {
        self.leading = Some(icon);
        self
    }
}

impl Styled for TextField {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.theme().colors;
        let focus = self.state.read(cx).focus_handle(cx);
        let focused = focus.is_focused(window);
        let border = motion::animate(
            ("field-border", self.state.entity_id().as_u64() as usize),
            if focused { c.accent } else { c.border },
            window,
            cx,
        );
        h_flex()
            .h(px(30.))
            .px(px(10.))
            .gap(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(border)
            .bg(c.bg)
            .text_color(c.text)
            .cursor_text()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx);
            })
            .when_some(self.leading, |row, name| {
                row.child(icon(name, c.muted).size(px(14.)))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(18.))
                    .line_height(px(18.))
                    .child(Input::new(&self.state)),
            )
            .refine_style(&self.style)
    }
}

/// A bordered multi-line field that grows with its text, for commands and JVM arguments.
#[derive(IntoElement)]
pub struct TextArea {
    state: Entity<TextareaState>,
}

impl TextArea {
    pub fn new(state: &Entity<TextareaState>) -> Self {
        Self {
            state: state.clone(),
        }
    }
}

impl RenderOnce for TextArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let c = theme.colors;
        let mono = theme.mono.clone();
        let focus = self.state.read(cx).focus_handle(cx);
        let focused = focus.is_focused(window);
        let border = motion::animate(
            ("area-border", self.state.entity_id().as_u64() as usize),
            if focused { c.accent } else { c.border },
            window,
            cx,
        );
        div()
            .w_full()
            .px(px(10.))
            .py(px(6.))
            .rounded(px(6.))
            .border_1()
            .border_color(border)
            .bg(c.bg)
            .text_color(c.text)
            .font_family(mono)
            .text_size(px(12.))
            .line_height(px(18.))
            .cursor_text()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx);
            })
            .child(Textarea::new(&self.state))
    }
}

/// A multi-line input state that shows `min` lines and grows with its text to `max`.
pub fn textarea(
    value: String,
    (min, max): (usize, usize),
    window: &mut Window,
    cx: &mut App,
) -> Entity<TextareaState> {
    use gpui_kit::AppContext as _;
    cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(min, max)
            .default_value(value)
    })
}
