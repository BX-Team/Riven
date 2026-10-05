use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, px,
};

use super::{h_flex, motion};
use crate::gui::theme::ActiveTheme as _;

type Handler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// An on/off switch, 40×22 (44×24 when large), optionally led by a clickable label.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    checked: bool,
    large: bool,
    label: Option<SharedString>,
    accessible: Option<SharedString>,
    on_change: Option<Handler>,
}

impl Switch {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self {
            id: id.into(),
            checked,
            large: false,
            label: None,
            accessible: None,
            on_change: None,
        }
    }

    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn accessible(mut self, name: impl Into<SharedString>) -> Self {
        self.accessible = Some(name.into());
        self
    }

    pub fn on_change(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.theme().colors;
        let on = self.checked;
        let (w, h) = if self.large { (44., 24.) } else { (40., 22.) };
        let knob = h - 4.;
        let key = format!("switch:{}", self.id);
        let mut anim = |part: &str, on_value: gpui_kit::Hsla, off_value: gpui_kit::Hsla| {
            motion::animate(
                SharedString::from(format!("{key}:{part}")),
                if on { on_value } else { off_value },
                window,
                cx,
            )
        };
        let track_bg = anim("track", c.accent, c.sel);
        let knob_bg = anim("knob", c.on_accent, c.muted);
        let pos = motion::glide(
            SharedString::from(format!("{key}:pos")),
            if on { 1f32 } else { 0. },
            window,
            cx,
        );
        let track = div()
            .w(px(w))
            .h(px(h))
            .flex_none()
            .rounded(px(h / 2.))
            .p(px(2.))
            .flex()
            .bg(track_bg)
            .child(
                div()
                    .ml(px(pos * (w - h)))
                    .size(px(knob))
                    .rounded(px(knob / 2.))
                    .bg(knob_bg),
            );
        let on_change = self.on_change;
        gpui_kit::base::Switch::new(self.id)
            .checked(on)
            .when_some(self.accessible.or(self.label.clone()), |s, name| {
                s.accessibility_label(name)
            })
            .when_some(on_change, |s, f| {
                s.on_change(move |v, _, w, cx| f(v, w, cx))
            })
            .cursor_pointer()
            .rounded(px(h / 2.))
            .focus_visible(|s| s.border_1().border_color(c.accent))
            .child(
                h_flex()
                    .gap(px(8.))
                    .when_some(self.label, |row, l| {
                        row.child(div().text_color(c.text2).child(l))
                    })
                    .child(track),
            )
    }
}
