use gpui_kit::{
    AnyElement, App, AppContext as _, Context, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, px, relative,
};
use riven_launch::crash::{Cause, Culprit, Dependency, Diagnosis};
use rust_i18n::t;

use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{Button, ButtonSize, h_flex, v_flex};

/// One line for the launch bar: what most likely went wrong.
pub fn summary(cause: &Cause) -> SharedString {
    match cause {
        Cause::Dependencies(_) => t!("crash.dependencies"),
        Cause::Mismatch { culprit, .. } => t!("crash.mismatch", name = culprit.label()),
        Cause::Mixin(culprit) => t!("crash.mixin", name = culprit.label()),
        Cause::Java { needed } => t!("crash.java", version = needed),
        Cause::Memory => t!("crash.memory"),
        Cause::Mod { culprit, .. } => t!("crash.mod", name = culprit.label()),
    }
    .into()
}

fn explanation(cause: &Cause) -> SharedString {
    match cause {
        Cause::Dependencies(_) => t!("crash.dependencies_hint"),
        Cause::Mismatch {
            culprit,
            other: Some(other),
        } => t!(
            "crash.mismatch_hint",
            name = culprit.label(),
            other = other.label()
        ),
        Cause::Mismatch { culprit, .. } => t!("crash.mismatch_game_hint", name = culprit.label()),
        Cause::Mixin(culprit) => t!("crash.mixin_hint", name = culprit.label()),
        Cause::Java { needed } => t!("crash.java_hint", version = needed),
        Cause::Memory => t!("crash.memory_hint"),
        Cause::Mod { culprit, .. } => t!("crash.mod_hint", name = culprit.label()),
    }
    .into()
}

/// The mod whose file is worth showing: the one to update or remove.
fn culprit(cause: &Cause) -> Option<&Culprit> {
    match cause {
        Cause::Mismatch { culprit, .. } | Cause::Mixin(culprit) | Cause::Mod { culprit, .. } => {
            Some(culprit)
        }
        Cause::Dependencies(_) | Cause::Java { .. } | Cause::Memory => None,
    }
}

pub fn open(diagnosis: Diagnosis, cx: &mut App) {
    let view = cx.new(|_| CrashDialog { diagnosis });
    super::dialogs::open(view.into(), 520., cx);
}

struct CrashDialog {
    diagnosis: Diagnosis,
}

impl CrashDialog {
    fn dependency(&self, dep: &Dependency, cx: &App) -> AnyElement {
        let c = cx.theme().colors;
        let found = match &dep.found {
            Some(v) => t!("crash.found", version = v),
            None => t!("crash.missing"),
        };
        let need = if dep.incompatible {
            t!(
                "crash.breaks",
                name = dep.by.label(),
                other = dep.needs.label()
            )
        } else {
            t!(
                "crash.needs",
                name = dep.by.label(),
                other = dep.needs.label()
            )
        };
        v_flex()
            .gap(px(2.))
            .child(div().child(need.to_string()))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .font_family(cx.theme().mono.clone())
                    .child(format!("{} · {found}", dep.range)),
            )
            .into_any_element()
    }
}

impl Render for CrashDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let cause = &self.diagnosis.cause;
        let body = match cause {
            Cause::Dependencies(deps) => v_flex()
                .gap(px(10.))
                .children(deps.iter().map(|d| self.dependency(d, cx)))
                .into_any_element(),
            Cause::Mod { error, .. } => div()
                .p(px(10.))
                .rounded(px(6.))
                .bg(c.row)
                .text_size(px(12.))
                .line_height(relative(1.5))
                .font_family(cx.theme().mono.clone())
                .text_color(c.text2)
                .child(error.clone())
                .into_any_element(),
            _ => div().into_any_element(),
        };
        let report = self.diagnosis.source.clone();
        let file = culprit(cause).and_then(|m| m.file.clone());
        let mut buttons = vec![
            Button::new("close")
                .outline()
                .size(ButtonSize::Md)
                .label(t!("common.close"))
                .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx))),
        ];
        if let Some(file) = file {
            buttons.push(
                Button::new("show-mod")
                    .outline()
                    .size(ButtonSize::Md)
                    .label(t!("crash.show_mod"))
                    .on_click(move |_, _, cx| cx.reveal_path(&file)),
            );
        }
        buttons.push(
            Button::new("open-report")
                .primary()
                .size(ButtonSize::Md)
                .label(t!("crash.open_report"))
                .on_click(move |_, _, cx| cx.open_with_system(&report)),
        );
        super::dialogs::dialog_shell(summary(cause), explanation(cause), body, buttons, cx)
    }
}

/// The launch bar's line after a crash: the cause and a button for the details.
pub fn status_line(id: &str, diagnosis: &Diagnosis, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    let shown = diagnosis.clone();
    h_flex()
        .min_w_0()
        .gap(px(8.))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .text_color(c.warn)
                .child(summary(&diagnosis.cause)),
        )
        .child(
            Button::new(SharedString::from(format!("crash-details-{id}")))
                .ghost()
                .size(ButtonSize::Xs)
                .label(t!("crash.details"))
                .on_click(move |_, _, cx| open(shown.clone(), cx)),
        )
        .into_any_element()
}
