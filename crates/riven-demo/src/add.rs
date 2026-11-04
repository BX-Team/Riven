use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription,
    WeakEntity, Window, div, img, px,
};
use moonglass::theme::ActiveTheme as _;
use moonglass::toast::ToastKind;
use moonglass::{Button, IconName, TextField, W_SEMIBOLD, h_flex, v_flex};

use crate::app::{Demo, Kind};
use crate::data::INSTANCES;

/// The Add dialog over a made-up "Modrinth": search, install, and the file shows up in the list.
pub struct AddDialog {
    demo: WeakEntity<Demo>,
    instance: usize,
    kind: Kind,
    query: Entity<InputState>,
    filter: String,
    _query: Subscription,
}

impl AddDialog {
    pub fn new(
        demo: WeakEntity<Demo>,
        instance: usize,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder(format!("Search {} on Modrinth", kind.noun()))
        });
        query.update(cx, |s, cx| s.focus(window, cx));
        let _query = cx.subscribe(&query, |this, input, event, cx| {
            if let InputEvent::Change = event {
                this.filter = input.read(cx).value().to_lowercase();
                cx.notify();
            }
        });
        Self {
            demo,
            instance,
            kind,
            query,
            filter: String::new(),
            _query,
        }
    }

    fn render_hit(&self, ix: usize, installed: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let hit = &self.kind.hits()[ix];
        let action = if installed {
            Button::new(("installed", ix))
                .icon(IconName::Check)
                .label("Installed")
                .disabled(true)
        } else {
            let (demo, instance, kind) = (self.demo.clone(), self.instance, self.kind);
            Button::new(("install", ix))
                .primary()
                .icon(IconName::Plus)
                .label("Install")
                .on_click(cx.listener(move |_, _, _, cx| {
                    let hit = &kind.hits()[ix];
                    let _ = demo.update(cx, |demo, cx| demo.add(instance, kind, hit, cx));
                    moonglass::toast::show(ToastKind::Success, format!("{} added", hit.name), cx);
                    cx.notify();
                }))
        };
        h_flex()
            .id(("hit", ix))
            .gap(px(12.))
            .px(px(10.))
            .py(px(10.))
            .rounded(px(8.))
            .hover(|row| row.bg(c.row))
            .child(
                div()
                    .size(px(40.))
                    .flex_none()
                    .rounded(px(8.))
                    .bg(c.sel)
                    .child(img(hit.icon).size(px(40.)).rounded(px(8.))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(div().font_weight(W_SEMIBOLD).truncate().child(hit.name))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.))
                                    .text_color(c.muted)
                                    .child(format!("↓ {}", hit.downloads)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(c.text2)
                            .truncate()
                            .child(hit.summary),
                    ),
            )
            .child(div().flex_none().child(action))
            .into_any_element()
    }
}

impl Render for AddDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let instance = &INSTANCES[self.instance];
        let target = match (self.kind, instance.loader) {
            (Kind::Mods, Some((loader, version))) => {
                format!("Minecraft {} · {loader} {version}", instance.minecraft)
            }
            _ => format!("Minecraft {}", instance.minecraft),
        };
        let installed: Vec<bool> = match self.demo.upgrade() {
            Some(demo) => {
                let demo = demo.read(cx);
                self.kind
                    .hits()
                    .iter()
                    .map(|h| demo.has(self.instance, self.kind, h.name))
                    .collect()
            }
            None => vec![false; self.kind.hits().len()],
        };
        let rows: Vec<AnyElement> = self
            .kind
            .hits()
            .iter()
            .enumerate()
            .filter(|(_, h)| self.filter.is_empty() || h.name.to_lowercase().contains(&self.filter))
            .map(|(ix, _)| self.render_hit(ix, installed[ix], cx))
            .collect();
        let empty = rows.is_empty();
        let demo = self.demo.clone();
        v_flex()
            .p(px(20.))
            .gap(px(14.))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::BOLD)
                            .child(format!("Add {}", self.kind.noun())),
                    )
                    .child(
                        div()
                            .text_color(c.muted)
                            .child(format!("{target} · only versions that fit are shown")),
                    ),
            )
            .child(
                TextField::new(&self.query)
                    .leading(IconName::Search)
                    .clearable(),
            )
            .child(
                div()
                    .id("hits")
                    .h(px(380.))
                    .overflow_y_scroll()
                    .children(rows)
                    .when(empty, |list| {
                        list.child(
                            div()
                                .pt(px(40.))
                                .text_center()
                                .text_color(c.muted)
                                .child("Nothing matches the search."),
                        )
                    }),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("add-done")
                        .outline()
                        .label("Done")
                        .on_click(move |_, _, cx| {
                            let _ = demo.update(cx, |demo, cx| demo.close_modal(cx));
                        }),
                ),
            )
    }
}
