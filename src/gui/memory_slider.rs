use std::rc::Rc;

use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::base::{Slider, SliderIndicator, SliderThumb, SliderTrack};
use gpui_kit::{
    App, AppContext as _, Context, DefiniteLength, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div, px, relative,
};
use riven_format::MemoryMb;
use rust_i18n::t;

use super::theme::ActiveTheme as _;
use super::ui::{W_SEMIBOLD, h_flex, v_flex};

const FLOOR: u32 = 512;
const STEP: u32 = 512;
/// The top of the scale when the installed memory is unknown.
const FALLBACK_CAP: u32 = 32 * 1024;

type OnChange = Rc<dyn Fn(MemoryMb, &mut App)>;

/// "2 GB", "2.5 GB", "768 MB".
pub fn amount(mb: u32) -> String {
    if mb < 1024 {
        return t!("memory.mb", n = mb).to_string();
    }
    let gb = mb as f32 / 1024.;
    let text = if (gb - gb.round()).abs() < 0.01 {
        format!("{}", gb.round() as u32)
    } else {
        format!("{gb:.1}")
    };
    t!("memory.gb", n = text).to_string()
}

/// The JVM's least and most memory on one track with two handles.
pub struct MemorySlider {
    state: Entity<SliderState>,
    value: MemoryMb,
    cap: u32,
    on_change: OnChange,
    _sub: Subscription,
}

fn cap_for(value: MemoryMb) -> u32 {
    let installed = riven_launch::java::total_memory_mb()
        .map(|t| t / 1024 * 1024)
        .unwrap_or(FALLBACK_CAP);
    installed.max(value.max).max(FLOOR * 2)
}

impl MemorySlider {
    pub fn new(
        value: MemoryMb,
        on_change: impl Fn(MemoryMb, &mut App) + 'static,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let cap = cap_for(value);
            let state = cx.new(|_| {
                SliderState::new()
                    .max(cap as f32)
                    .min(FLOOR as f32)
                    .step(STEP as f32)
                    .default_value((value.min as f32, value.max as f32))
            });
            let sub = cx.subscribe(&state, |this: &mut Self, _, event: &SliderEvent, cx| {
                let (SliderEvent::Change(v) | SliderEvent::Release(v)) = event;
                let picked = MemoryMb {
                    min: v.start().round() as u32,
                    max: (v.end().round() as u32).max(v.start().round() as u32),
                };
                this.value = picked;
                if let SliderEvent::Release(_) = event {
                    (this.on_change)(picked, cx);
                }
                cx.notify();
            });
            Self {
                state,
                value,
                cap,
                on_change: Rc::new(on_change),
                _sub: sub,
            }
        })
    }

    /// Shows another value without reporting it, as when an override starts from the defaults.
    pub fn set(&mut self, value: MemoryMb, window: &mut Window, cx: &mut Context<Self>) {
        self.value = value;
        self.state.update(cx, |s, cx| {
            s.set_value(
                SliderValue::Range(value.min as f32, value.max as f32),
                window,
                cx,
            )
        });
        cx.notify();
    }
}

impl Render for MemorySlider {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let percentage = self.state.read(cx).percentage();
        let thumb = |position: DefiniteLength, start: bool| {
            SliderThumb::new(&self.state)
                .start(start)
                .absolute()
                .top(px(-5.))
                .left(position)
                .ml(px(-8.))
                .size(px(16.))
                .rounded(px(8.))
                .bg(c.panel)
                .border_2()
                .border_color(c.accent)
                .cursor_pointer()
        };
        let label = |caption: String, mb: u32| {
            h_flex()
                .gap(px(6.))
                .child(div().text_color(c.muted).child(caption))
                .child(div().font_weight(W_SEMIBOLD).child(amount(mb)))
        };
        v_flex()
            .w_full()
            .gap(px(8.))
            .child(
                h_flex()
                    .justify_between()
                    .child(label(t!("settings.memory_min").to_string(), self.value.min))
                    .child(label(t!("settings.memory_max").to_string(), self.value.max)),
            )
            .child(
                Slider::new(&self.state).w_full().child(
                    SliderTrack::new(&self.state)
                        .flex()
                        .items_center()
                        .h(px(24.))
                        .w_full()
                        .cursor_pointer()
                        .child(
                            SliderIndicator::new(&self.state)
                                .relative()
                                .w_full()
                                .h(px(6.))
                                .rounded(px(3.))
                                .bg(c.row)
                                .child(
                                    div()
                                        .absolute()
                                        .h_full()
                                        .left(relative(percentage.start))
                                        .right(relative(1. - percentage.end))
                                        .rounded(px(3.))
                                        .bg(c.accent),
                                )
                                .child(thumb(relative(percentage.start), true))
                                .child(thumb(relative(percentage.end), false)),
                        ),
                ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .child(amount(FLOOR))
                    .child(amount(self.cap)),
            )
    }
}
