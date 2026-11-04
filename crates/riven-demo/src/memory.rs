use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::base::{Slider, SliderIndicator, SliderThumb, SliderTrack};
use gpui_kit::{
    AppContext as _, Context, DefiniteLength, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div, px, relative,
};
use moonglass::theme::ActiveTheme as _;
use moonglass::{W_SEMIBOLD, h_flex, v_flex};

const FLOOR: u32 = 512;
const STEP: u32 = 512;
const CAP: u32 = 16 * 1024;

/// "2 GB", "2.5 GB", "768 MB".
fn amount(mb: u32) -> String {
    if mb < 1024 {
        return format!("{mb} MB");
    }
    let gb = mb as f32 / 1024.;
    if (gb - gb.round()).abs() < 0.01 {
        format!("{} GB", gb.round() as u32)
    } else {
        format!("{gb:.1} GB")
    }
}

/// The JVM's least and most memory on one track with two handles, as in the app.
pub struct MemorySlider {
    state: Entity<SliderState>,
    min: u32,
    max: u32,
    _sub: Subscription,
}

impl MemorySlider {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (min, max) = (2048, 6144);
        let state = cx.new(|_| {
            SliderState::new()
                .max(CAP as f32)
                .min(FLOOR as f32)
                .step(STEP as f32)
                .default_value((min as f32, max as f32))
        });
        let _sub = cx.subscribe(&state, |this, _, event: &SliderEvent, cx| {
            let (SliderEvent::Change(v) | SliderEvent::Release(v)) = event;
            this.min = v.start().round() as u32;
            this.max = (v.end().round() as u32).max(this.min);
            cx.notify();
        });
        Self {
            state,
            min,
            max,
            _sub,
        }
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
        let label = |caption: &'static str, mb: u32| {
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
                    .child(label("Minimum", self.min))
                    .child(label("Maximum", self.max)),
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
                    .child(amount(CAP)),
            )
    }
}
