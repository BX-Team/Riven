use std::path::{Path, PathBuf};

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, px, relative,
};
use riven_launch::crash::{Cause, Dependency, Diagnosis, Native, Ticking, TickingKind, Vendor};
use rust_i18n::t;

use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::toast::ToastKind;
use super::ui::{Button, ButtonSize, h_flex, v_flex};

/// One line for the launch bar: what most likely went wrong.
pub fn summary(cause: &Cause) -> SharedString {
    match cause {
        Cause::Dependencies(_) => t!("crash.dependencies"),
        Cause::Duplicates(found) => match found.as_slice() {
            [one] => t!("crash.duplicate", name = one.culprit.label()),
            _ => t!("crash.duplicates"),
        },
        Cause::Overlap { first, second, .. } => t!(
            "crash.overlap",
            name = first.label(),
            other = second.label()
        ),
        Cause::Mismatch { culprit, .. } => t!("crash.mismatch", name = culprit.label()),
        Cause::MissingClass { .. } => t!("crash.missing_class"),
        Cause::Mixin(culprit) => t!("crash.mixin", name = culprit.label()),
        Cause::Load { culprit, .. } => t!("crash.load", name = culprit.label()),
        Cause::Java { needed } => t!("crash.java", version = needed),
        Cause::JavaTooNew => t!("crash.java_too_new"),
        Cause::Memory => t!("crash.memory"),
        Cause::SystemMemory => t!("crash.system_memory"),
        Cause::HeapSize => t!("crash.heap_size"),
        Cause::Native(Native::Driver(Some(vendor))) => {
            t!("crash.driver", vendor = vendor_name(*vendor))
        }
        Cause::Native(Native::Driver(None)) => t!("crash.driver_unknown"),
        Cause::Native(Native::Java) => t!("crash.java_crashed"),
        Cause::Native(Native::Library(name)) => t!("crash.library", name = name),
        Cause::Graphics => t!("crash.graphics"),
        Cause::DiskFull => t!("crash.disk_full"),
        Cause::Corrupt { file: Some(file) } => t!("crash.corrupt_file", file = file),
        Cause::Corrupt { file: None } => t!("crash.corrupt"),
        Cause::Config {
            culprit: Some(culprit),
            ..
        } => t!("crash.config_mod", name = culprit.label()),
        Cause::Config { .. } => t!("crash.config"),
        Cause::Mod { culprit, .. } => t!("crash.mod", name = culprit.label()),
        Cause::Ticking(Ticking {
            kind: TickingKind::Entity,
            ..
        }) => t!("crash.ticking_entity"),
        Cause::Ticking(_) => t!("crash.ticking_block"),
    }
    .into()
}

fn explanation(cause: &Cause) -> SharedString {
    match cause {
        Cause::Dependencies(_) => t!("crash.dependencies_hint"),
        Cause::Duplicates(_) => t!("crash.duplicates_hint"),
        Cause::Overlap { package, .. } => t!("crash.overlap_hint", package = package),
        Cause::Mismatch {
            culprit,
            other: Some(other),
        } => t!(
            "crash.mismatch_hint",
            name = culprit.label(),
            other = other.label()
        ),
        Cause::Mismatch { culprit, .. } => t!("crash.mismatch_game_hint", name = culprit.label()),
        Cause::MissingClass { .. } => t!("crash.missing_class_hint"),
        Cause::Mixin(culprit) => t!("crash.mixin_hint", name = culprit.label()),
        Cause::Load { culprit, .. } => t!("crash.load_hint", name = culprit.label()),
        Cause::Java { needed } => t!("crash.java_hint", version = needed),
        Cause::JavaTooNew => t!("crash.java_too_new_hint"),
        Cause::Memory => t!("crash.memory_hint"),
        Cause::SystemMemory => t!("crash.system_memory_hint"),
        Cause::HeapSize => t!("crash.heap_size_hint"),
        Cause::Native(Native::Driver(_)) => t!("crash.driver_hint"),
        Cause::Native(Native::Java) => t!("crash.java_crashed_hint"),
        Cause::Native(Native::Library(name)) => t!("crash.library_hint", name = name),
        Cause::Graphics => t!("crash.graphics_hint"),
        Cause::DiskFull => t!("crash.disk_full_hint"),
        Cause::Corrupt { .. } => t!("crash.corrupt_hint"),
        Cause::Config { .. } => t!("crash.config_hint"),
        Cause::Mod { culprit, .. } => t!("crash.mod_hint", name = culprit.label()),
        Cause::Ticking(Ticking {
            kind: TickingKind::Entity,
            ..
        }) => t!("crash.ticking_entity_hint"),
        Cause::Ticking(_) => t!("crash.ticking_block_hint"),
    }
    .into()
}

fn vendor_name(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Nvidia => "NVIDIA",
        Vendor::Amd => "AMD",
        Vendor::Intel => "Intel",
    }
}

pub fn open(id: String, diagnosis: Diagnosis, cx: &mut App) {
    let view = cx.new(|_| CrashDialog { id, diagnosis });
    super::dialogs::open(view.into(), 620., cx);
}

struct CrashDialog {
    id: String,
    diagnosis: Diagnosis,
}

/// A muted monospace box for an error message, a class or a file.
fn code(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    div()
        .p(px(10.))
        .rounded(px(6.))
        .bg(c.row)
        .text_size(px(12.))
        .line_height(relative(1.5))
        .font_family(cx.theme().mono.clone())
        .text_color(c.text2)
        .child(text.into())
        .into_any_element()
}

/// A line of text with a muted monospace detail under it.
fn entry(title: String, detail: String, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    v_flex()
        .gap(px(2.))
        .child(div().child(title))
        .child(
            div()
                .text_size(px(12.))
                .text_color(c.muted)
                .font_family(cx.theme().mono.clone())
                .child(detail),
        )
        .into_any_element()
}

fn dependency(dep: &Dependency, cx: &App) -> AnyElement {
    let found = match &dep.found {
        Some(v) => t!("crash.found", version = v),
        None => t!("crash.missing"),
    };
    let key = if dep.incompatible {
        "crash.breaks"
    } else {
        "crash.needs"
    };
    entry(
        t!(key, name = dep.by.label(), other = dep.needs.label()).into_owned(),
        format!("{} · {found}", dep.range),
        cx,
    )
}

fn ticking(ticking: &Ticking, cx: &App) -> AnyElement {
    let title = match ticking.kind {
        TickingKind::Entity => t!("crash.place_entity"),
        TickingKind::Block => t!("crash.place_block"),
    };
    let detail = match &ticking.at {
        Some(at) => format!("{} · {at}", ticking.id),
        None => ticking.id.clone(),
    };
    entry(title.into_owned(), detail, cx)
}

/// The mod file the player may switch off from here: not nested, still there, in an editable instance.
fn disableable(id: &str, file: &Path, cx: &App) -> bool {
    let state = AppState::global(cx).read(cx);
    let (Some(instance), Some(store)) = (state.instance(id), state.store.as_ref()) else {
        return false;
    };
    file.is_file() && riven_launch::own::editable(instance, &store.game_dir(id))
}

fn disable(id: &str, file: &Path, name: &str, cx: &mut App) {
    let state = AppState::global(cx);
    let (instance, store) = {
        let s = state.read(cx);
        (s.instance(id).cloned(), s.store.clone())
    };
    let (Some(instance), Some(store)) = (instance, store) else {
        return;
    };
    let result = riven_launch::own::disable_mod(&instance, &store.game_dir(id), file);
    state.update(cx, |s, cx| match result {
        Ok(()) => {
            *s.revisions.entry(id.to_owned()).or_default() += 1;
            s.toast(ToastKind::Success, t!("crash.disabled", name = name), cx);
            s.close_modal(cx);
        }
        Err(e) => s.toast(ToastKind::Error, e.to_string(), cx),
    });
}

impl Render for CrashDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cause = &self.diagnosis.cause;
        let mut parts: Vec<AnyElement> = match cause {
            Cause::Dependencies(deps) => deps.iter().map(|d| dependency(d, cx)).collect(),
            Cause::Duplicates(found) => found
                .iter()
                .map(|d| entry(d.culprit.label().to_owned(), d.files.join(", "), cx))
                .collect(),
            Cause::MissingClass { class } => vec![code(class.clone(), cx)],
            Cause::Load { error, .. } | Cause::Mod { error, .. } if !error.is_empty() => {
                vec![code(error.clone(), cx)]
            }
            Cause::Config {
                file: Some(file), ..
            } => vec![code(file.clone(), cx)],
            Cause::Ticking(t) => vec![ticking(t, cx)],
            _ => Vec::new(),
        };
        if let Some(t) = &self.diagnosis.ticking {
            parts.push(ticking(t, cx));
        }
        let body = v_flex().gap(px(10.)).children(parts);

        let mut buttons = vec![
            Button::new("close")
                .outline()
                .size(ButtonSize::Md)
                .label(t!("common.close"))
                .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx))),
        ];
        if let Some(culprit) = cause.culprit()
            && let Some(file) = culprit.file.clone()
        {
            if !culprit.nested && disableable(&self.id, &file, cx) {
                let (id, name, file) = (self.id.clone(), culprit.label().to_owned(), file.clone());
                buttons.push(
                    Button::new("disable-mod")
                        .outline()
                        .size(ButtonSize::Md)
                        .label(t!("crash.disable_mod"))
                        .on_click(move |_, _, cx| disable(&id, &file, &name, cx)),
                );
            }
            buttons.push(
                Button::new("show-mod")
                    .outline()
                    .size(ButtonSize::Md)
                    .label(t!("crash.show_mod"))
                    .on_click(move |_, _, cx| cx.reveal_path(&file)),
            );
        }
        if let Some(report) = self.diagnosis.source.clone() {
            buttons.push(
                Button::new("open-report")
                    .primary()
                    .size(ButtonSize::Md)
                    .label(t!("crash.open_report"))
                    .on_click(move |_, _, cx| cx.open_with_system(&report)),
            );
        }
        super::dialogs::dialog_shell(summary(cause), explanation(cause), body, buttons, cx)
    }
}

/// The launch bar's line after a crash: the cause and a button for the details.
pub fn status_line(id: &str, diagnosis: &Diagnosis, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
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
        .child(details_button(id, diagnosis, ButtonSize::Xs))
        .into_any_element()
}

/// The Logs tab's strip above a crash report: the cause, its explanation and the details.
pub fn banner(id: &str, diagnosis: &Diagnosis, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    h_flex()
        .flex_none()
        .gap(px(12.))
        .px(px(18.))
        .py(px(10.))
        .border_b_1()
        .border_color(c.row)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .text_color(c.warn)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(summary(&diagnosis.cause)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(c.muted)
                        .line_height(relative(1.4))
                        .child(explanation(&diagnosis.cause)),
                ),
        )
        .child(details_button(id, diagnosis, ButtonSize::Sm))
        .into_any_element()
}

fn details_button(id: &str, diagnosis: &Diagnosis, size: ButtonSize) -> Button {
    let (id, shown) = (id.to_owned(), diagnosis.clone());
    Button::new(SharedString::from(format!("crash-details-{id}")))
        .ghost()
        .size(size)
        .label(t!("crash.details"))
        .on_click(move |_, _, cx| open(id.clone(), shown.clone(), cx))
}

/// Whether a log file is one the analysis reads: a crash report or a JVM crash log.
pub fn explains(game_dir: &Path, path: &Path) -> bool {
    path.parent() == Some(&game_dir.join("crash-reports")) || riven_launch::crash::is_jvm(path)
}

/// Explains a crash report or JVM log, for a background thread; `None` for other logs.
pub fn explain(game_dir: PathBuf, path: PathBuf) -> Option<Diagnosis> {
    explains(&game_dir, &path)
        .then(|| riven_launch::crash::explain(&game_dir, &path))
        .flatten()
}
