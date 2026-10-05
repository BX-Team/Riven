use gpui_kit::base::{Selectable, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement as _, StyleRefinement,
    Styled, Window, div, px,
};

use super::{IconName, W_HEAVY, icon, motion, tooltip};
use crate::gui::theme::ActiveTheme as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// 28 px: close and remove buttons inside panels.
    Xs,
    /// 30 px: toolbars, settings controls, sidebar icon buttons.
    #[default]
    Sm,
    /// 38 px: buttons in popovers and dialogs.
    Md,
    /// 44 px: Play.
    Lg,
}

impl ButtonSize {
    fn height(self) -> f32 {
        match self {
            Self::Xs => 28.,
            Self::Sm => 30.,
            Self::Md => 38.,
            Self::Lg => 44.,
        }
    }

    fn padding(self) -> f32 {
        match self {
            Self::Xs => 8.,
            Self::Sm => 12.,
            Self::Md => 16.,
            Self::Lg => 30.,
        }
    }

    fn radius(self) -> f32 {
        match self {
            Self::Xs | Self::Sm => 6.,
            Self::Md | Self::Lg => 8.,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Primary,
    Danger,
    Outline,
    Ghost,
}

/// A button drawn the way the design draws them, on top of gpui-base's behavior.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    base: gpui_kit::base::Button,
    style: StyleRefinement,
    variant: Variant,
    size: ButtonSize,
    icon: Option<IconName>,
    label: Option<SharedString>,
    children: Vec<AnyElement>,
    tooltip: Option<SharedString>,
    selected: bool,
    disabled: bool,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            base: gpui_kit::base::Button::new(id.clone()),
            id,
            style: StyleRefinement::default(),
            variant: Variant::Outline,
            size: ButtonSize::Sm,
            icon: None,
            label: None,
            children: Vec::new(),
            tooltip: None,
            selected: false,
            disabled: false,
        }
    }

    pub fn primary(mut self) -> Self {
        self.variant = Variant::Primary;
        self
    }

    /// A filled red button for actions that destroy something.
    pub fn danger(mut self) -> Self {
        self.variant = Variant::Danger;
        self
    }

    pub fn outline(mut self) -> Self {
        self.variant = Variant::Outline;
        self
    }

    pub fn ghost(mut self) -> Self {
        self.variant = Variant::Ghost;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Hover text; icon-only buttons also get it as their accessible name.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.base = self.base.on_click(f);
        self
    }
}

impl Selectable for Button {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.theme().colors;
        let size = self.size;
        let square = self.label.is_none() && self.children.is_empty();
        let primary = matches!(self.variant, Variant::Primary | Variant::Danger);
        let enabled = !self.disabled;
        let key = format!("button:{}", self.id);
        let hover = motion::hover(SharedString::from(format!("{key}:hover")), window, cx);
        let hovered = hover.on && enabled;
        let fg = match self.variant {
            Variant::Primary => c.on_accent,
            Variant::Danger => gpui_kit::white(),
            _ if self.selected || hovered => c.text,
            Variant::Outline if !square => c.text,
            _ => c.muted,
        };
        let bg = match self.variant {
            Variant::Primary => c.accent,
            Variant::Danger => crate::gui::theme::danger(),
            _ if self.selected => c.sel,
            _ if hovered => c.row,
            _ => c.row.opacity(0.),
        };
        let fg = motion::animate(SharedString::from(format!("{key}:fg")), fg, window, cx);
        let bg = motion::animate(SharedString::from(format!("{key}:bg")), bg, window, cx);
        let lift = motion::animate(
            SharedString::from(format!("{key}:lift")),
            if primary && hovered { 0.88 } else { 1. },
            window,
            cx,
        );

        hover
            .track(self.base)
            .disabled(self.disabled)
            .selected(self.selected)
            .when_some(self.tooltip.clone().filter(|_| square), |b, t| {
                b.accessibility_label(t)
            })
            .h(px(size.height()))
            .flex_none()
            .gap(px(8.))
            .rounded(px(size.radius()))
            .text_color(fg)
            .bg(bg)
            .map(|b| {
                if square {
                    b.w(px(size.height()))
                } else {
                    b.px(px(size.padding()))
                }
            })
            .map(|b| match self.variant {
                Variant::Primary | Variant::Danger => b
                    .opacity(lift)
                    .font_weight(if size == ButtonSize::Lg {
                        W_HEAVY
                    } else {
                        FontWeight::BOLD
                    })
                    .when(size == ButtonSize::Lg, |b| b.text_size(px(15.))),
                Variant::Outline => b.border_1().border_color(c.border),
                Variant::Ghost => b,
            })
            .when(enabled, |b| {
                b.cursor_pointer()
                    .when(primary, |b| b.active(|s| s.opacity(0.8)))
            })
            .when(self.disabled, |b| b.opacity(0.5))
            .focus_visible(|s| s.border_1().border_color(c.accent))
            .when_some(self.tooltip, |b, t| b.tooltip(tooltip(t)))
            .when_some(self.icon, |b, name| b.child(icon(name, fg)))
            .when_some(self.label, |b, l| b.child(div().child(l)))
            .children(self.children)
            .refine_style(&self.style)
    }
}
