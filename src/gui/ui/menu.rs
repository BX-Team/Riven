use std::rc::Rc;

use gpui_kit::base::input::InputState;
use gpui_kit::base::{Popover, box_shadow};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, App, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    hsla, px,
};

use super::{Button, IconName, TextField, UiText as _, h_flex, icon, motion, v_flex};
use crate::gui::theme::ActiveTheme as _;

#[derive(Clone)]
pub struct MenuItem {
    pub value: SharedString,
    pub label: SharedString,
}

impl MenuItem {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

type OnSelect = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// A button showing the chosen item that opens the list of the others.
#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    items: Vec<MenuItem>,
    selected: Option<SharedString>,
    placeholder: SharedString,
    width: Pixels,
    search: Option<Entity<InputState>>,
    on_select: OnSelect,
}

impl Dropdown {
    pub fn new(
        id: impl Into<ElementId>,
        items: Vec<MenuItem>,
        selected: Option<SharedString>,
        on_select: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            items,
            selected,
            placeholder: SharedString::default(),
            width: px(200.),
            search: None,
            on_select: Rc::new(on_select),
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Filters a long list by what is typed into `state`.
    pub fn searchable(mut self, state: &Entity<InputState>) -> Self {
        self.search = Some(state.clone());
        self
    }
}

impl RenderOnce for Dropdown {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.theme().colors;
        let current = self
            .selected
            .as_ref()
            .and_then(|v| self.items.iter().find(|i| &i.value == v))
            .map(|i| i.label.clone());
        let has_value = current.is_some();
        let width = self.width;
        let trigger = Button::new((self.id.clone(), "trigger"))
            .outline()
            .w(width)
            .justify_between()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(!has_value, |d| d.text_color(c.muted))
                    .child(current.unwrap_or(self.placeholder)),
            )
            .child(icon(IconName::ChevronDown, c.muted).size(px(12.)));
        let items = self.items;
        let selected = self.selected;
        let search = self.search;
        let on_select = self.on_select;
        let list_id = self.id.clone();
        Popover::new(self.id)
            .anchor(Anchor::TopLeft)
            .offset(px(4.))
            .trigger(trigger)
            .content(move |_, window, cx| {
                let popover = cx.entity();
                let needle = search
                    .as_ref()
                    .map(|s| s.read(cx).value().to_lowercase())
                    .unwrap_or_default();
                let rows = items
                    .iter()
                    .filter(|i| needle.is_empty() || i.label.to_lowercase().contains(&needle))
                    .enumerate()
                    .map(|(n, item)| {
                        let on = selected.as_ref() == Some(&item.value);
                        let value = item.value.clone();
                        let on_select = on_select.clone();
                        let popover = popover.clone();
                        let hover = motion::hover(("menu-hover", n), window, cx);
                        let lit = on || hover.on;
                        let bg = motion::animate(
                            ("menu-bg", n),
                            match (on, hover.on) {
                                (true, _) => c.sel,
                                (false, true) => c.row,
                                _ => c.row.opacity(0.),
                            },
                            window,
                            cx,
                        );
                        hover
                            .track(h_flex().id(n))
                            .flex_none()
                            .h(px(28.))
                            .px(px(8.))
                            .gap(px(8.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_color(if lit { c.text } else { c.text2 })
                            .bg(bg)
                            .on_click(move |_, window, cx| {
                                on_select(value.clone(), window, cx);
                                popover.update(cx, |p, cx| p.dismiss(window, cx));
                            })
                            .child(div().flex_1().truncate().child(item.label.clone()))
                            .when(on, |r| r.child(icon(IconName::Check, c.ok).size(px(13.))))
                    })
                    .collect::<Vec<_>>();
                let list = v_flex()
                    .ui_text(cx)
                    .w(width)
                    .p(px(4.))
                    .gap(px(2.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.panel)
                    .text_color(c.text)
                    .shadow(vec![box_shadow(0., 12., 32., 0., hsla(0., 0., 0., 0.35))])
                    .when_some(search.clone(), |col, s| {
                        col.child(TextField::new(&s).leading(IconName::Search).mb(px(2.)))
                    })
                    .child(
                        v_flex()
                            .id((list_id.clone(), "list"))
                            .max_h(px(300.))
                            .overflow_y_scroll()
                            .gap(px(2.))
                            .children(rows),
                    );
                motion::enter("menu-in", list, -6., window, cx)
            })
    }
}
