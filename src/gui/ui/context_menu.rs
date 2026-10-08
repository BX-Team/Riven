use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point,
    Styled as _, Window, anchored, deferred, div, point, px,
};

use super::menu::{MenuEntry, OnAction, menu_panel};
use super::motion;

const WIDTH: f32 = 220.;
const MARGIN: f32 = 8.;

/// A menu a right click opened at the pointer.
#[derive(Clone)]
pub struct ContextMenu {
    pub position: Point<Pixels>,
    pub entries: Vec<MenuEntry>,
    /// Tells one opening from the next, so the entrance plays for each.
    pub serial: u64,
}

fn height(entries: &[MenuEntry]) -> f32 {
    let rows: f32 = entries
        .iter()
        .map(|e| match e {
            MenuEntry::Separator => 9.,
            MenuEntry::Caption(_) => 23.,
            MenuEntry::Action { .. } => 29.,
        })
        .sum();
    rows + 10.
}

/// The open context menu over everything, closed by any press outside it.
pub fn context_menu(
    menu: &ContextMenu,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let viewport = window.viewport_size();
    let close: OnAction = Rc::new(on_close);
    let x = menu
        .position
        .x
        .min(viewport.width - px(WIDTH + MARGIN))
        .max(px(MARGIN));
    let fits_below = menu.position.y + px(height(&menu.entries) + MARGIN) <= viewport.height;
    let y = if fits_below {
        menu.position.y
    } else {
        (menu.position.y - px(height(&menu.entries))).max(px(MARGIN))
    };
    let list = menu_panel(&menu.entries, px(WIDTH), close.clone(), window, cx);
    let panel = div()
        .absolute()
        .left(x)
        .top(y)
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
        .child(motion::enter(
            ("context-menu", menu.serial as usize),
            list,
            -4.,
            window,
            cx,
        ));
    let dismiss = close.clone();
    deferred(
        anchored().position(point(px(0.), px(0.))).child(
            div()
                .id("context-menu-backdrop")
                .w(viewport.width)
                .h(viewport.height)
                .occlude()
                .on_any_mouse_down(move |_, window, cx| {
                    cx.stop_propagation();
                    dismiss(window, cx);
                })
                .on_scroll_wheel(move |_, window, cx| close(window, cx))
                .child(panel),
        ),
    )
    .with_priority(30)
    .into_any_element()
}
