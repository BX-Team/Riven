use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, DragMoveEvent, EmptyView, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px, uniform_list,
};
use riven_format::DevPanel;
use rust_i18n::t;

use super::{Check, DevView, Panel};
use crate::gui::logs::display_line;
use crate::gui::session::Phase;
use crate::gui::state::AppState;
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{Button, ButtonSize, IconName, h_flex, icon, motion, scrollbar, v_flex};

const ROW: f32 = 18.;
const HEADER: f32 = 39.;
const MIN_HEIGHT: f32 = 90.;
/// Space the editor keeps above the panel at the least.
const MIN_ABOVE: f32 = 220.;
/// Lines of the test run's log the panel shows, the newest last.
const LOG_TAIL: usize = 5_000;

/// What is dragged while the panel's top edge is being moved.
pub(super) struct PanelResize;

impl DevView {
    fn panel_prefs(&self, cx: &Context<Self>) -> DevPanel {
        AppState::global(cx).read(cx).settings.dev_panel
    }

    pub(super) fn panel_height(&self, cx: &Context<Self>) -> f32 {
        self.panel_drag
            .unwrap_or(self.panel_prefs(cx).height as f32)
            .max(MIN_HEIGHT)
    }

    fn set_panel(&mut self, edit: impl FnOnce(&mut DevPanel), cx: &mut Context<Self>) {
        AppState::global(cx).update(cx, |s, cx| {
            s.update_settings(|p| edit(&mut p.dev_panel), cx)
        });
    }

    /// Shows the panel on `panel`, or folds it when that tab is already open.
    fn pick_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        let hidden = self.panel_prefs(cx).hidden;
        if hidden || self.panel != panel {
            self.panel = panel;
            if hidden {
                self.set_panel(|p| p.hidden = false, cx);
            }
        } else {
            self.set_panel(|p| p.hidden = true, cx);
        }
        self.panel_seen = usize::MAX;
        cx.notify();
    }

    pub(super) fn drag_panel(
        &mut self,
        event: &DragMoveEvent<PanelResize>,
        cx: &mut Context<Self>,
    ) {
        let bounds = event.bounds;
        let height = f32::from(bounds.bottom() - event.event.position.y);
        let max = (f32::from(bounds.size.height) - MIN_ABOVE).max(MIN_HEIGHT);
        self.panel_drag = Some(height.clamp(MIN_HEIGHT, max));
        cx.notify();
    }

    pub(super) fn drop_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(height) = self.panel_drag.take() {
            self.set_panel(|p| p.height = height.round() as u32, cx);
        }
    }

    /// Rows of the Git or Log stream, a trailing status row included.
    fn stream_len(&self, cx: &Context<Self>) -> usize {
        match self.panel {
            Panel::Check => 0,
            Panel::Git => self.git.output.len() + usize::from(self.git.running.is_some()),
            Panel::Log => {
                let state = AppState::global(cx);
                let s = state.read(cx);
                self.test_instance()
                    .and_then(|id| s.sessions.get(&id))
                    .map_or(0, |session| session.log.len().min(LOG_TAIL) + 1)
            }
        }
    }

    fn status_row(
        &self,
        spin: bool,
        color: gpui_kit::Hsla,
        text: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        h_flex()
            .h(px(ROW))
            .gap(px(8.))
            .text_color(color)
            .when(spin, |row| {
                row.child(motion::spinner(
                    "dev-stream-spin",
                    icon(IconName::Loader, c.muted).size(px(12.)),
                    cx,
                ))
            })
            .child(text)
            .into_any_element()
    }

    fn stream_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors;
        let line = |text: &SharedString, color| {
            div()
                .h(px(ROW))
                .whitespace_nowrap()
                .text_color(color)
                .child(display_line(text))
                .into_any_element()
        };
        match self.panel {
            Panel::Check => None,
            Panel::Git => match self.git.output.get(ix) {
                Some((error, text)) => Some(if text.starts_with("$ ") {
                    div()
                        .h(px(ROW))
                        .whitespace_nowrap()
                        .text_color(c.text)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(display_line(text))
                        .into_any_element()
                } else {
                    line(text, if *error { c.warn } else { c.text2 })
                }),
                None => {
                    let label = self.git.running.clone()?;
                    Some(self.status_row(true, c.muted, label.to_string(), cx))
                }
            },
            Panel::Log => {
                let state = AppState::global(cx);
                let (row, phase) = {
                    let s = state.read(cx);
                    let session = self.test_instance().and_then(|id| s.sessions.get(&id))?;
                    let skip = session.log.len().saturating_sub(LOG_TAIL);
                    let row = session.log.get(skip + ix).map(|(stderr, text)| {
                        let color = crate::gui::logs::color(*stderr, text, &c);
                        line(text, color)
                    });
                    (row, session.phase.clone())
                };
                if row.is_some() {
                    return row;
                }
                let (spin, color, text) = match phase {
                    Phase::Working { stage, .. } => {
                        (true, c.muted, crate::gui::launch_bar::stage_label(stage))
                    }
                    Phase::Running { pid, .. } => (
                        false,
                        c.muted,
                        t!("dev.test_running", pid = pid).to_string(),
                    ),
                    Phase::Finished { code } => (
                        false,
                        c.muted,
                        t!(
                            "dev.test_exited",
                            code = code.map_or("—".into(), |c| c.to_string())
                        )
                        .to_string(),
                    ),
                    Phase::Failed(e) => (false, c.warn, e.to_string()),
                };
                Some(self.status_row(spin, color, text, cx))
            }
        }
    }

    /// Keeps a growing stream scrolled to its end, unless the user scrolled up.
    pub(super) fn follow_stream(&mut self, cx: &Context<Self>) {
        let len = self.stream_len(cx);
        if len == self.panel_seen {
            return;
        }
        let following = self.panel_seen == usize::MAX
            || self.panel_seen == 0
            || self.panel_list.is_scrolled_to_end().unwrap_or(true);
        self.panel_seen = len;
        if following && len > 0 {
            self.panel_list
                .scroll_to_item(len - 1, ScrollStrategy::Bottom);
        }
    }

    fn check_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors;
        let line = |mark: &'static str, color, text: String| {
            h_flex()
                .items_start()
                .gap(px(8.))
                .child(div().flex_none().text_color(color).child(mark))
                .child(div().flex_1().min_w_0().child(text))
                .into_any_element()
        };
        match &self.check {
            Check::Idle => vec![],
            Check::Running => {
                vec![self.status_row(true, c.muted, t!("dev.checking").to_string(), cx)]
            }
            Check::Failed(e) => vec![line("✕", crate::gui::theme::danger(), e.to_string())],
            Check::Done(report) => {
                let n = self
                    .project
                    .as_ref()
                    .map_or(0, |ws| ws.project.content.len());
                let mut out: Vec<AnyElement> = report
                    .issues
                    .iter()
                    .cloned()
                    .chain(report.problems.iter().map(ToString::to_string))
                    .map(|text| line("✕", crate::gui::theme::danger(), text))
                    .collect();
                out.extend(
                    report
                        .unreadable
                        .iter()
                        .map(|text| line("!", c.warn, text.clone())),
                );
                if report.is_clean() {
                    out.insert(0, line("✓", c.ok, t!("dev.check_clean", n = n).to_string()));
                }
                out
            }
        }
    }

    fn problem_count(&self) -> usize {
        match &self.check {
            Check::Done(r) => r.issues.len() + r.problems.len(),
            Check::Failed(_) => 1,
            _ => 0,
        }
    }

    fn panel_header(&self, hidden: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let tab = |panel: Panel, label: SharedString, cx: &mut Context<Self>| {
            let on = self.panel == panel && !hidden;
            div()
                .id(SharedString::from(format!("dev-panel-{panel:?}")))
                .h(px(26.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .map(|d| {
                    if on {
                        d.bg(c.sel)
                            .text_color(c.text)
                            .font_weight(FontWeight::SEMIBOLD)
                    } else {
                        d.text_color(c.muted).hover(|s| s.text_color(c.text))
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| this.pick_panel(panel, cx)))
                .child(label)
        };
        let problems = self.problem_count();
        let check_label: SharedString = if problems > 0 {
            format!("{} · {problems}", t!("dev.check")).into()
        } else {
            t!("dev.check").into()
        };
        let log_live = self
            .test_instance()
            .and_then(|id| {
                AppState::global(cx)
                    .read(cx)
                    .sessions
                    .get(&id)
                    .map(|s| s.busy())
            })
            .unwrap_or(false);
        h_flex()
            .flex_none()
            .h(px(HEADER))
            .gap(px(2.))
            .px(px(10.))
            .when(!hidden, |row| row.border_b_1().border_color(c.border))
            .child(
                tab(Panel::Check, check_label, cx).when(problems > 0 && hidden, |t| {
                    t.text_color(crate::gui::theme::danger())
                }),
            )
            .child(tab(Panel::Git, "Git".into(), cx))
            .child(
                tab(Panel::Log, t!("dev.log").into(), cx).when(log_live, |t| {
                    t.child(div().ml(px(6.)).text_color(c.ok).child("●"))
                }),
            )
            .child(div().flex_1())
            .when(!hidden && self.panel == Panel::Git, |row| {
                row.child(self.git_clear_button(cx))
            })
            .when(
                !hidden && self.panel == Panel::Log && self.log_len(cx) > 0,
                |row| {
                    row.child(
                        Button::new("dev-open-test")
                            .ghost()
                            .size(ButtonSize::Xs)
                            .icon(IconName::ChevronRight)
                            .tooltip(t!("dev.test_open"))
                            .on_click(cx.listener(|this, _, _, cx| this.open_test_instance(cx))),
                    )
                },
            )
            .when(!hidden && self.panel == Panel::Check, |row| {
                row.child(
                    Button::new("dev-recheck")
                        .ghost()
                        .size(ButtonSize::Xs)
                        .icon(IconName::Refresh)
                        .tooltip(t!("dev.recheck"))
                        .disabled(matches!(self.check, Check::Running))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_check(cx);
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("dev-panel-fold")
                    .ghost()
                    .size(ButtonSize::Xs)
                    .icon(if hidden {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    })
                    .tooltip(if hidden {
                        t!("dev.panel_show")
                    } else {
                        t!("dev.panel_hide")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_panel(|p| p.hidden = !hidden, cx);
                        this.panel_seen = usize::MAX;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let hidden = self.panel_prefs(cx).hidden;
        if hidden {
            return div()
                .flex_none()
                .border_t_1()
                .border_color(c.border)
                .bg(c.panel)
                .child(self.panel_header(true, cx))
                .into_any_element();
        }
        let body: AnyElement = match self.panel {
            Panel::Check => v_flex()
                .id("dev-panel-check")
                .size_full()
                .overflow_y_scroll()
                .px(px(14.))
                .py(px(10.))
                .gap(px(4.))
                .children(self.check_rows(cx))
                .into_any_element(),
            panel => {
                let len = self.stream_len(cx);
                if len == 0 {
                    let hint = match panel {
                        Panel::Git => t!("dev.git_output_empty"),
                        _ => t!("dev.log_empty"),
                    };
                    div()
                        .px(px(14.))
                        .py(px(10.))
                        .text_color(c.muted)
                        .child(hint.to_string())
                        .into_any_element()
                } else {
                    div()
                        .relative()
                        .size_full()
                        .child(
                            uniform_list(
                                "dev-panel-stream",
                                len,
                                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                    range
                                        .filter_map(|ix| this.stream_row(ix, cx))
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .size_full()
                            .px(px(14.))
                            .py(px(8.))
                            .line_height(px(ROW))
                            .track_scroll(&self.panel_list),
                        )
                        .child(scrollbar(&self.panel_list))
                        .into_any_element()
                }
            }
        };
        let handle = div()
            .id("dev-panel-resize")
            .absolute()
            .top(px(-3.))
            .left_0()
            .right_0()
            .h(px(6.))
            .cursor_row_resize()
            .on_drag(PanelResize, |_, _, _, cx| cx.new(|_| EmptyView));
        v_flex()
            .relative()
            .flex_none()
            .h(px(self.panel_height(cx)))
            .border_t_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(self.panel_header(false, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .font_family(mono)
                    .text_size(px(12.))
                    .text_color(c.text2)
                    .child(body),
            )
            .child(handle)
            .into_any_element()
    }
}
