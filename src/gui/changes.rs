use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px, relative,
};
use riven_format::Changes;
use rust_i18n::t;

use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{Button, ButtonSize, v_flex};

/// "What's new": the author's notes and the content an update to `version` changed.
pub fn open(version: String, changes: Changes, cx: &mut App) {
    let view = cx.new(|_| ChangesDialog { version, changes });
    super::dialogs::open(view.into(), 520., cx);
}

struct ChangesDialog {
    version: String,
    changes: Changes,
}

fn section(title: SharedString, names: &[String], cx: &App) -> Option<AnyElement> {
    if names.is_empty() {
        return None;
    }
    let c = cx.theme().colors;
    Some(
        v_flex()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.muted)
                    .child(format!("{title} · {}", names.len())),
            )
            .child(div().line_height(relative(1.5)).child(names.join(", ")))
            .into_any_element(),
    )
}

impl Render for ChangesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let ch = &self.changes;
        let notes = ch.notes.iter().map(|n| {
            v_flex()
                .gap(px(4.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(n.version.clone()),
                )
                .child(
                    div()
                        .line_height(relative(1.5))
                        .text_color(c.text2)
                        .child(n.text.clone()),
                )
        });
        let sections = [
            section(t!("changes.added").into(), &ch.added, cx),
            section(t!("changes.updated").into(), &ch.updated, cx),
            section(t!("changes.removed").into(), &ch.removed, cx),
        ];
        let empty = ch.notes.is_empty() && sections.iter().all(Option::is_none);
        let body = v_flex()
            .id("pack-changes")
            .max_h(px(380.))
            .overflow_y_scroll()
            .gap(px(14.))
            .children(notes)
            .children(sections.into_iter().flatten())
            .when(ch.other > 0, |b| {
                b.child(
                    div()
                        .text_size(px(12.))
                        .text_color(c.muted)
                        .child(t!("changes.other", n = ch.other).to_string()),
                )
            })
            .when(empty, |b| {
                b.child(
                    div()
                        .text_color(c.muted)
                        .child(t!("changes.nothing").to_string()),
                )
            });
        let close = Button::new("close")
            .outline()
            .size(ButtonSize::Md)
            .label(t!("common.close"))
            .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx)));
        super::dialogs::dialog_shell(
            t!("changes.title", version = self.version),
            t!("changes.from", version = ch.from),
            body,
            vec![close],
            cx,
        )
    }
}
