use std::time::{Duration, Instant};

use gpui_kit::base::{Interpolate, Presence, PresencePhase, Transition, transition};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, Div, ElementId, Entity, EntityId,
    IntoElement, ParentElement as _, Pixels, SharedString, StatefulInteractiveElement, Styled, Svg,
    Transformation, Window, div, ease_out_quint, percentage, px,
};

use crate::gui::theme;

/// How long hover and selection colors take to settle.
const FAST: Duration = Duration::from_millis(140);
/// How long a highlight takes to glide to another item.
const GLIDE: Duration = Duration::from_millis(240);
const ENTER: Duration = Duration::from_millis(240);
pub const EXIT: Duration = Duration::from_millis(140);
/// The gap between items of a list that appear one after another.
const STAGGER: Duration = Duration::from_millis(30);

fn settle<T: Interpolate + PartialEq + 'static>(
    key: ElementId,
    target: T,
    time: Duration,
    window: &mut Window,
    cx: &mut App,
) -> T {
    let time = if theme::fading(cx) {
        Duration::ZERO
    } else {
        time
    };
    transition(
        key,
        target,
        Transition::new(time).ease(ease_out_quint()),
        window,
        cx,
    )
}

/// Moves a color or value toward `target`; jumps there when motion is reduced or the theme fades.
pub fn animate<T: Interpolate + PartialEq + 'static>(
    key: impl Into<ElementId>,
    target: T,
    window: &mut Window,
    cx: &mut App,
) -> T {
    settle(key.into(), target, FAST, window, cx)
}

/// Like [`animate`], slower: for things that travel, such as a switch knob.
pub fn glide<T: Interpolate + PartialEq + 'static>(
    key: impl Into<ElementId>,
    target: T,
    window: &mut Window,
    cx: &mut App,
) -> T {
    settle(key.into(), target, GLIDE, window, cx)
}

/// Whether the pointer is over an element, remembered across frames of one view.
pub struct Hover {
    pub on: bool,
    state: Entity<bool>,
    view: EntityId,
}

pub fn hover(key: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> Hover {
    let state = window.use_keyed_state(key.into(), cx, |_, _| false);
    Hover {
        on: *state.read(cx),
        state,
        view: window.current_view(),
    }
}

impl Hover {
    pub fn track<E: StatefulInteractiveElement>(self, el: E) -> E {
        el.on_hover(move |on, _, cx| {
            self.state.update(cx, |s, _| *s = *on);
            cx.notify(self.view);
        })
    }
}

/// How far a surface has appeared (0–1); plays once per key, even across reduced motion.
fn presence(id: ElementId, delay: Duration, window: &mut Window, cx: &mut App) -> f32 {
    let sample = Presence::new(id, true)
        .transition(Transition::new(ENTER).delay(delay).ease(ease_out_quint()))
        .sample(window, cx);
    match sample.phase {
        PresencePhase::Present => 1.,
        _ => sample.progress,
    }
}

/// Fades a surface in while it rises `rise` pixels into place.
pub fn enter<E: IntoElement + Styled + 'static>(
    id: impl Into<ElementId>,
    el: E,
    rise: f32,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    enter_nth(id, el, rise, 0, window, cx)
}

/// Like [`enter`], `step` beats after the first item of a list.
pub fn enter_nth<E: IntoElement + Styled + 'static>(
    id: impl Into<ElementId>,
    el: E,
    rise: f32,
    step: usize,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = presence(id.into(), STAGGER * step as u32, window, cx);
    el.relative()
        .opacity(t)
        .top(px((1. - t) * rise))
        .into_any_element()
}

/// Progress of a surface that is leaving: 1 while it shows, falling to 0 once `closing` is set.
pub fn leave(id: impl Into<ElementId>, closing: bool, window: &mut Window, cx: &mut App) -> f32 {
    if !closing {
        return presence(id.into(), Duration::ZERO, window, cx);
    }
    let sample = Presence::new(id.into(), false)
        .transition(Transition::new(EXIT).ease(ease_out_quint()))
        .sample(window, cx);
    sample.progress
}

/// Entrance progress of row `ix` of a list shown at `since`, one row after another.
pub fn cascade(since: Option<Instant>, ix: usize, window: &mut Window, cx: &App) -> f32 {
    let Some(since) = since.filter(|_| !cx.reduce_motion()) else {
        return 1.;
    };
    let delay = Duration::from_millis(14) * ix.min(24) as u32;
    let elapsed = since.elapsed().saturating_sub(delay);
    let t = (elapsed.as_secs_f32() / ENTER.as_secs_f32()).min(1.);
    if t < 1. {
        window.request_animation_frame();
    }
    ease_out_quint()(t)
}

/// A loading indicator: an arc that turns while motion is allowed.
pub fn spinner(id: impl Into<ElementId>, icon: Svg, cx: &App) -> AnyElement {
    if cx.reduce_motion() {
        return icon.into_any_element();
    }
    icon.with_animation(
        id,
        Animation::new(Duration::from_millis(900)).repeat(),
        |svg, t| svg.with_transformation(Transformation::rotate(percentage(t))),
    )
    .into_any_element()
}

#[derive(Default)]
struct Slots {
    items: Vec<Bounds<Pixels>>,
    last: Option<Bounds<Pixels>>,
}

/// A list whose selected item sits on a gliding highlight, one frame behind layout.
pub fn highlighted(
    key: &str,
    container: Div,
    items: Vec<AnyElement>,
    selected: Option<usize>,
    paint: impl FnOnce(Div) -> Div,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let state = window.use_keyed_state(SharedString::from(format!("{key}:slots")), cx, |_, _| {
        Slots::default()
    });
    let view = window.current_view();
    let target = selected.and_then(|ix| state.read(cx).items.get(ix).copied());
    if let Some(b) = target {
        state.update(cx, |s, _| s.last = Some(b));
    }
    let place = state
        .read(cx)
        .last
        .map(|b| glide(SharedString::from(format!("{key}:place")), b, window, cx));
    let shown = animate(
        SharedString::from(format!("{key}:shown")),
        if target.is_some() { 1f32 } else { 0. },
        window,
        cx,
    );
    let indicator = place.map(|b| {
        paint(
            div()
                .absolute()
                .left(b.origin.x)
                .top(b.origin.y)
                .w(b.size.width)
                .h(b.size.height)
                .opacity(shown),
        )
    });
    container
        .relative()
        .when_none(&indicator, |c| c.child(div().absolute()))
        .children(indicator)
        .children(items)
        .on_children_prepainted(move |bounds, _, cx| {
            let Some(first) = bounds.get(1).copied() else {
                return;
            };
            let items: Vec<_> = bounds[1..]
                .iter()
                .map(|b| Bounds::new(b.origin - first.origin, b.size))
                .collect();
            if state.read(cx).items != items {
                state.update(cx, |s, _| s.items = items);
                cx.notify(view);
            }
        })
}
