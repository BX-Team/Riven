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

type OnAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// One line of an [`ActionMenu`].
#[derive(Clone)]
pub enum MenuEntry {
    Caption(SharedString),
    Separator,
    Action {
        icon: Option<IconName>,
        label: SharedString,
        checked: bool,
        danger: bool,
        disabled: bool,
        run: OnAction,
    },
}

impl MenuEntry {
    pub fn action(
        label: impl Into<SharedString>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self::Action {
            icon: None,
            label: label.into(),
            checked: false,
            danger: false,
            disabled: false,
            run: Rc::new(run),
        }
    }

    pub fn icon(mut self, name: IconName) -> Self {
        if let Self::Action { icon, .. } = &mut self {
            *icon = Some(name);
        }
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        if let Self::Action { checked, .. } = &mut self {
            *checked = on;
        }
        self
    }

    pub fn danger(mut self) -> Self {
        if let Self::Action { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }

    pub fn disabled(mut self, off: bool) -> Self {
        if let Self::Action { disabled, .. } = &mut self {
            *disabled = off;
        }
        self
    }
}

/// A trigger button opening a list of actions, with optional captions between groups.
#[derive(IntoElement)]
pub struct ActionMenu {
    id: ElementId,
    trigger: Button,
    entries: Vec<MenuEntry>,
    width: Pixels,
    anchor: Anchor,
}

impl ActionMenu {
    pub fn new(id: impl Into<ElementId>, trigger: Button, entries: Vec<MenuEntry>) -> Self {
        Self {
            id: id.into(),
            trigger,
            entries,
            width: px(220.),
            anchor: Anchor::TopRight,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// Opens under the trigger's left edge instead of its right one.
    pub fn left(mut self) -> Self {
        self.anchor = Anchor::TopLeft;
        self
    }
}

impl RenderOnce for ActionMenu {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let entries = self.entries;
        let width = self.width;
        Popover::new(self.id)
            .anchor(self.anchor)
            .offset(px(4.))
            .trigger(self.trigger)
            .content(move |_, window, cx| {
                let c = cx.theme().colors;
                let popover = cx.entity();
                let rows: Vec<_> = entries
                    .iter()
                    .enumerate()
                    .map(|(n, entry)| match entry {
                        MenuEntry::Separator => div()
                            .my(px(4.))
                            .mx(px(4.))
                            .h(px(1.))
                            .bg(c.border)
                            .into_any_element(),
                        MenuEntry::Caption(text) => div()
                            .px(px(8.))
                            .pt(px(6.))
                            .pb(px(2.))
                            .text_size(px(11.))
                            .text_color(c.muted)
                            .child(text.clone())
                            .into_any_element(),
                        MenuEntry::Action {
                            icon: glyph,
                            label,
                            checked,
                            danger,
                            disabled,
                            run,
                        } => {
                            let hover = motion::hover(("action-menu-hover", n), window, cx);
                            let bg = motion::animate(
                                ("action-menu-bg", n),
                                if hover.on && !disabled {
                                    c.row
                                } else {
                                    c.row.opacity(0.)
                                },
                                window,
                                cx,
                            );
                            let ink = if *danger {
                                crate::gui::theme::danger()
                            } else if *checked {
                                c.text
                            } else {
                                c.text2
                            };
                            let (run, popover, disabled) =
                                (run.clone(), popover.clone(), *disabled);
                            hover
                                .track(h_flex().id(n))
                                .flex_none()
                                .h(px(28.))
                                .px(px(8.))
                                .gap(px(10.))
                                .rounded(px(6.))
                                .bg(bg)
                                .text_color(ink)
                                .when(disabled, |r| r.opacity(0.5))
                                .when(!disabled, |r| {
                                    r.cursor_pointer().on_click(move |_, window, cx| {
                                        popover.update(cx, |p, cx| p.dismiss(window, cx));
                                        run(window, cx);
                                    })
                                })
                                .when_some(*glyph, |r, g| r.child(icon(g, ink)))
                                .child(div().flex_1().truncate().child(label.clone()))
                                .when(*checked, |r| {
                                    r.child(icon(IconName::Check, c.ok).size(px(13.)))
                                })
                                .into_any_element()
                        }
                    })
                    .collect();
                let list = v_flex()
                    .ui_text(cx)
                    .w(width)
                    .p(px(4.))
                    .gap(px(1.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.panel)
                    .shadow(vec![box_shadow(0., 12., 32., 0., hsla(0., 0., 0., 0.35))])
                    .children(rows);
                motion::enter("action-menu-in", list, -6., window, cx)
            })
    }
}
