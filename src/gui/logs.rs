use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ClipboardItem, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, UniformListScrollHandle, Window, div, px, uniform_list,
};
use rust_i18n::t;

use super::runtime;
use super::state::AppState;
use super::theme::{ActiveTheme as _, Palette};
use super::ui::{Button, IconName, h_flex, icon, scrollbar, v_flex};
use riven_launch::logs::{LogFile, LogKind};

const ROW_HEIGHT: f32 = 20.;
const LIST_WIDTH: f32 = 230.;

/// What the log pane shows.
#[derive(Clone, PartialEq, Eq)]
enum Source {
    /// This launcher session's game output, else `latest.log`.
    Current,
    File(PathBuf),
}

/// The Logs tab: the running game's output, older logs and crash reports.
pub struct LogsView {
    id: String,
    game_dir: PathBuf,
    scroll: UniformListScrollHandle,
    source: Source,
    files: Vec<LogFile>,
    /// Lines of the file shown, or of `latest.log` when no launch from this session has output.
    file: Option<Vec<(bool, SharedString)>>,
    shown: usize,
    uploading: bool,
    /// The last paste of what is shown.
    pasted: Option<String>,
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
            source: Source::Current,
            files: Vec::new(),
            file: None,
            shown: 0,
            uploading: false,
            pasted: None,
            _state,
        };
        view.read_file(cx);
        view
    }

    /// Lists the logs folder and reads what is selected again.
    fn read_file(&mut self, cx: &mut Context<Self>) {
        let path = match &self.source {
            Source::Current => self.game_dir.join("logs").join("latest.log"),
            Source::File(path) => path.clone(),
        };
        let dir = self.game_dir.clone();
        cx.spawn(async move |this, cx| {
            let (files, lines) = runtime::blocking(move || {
                let lines = riven_launch::logs::read(&path)
                    .map(|text| {
                        text.lines()
                            .map(|l| (false, SharedString::from(riven_launch::strip_ansi(l))))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                (riven_launch::logs::list(&dir), lines)
            })
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.files = files;
                this.file = Some(lines);
                this.shown = 0;
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, source: Source, cx: &mut Context<Self>) {
        if self.source != source {
            self.source = source;
            self.file = None;
            self.pasted = None;
            self.read_file(cx);
        }
    }

    fn lines<'a>(&'a self, cx: &'a App) -> LogLines<'a> {
        match (
            &self.source,
            AppState::global(cx).read(cx).sessions.get(&self.id),
        ) {
            (Source::Current, Some(s)) if !s.log.is_empty() => LogLines::Session(&s.log),
            _ => LogLines::File(self.file.as_deref().unwrap_or_default()),
        }
    }

    fn upload(&mut self, cx: &mut Context<Self>) {
        if self.uploading {
            return;
        }
        let text = self.lines(cx).join();
        self.uploading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let posted = runtime::spawn(async move { riven_launch::logs::upload(&text).await })
                .await
                .unwrap_or_else(|_| Err(riven_launch::LaunchError::Upload("cancelled".into())));
            let _ = this.update(cx, |this, cx| {
                this.uploading = false;
                let state = AppState::global(cx);
                match posted {
                    Ok(url) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                        state.update(cx, |s, cx| {
                            s.toast(
                                super::toast::ToastKind::Success,
                                t!("logs.uploaded", url = url).to_string(),
                                cx,
                            )
                        });
                        this.pasted = Some(url);
                    }
                    Err(e) => state.update(cx, |s, cx| {
                        s.toast(super::toast::ToastKind::Error, e.to_string(), cx)
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let row = |id: SharedString,
                   label: String,
                   detail: Option<String>,
                   on: bool,
                   tint: Option<Hsla>,
                   source: Source,
                   cx: &mut Context<Self>| {
            v_flex()
                .id(id)
                .px(px(8.))
                .py(px(5.))
                .rounded(px(6.))
                .cursor_pointer()
                .map(|r| {
                    if on {
                        r.bg(c.sel)
                    } else {
                        r.hover(|s| s.bg(c.row))
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| this.select(source.clone(), cx)))
                .child(
                    div()
                        .truncate()
                        .text_color(tint.unwrap_or(if on { c.text } else { c.text2 }))
                        .child(label),
                )
                .children(detail.map(|d| div().text_size(px(11.)).text_color(c.muted).child(d)))
        };
        let mut items = vec![
            row(
                "log-current".into(),
                t!("logs.current").to_string(),
                None,
                self.source == Source::Current,
                None,
                Source::Current,
                cx,
            )
            .into_any_element(),
        ];
        for (kind, caption) in [
            (LogKind::Crash, t!("logs.crashes")),
            (LogKind::Log, t!("logs.files")),
        ] {
            let files: Vec<&LogFile> = self.files.iter().filter(|f| f.kind == kind).collect();
            if files.is_empty() {
                continue;
            }
            items.push(
                super::ui::caption(caption, cx)
                    .mt(px(10.))
                    .into_any_element(),
            );
            for (i, f) in files.into_iter().enumerate() {
                let source = Source::File(f.path.clone());
                let on = self.source == source;
                let tint = (kind == LogKind::Crash).then_some(c.warn);
                items.push(
                    row(
                        SharedString::from(format!("log-{kind:?}-{i}")),
                        f.name.clone(),
                        Some(super::time::ago(f.modified).to_string()),
                        on,
                        tint,
                        source,
                        cx,
                    )
                    .into_any_element(),
                );
            }
        }
        v_flex()
            .id("log-files")
            .w(px(LIST_WIDTH))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .p(px(8.))
            .gap(px(1.))
            .border_r_1()
            .border_color(c.border)
            .text_size(px(12.))
            .children(items)
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
        let source: SharedString = match (&self.source, live) {
            (_, true) => t!("logs.session").into(),
            (Source::Current, false) => t!("logs.latest").into(),
            (Source::File(path), false) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
                .into(),
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
                        .ghost()
                        .icon(IconName::Refresh)
                        .tooltip(t!("logs.reload"))
                        .on_click(cx.listener(|this, _, _, cx| this.read_file(cx))),
                )
            })
            .when_some(self.pasted.clone(), |row, url| {
                row.child(
                    Button::new("open-paste")
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .tooltip(url.clone())
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .child(
                Button::new("upload-log")
                    .icon(IconName::Upload)
                    .label(if self.uploading {
                        t!("logs.uploading")
                    } else {
                        t!("logs.upload")
                    })
                    .tooltip(t!("logs.upload_hint"))
                    .disabled(len == 0 || self.uploading)
                    .on_click(cx.listener(|this, _, _, cx| this.upload(cx))),
            )
            .child(
                Button::new("copy-log")
                    .icon(IconName::Copy)
                    .label(t!("logs.copy"))
                    .disabled(len == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
            )
            .child(
                Button::new("open-logs")
                    .ghost()
                    .icon(IconName::Folder)
                    .tooltip(t!("instance.open_folder"))
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
        h_flex()
            .size_full()
            .items_start()
            .child(self.render_list(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(toolbar)
                    .child(body),
            )
    }
}

pub fn view(id: String, game_dir: PathBuf, cx: &mut App) -> Entity<LogsView> {
    use gpui_kit::AppContext as _;
    cx.new(|cx| LogsView::new(id, game_dir, cx))
}
