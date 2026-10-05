use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ClipboardItem, Context, Entity, Hsla, IntoElement, ParentElement as _, Render,
    ScrollStrategy, SharedString, Styled as _, Subscription, UniformListScrollHandle, Window, div,
    px, uniform_list,
};
use rust_i18n::t;

use super::runtime;
use super::state::AppState;
use super::theme::{ActiveTheme as _, Palette};
use super::ui::{Button, IconName, h_flex, icon, scrollbar, v_flex};

const ROW_HEIGHT: f32 = 20.;

/// The Logs tab: the running game's output, or `logs/latest.log` from the last run.
pub struct LogsView {
    id: String,
    game_dir: PathBuf,
    scroll: UniformListScrollHandle,
    /// Lines of `latest.log`, read when no launch from this session has output.
    file: Option<Vec<(bool, SharedString)>>,
    shown: usize,
    _state: Subscription,
}

/// Longest part of a line drawn; text past it is laid out and painted every frame for nothing.
const SHOWN_CHARS: usize = 400;

/// A log line as drawn: cut short when it is very long. Copying still takes the whole line.
pub fn display_line(text: &SharedString) -> SharedString {
    match text.char_indices().nth(SHOWN_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]).into(),
        None => text.clone(),
    }
}

pub fn color(stderr: bool, line: &str, c: &Palette) -> Hsla {
    if stderr || line.contains("/ERROR]") || line.contains("/FATAL]") {
        c.warn
    } else if line.contains("/WARN]") {
        c.warn.opacity(0.75)
    } else if line.contains("/DEBUG]") {
        c.muted
    } else {
        c.text2
    }
}

impl LogsView {
    pub fn new(id: String, game_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        let _state = cx.observe(&state, |_, _, cx| cx.notify());
        let mut view = Self {
            id,
            game_dir,
            scroll: UniformListScrollHandle::new(),
            file: None,
            shown: 0,
            _state,
        };
        view.read_file(cx);
        view
    }

    fn read_file(&mut self, cx: &mut Context<Self>) {
        let path = self.game_dir.join("logs").join("latest.log");
        cx.spawn(async move |this, cx| {
            let lines = runtime::blocking(move || {
                std::fs::read(&path)
                    .map(|bytes| {
                        String::from_utf8_lossy(&bytes)
                            .lines()
                            .map(|l| (false, SharedString::from(riven_launch::strip_ansi(l))))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.file = Some(lines);
                cx.notify();
            });
        })
        .detach();
    }

    fn lines<'a>(&'a self, cx: &'a App) -> LogLines<'a> {
        match AppState::global(cx).read(cx).sessions.get(&self.id) {
            Some(s) if !s.log.is_empty() => LogLines::Session(&s.log),
            _ => LogLines::File(self.file.as_deref().unwrap_or_default()),
        }
    }

    fn copy(&self, cx: &mut App) {
        let text = self.lines(cx).join();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

enum LogLines<'a> {
    Session(&'a std::collections::VecDeque<(bool, SharedString)>),
    File(&'a [(bool, SharedString)]),
}

impl LogLines<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Session(l) => l.len(),
            Self::File(l) => l.len(),
        }
    }

    fn get(&self, ix: usize) -> Option<&(bool, SharedString)> {
        match self {
            Self::Session(l) => l.get(ix),
            Self::File(l) => l.get(ix),
        }
    }

    fn join(&self) -> String {
        (0..self.len())
            .filter_map(|i| self.get(i))
            .map(|(_, l)| l.as_ref())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Render for LogsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let c = theme.colors;
        let mono = theme.mono.clone();
        let (len, live) = {
            let lines = self.lines(cx);
            (lines.len(), matches!(lines, LogLines::Session(_)))
        };
        if len != self.shown {
            let following = self.shown == 0 || self.scroll.is_scrolled_to_end().unwrap_or(true);
            self.shown = len;
            if following && len > 0 {
                self.scroll.scroll_to_item(len - 1, ScrollStrategy::Bottom);
            }
        }
        let dir = self.game_dir.join("logs");
        let source = if live {
            t!("logs.session")
        } else {
            t!("logs.latest")
        };
        let toolbar = h_flex()
            .flex_none()
            .gap(px(10.))
            .px(px(18.))
            .py(px(10.))
            .border_b_1()
            .border_color(c.row)
            .child(
                h_flex()
                    .flex_1()
                    .gap(px(8.))
                    .text_color(c.muted)
                    .child(icon(IconName::Terminal, c.muted))
                    .child(source.to_string())
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_size(px(11.))
                            .child(t!("logs.lines", n = len).to_string()),
                    ),
            )
            .when(!live, |row| {
                row.child(
                    Button::new("reload-log")
                        .label(t!("logs.reload"))
                        .on_click(cx.listener(|this, _, _, cx| this.read_file(cx))),
                )
            })
            .child(
                Button::new("copy-log")
                    .label(t!("logs.copy"))
                    .disabled(len == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
            )
            .child(
                Button::new("open-logs")
                    .label(t!("instance.open_folder"))
                    .on_click(move |_, _, cx| cx.open_with_system(&dir)),
            );
        let body = if len == 0 {
            super::app::placeholder(
                IconName::Terminal,
                t!("logs.empty_title").into(),
                t!("logs.empty_hint").into(),
                cx,
            )
            .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    uniform_list(
                        "log",
                        len,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            let c = cx.theme().colors;
                            let lines = this.lines(cx);
                            range
                                .filter_map(|ix| {
                                    let (stderr, text) = lines.get(ix)?;
                                    Some(
                                        div()
                                            .h(px(ROW_HEIGHT))
                                            .px(px(18.))
                                            .whitespace_nowrap()
                                            .text_color(color(*stderr, text, &c))
                                            .child(display_line(text)),
                                    )
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full()
                    .py(px(6.))
                    .font_family(mono)
                    .text_size(px(12.))
                    .line_height(px(ROW_HEIGHT))
                    .track_scroll(&self.scroll),
                )
                .child(scrollbar(&self.scroll))
                .into_any_element()
        };
        v_flex().size_full().child(toolbar).child(body)
    }
}

pub fn view(id: String, game_dir: PathBuf, cx: &mut App) -> Entity<LogsView> {
    use gpui_kit::AppContext as _;
    cx.new(|cx| LogsView::new(id, game_dir, cx))
}
