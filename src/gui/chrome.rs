use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, CursorStyle, Decorations, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Render, ResizeEdge, SharedString,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window, WindowControlArea,
    WindowDecorations, WindowOptions, div, hsla, point, px,
};

use super::theme::ActiveTheme as _;
use super::ui::{IconName, h_flex, icon, motion};

const HEIGHT: f32 = 44.;
const EDGE: f32 = 5.;

/// Window options for a window that draws its own title bar and window buttons.
pub fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.), px(15.))),
        }),
        app_owns_titlebar_drag: true,
        window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
        ..Default::default()
    }
}

struct Drag {
    pressed: bool,
}

impl Render for Drag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The window's top row: the name, `actions`, then the window buttons outside macOS.
pub fn title_bar(actions: AnyElement, window: &mut Window, cx: &mut App) -> AnyElement {
    let c = cx.theme().colors;
    let drag = window.use_state(cx, |_, _| Drag { pressed: false });
    let mac = cfg!(target_os = "macos");
    h_flex()
        .id("title-bar")
        .flex_none()
        .h(px(HEIGHT))
        .bg(c.panel)
        .border_b_1()
        .border_color(c.border)
        .child(
            h_flex()
                .id("drag")
                .flex_1()
                .h_full()
                .pl(px(if mac { 84. } else { 16. }))
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down(
                    MouseButton::Left,
                    window.listener_for(&drag, |d, _, _, _| d.pressed = true),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    window.listener_for(&drag, |d, _, _, _| d.pressed = false),
                )
                .on_mouse_move(window.listener_for(&drag, |d, _, window, _| {
                    if d.pressed {
                        d.pressed = false;
                        window.start_window_move();
                    }
                }))
                .on_click(|event, window, _| {
                    if event.click_count() == 2 {
                        if cfg!(target_os = "macos") {
                            window.titlebar_double_click();
                        } else {
                            window.zoom_window();
                        }
                    }
                })
                .when(cfg!(target_os = "linux"), |d| {
                    d.on_mouse_down(MouseButton::Right, |e, window, _| {
                        window.show_window_menu(e.position)
                    })
                })
                .gap(px(8.))
                .child(gpui_kit::img(super::assets::LOGO).size(px(20.)).flex_none())
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(14.))
                        .child("Riven Launcher"),
                ),
        )
        .child(div().pr(px(if mac { 12. } else { 8. })).child(actions))
        .when(!mac, |bar| {
            let controls = window.window_controls();
            let maximized = window.is_maximized();
            let linux = cfg!(target_os = "linux");
            bar.child(div().w(px(1.)).h(px(20.)).mr(px(4.)).bg(c.border))
                .when(!linux || controls.minimize, |b| {
                    b.child(control(Control::Minimize, c.text2, c.row, window, cx))
                })
                .when(!linux || controls.maximize, |b| {
                    let kind = if maximized {
                        Control::Restore
                    } else {
                        Control::Maximize
                    };
                    b.child(control(kind, c.text2, c.row, window, cx))
                })
                .child(control(
                    Control::Close,
                    c.text2,
                    super::theme::danger(),
                    window,
                    cx,
                ))
        })
        .into_any_element()
}

#[derive(Clone, Copy)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

fn control(
    kind: Control,
    fg: Hsla,
    hover_bg: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let (id, name, area) = match kind {
        Control::Minimize => ("minimize", IconName::WindowMinimize, WindowControlArea::Min),
        Control::Maximize => ("maximize", IconName::WindowMaximize, WindowControlArea::Max),
        Control::Restore => ("restore", IconName::WindowRestore, WindowControlArea::Max),
        Control::Close => ("close", IconName::WindowClose, WindowControlArea::Close),
    };
    let close = matches!(kind, Control::Close);
    let hover = motion::hover(SharedString::from(format!("control:{id}")), window, cx);
    let bg = motion::animate(
        SharedString::from(format!("control:{id}:bg")),
        if hover.on {
            hover_bg
        } else {
            hover_bg.opacity(0.)
        },
        window,
        cx,
    );
    let ink = if hover.on && close {
        hsla(0., 0., 1., 1.)
    } else {
        fg
    };
    hover
        .track(div().id(id))
        .w(px(46.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(bg)
        .when(cfg!(target_os = "windows"), |d| d.window_control_area(area))
        .when(cfg!(target_os = "linux"), |d| {
            d.on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                match kind {
                    Control::Minimize => window.minimize_window(),
                    Control::Maximize | Control::Restore => window.zoom_window(),
                    Control::Close => window.remove_window(),
                }
            })
        })
        .child(icon(name, ink).size(px(16.)))
}

/// Invisible strips along the window's free edges that resize it when it draws its own frame.
pub fn resize_edges(window: &Window) -> Option<AnyElement> {
    let Decorations::Client { tiling } = window.window_decorations() else {
        return None;
    };
    if window.is_maximized() || window.is_fullscreen() {
        return None;
    }
    let edge = |id: &'static str, edge: ResizeEdge, cursor: CursorStyle| {
        div().id(id).absolute().cursor(cursor).on_mouse_down(
            MouseButton::Left,
            move |_, window, cx| {
                cx.stop_propagation();
                window.start_window_resize(edge);
            },
        )
    };
    let e = px(EDGE);
    let corner = px(EDGE * 2.);
    Some(
        div()
            .absolute()
            .inset_0()
            .when(!tiling.top, |d| {
                d.child(
                    edge("resize-top", ResizeEdge::Top, CursorStyle::ResizeUpDown)
                        .top_0()
                        .left(corner)
                        .right(corner)
                        .h(e),
                )
            })
            .when(!tiling.bottom, |d| {
                d.child(
                    edge(
                        "resize-bottom",
                        ResizeEdge::Bottom,
                        CursorStyle::ResizeUpDown,
                    )
                    .bottom_0()
                    .left(corner)
                    .right(corner)
                    .h(e),
                )
            })
            .when(!tiling.left, |d| {
                d.child(
                    edge(
                        "resize-left",
                        ResizeEdge::Left,
                        CursorStyle::ResizeLeftRight,
                    )
                    .left_0()
                    .top(corner)
                    .bottom(corner)
                    .w(e),
                )
            })
            .when(!tiling.right, |d| {
                d.child(
                    edge(
                        "resize-right",
                        ResizeEdge::Right,
                        CursorStyle::ResizeLeftRight,
                    )
                    .right_0()
                    .top(corner)
                    .bottom(corner)
                    .w(e),
                )
            })
            .when(!tiling.top && !tiling.left, |d| {
                d.child(
                    edge(
                        "resize-tl",
                        ResizeEdge::TopLeft,
                        CursorStyle::ResizeUpLeftDownRight,
                    )
                    .top_0()
                    .left_0()
                    .size(corner),
                )
            })
            .when(!tiling.top && !tiling.right, |d| {
                d.child(
                    edge(
                        "resize-tr",
                        ResizeEdge::TopRight,
                        CursorStyle::ResizeUpRightDownLeft,
                    )
                    .top_0()
                    .right_0()
                    .size(corner),
                )
            })
            .when(!tiling.bottom && !tiling.left, |d| {
                d.child(
                    edge(
                        "resize-bl",
                        ResizeEdge::BottomLeft,
                        CursorStyle::ResizeUpRightDownLeft,
                    )
                    .bottom_0()
                    .left_0()
                    .size(corner),
                )
            })
            .when(!tiling.bottom && !tiling.right, |d| {
                d.child(
                    edge(
                        "resize-br",
                        ResizeEdge::BottomRight,
                        CursorStyle::ResizeUpLeftDownRight,
                    )
                    .bottom_0()
                    .right_0()
                    .size(corner),
                )
            })
            .into_any_element(),
    )
}
