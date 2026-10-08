use std::collections::VecDeque;
use std::path::PathBuf;

use gpui_kit::base::input::TextareaState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use riven_build::git::{self, DiffLine, FileState, GitError, Status};
use riven_format::PackPath;
use rust_i18n::t;
use tokio::sync::mpsc;

use super::sections::section_head;
use super::{DevView, Panel, Tab};
use crate::gui::runtime;
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{Button, ButtonSize, IconName, TextArea, caption, h_flex, v_flex};

/// Lines of git output the panel keeps.
const OUTPUT_LINES: usize = 500;
/// Changed files and pack changes listed before "and N more".
const SHOWN: usize = 200;

pub(super) enum Repo {
    Unknown,
    Loading,
    /// The project folder is not inside a git repository.
    None,
    Ready(Box<Status>),
    Failed(SharedString),
}

/// The Git section's state: what gix read, and the output of the last git commands.
pub(super) struct GitState {
    pub(super) repo: Repo,
    /// What changed in `riven.json` since `HEAD`, read along with the status.
    diff: Vec<DiffLine>,
    /// A read is under way; `again` asks for one more once it ends.
    reading: bool,
    again: bool,
    /// Whether a `git` executable runs; reading works without one.
    pub(super) installed: bool,
    message: Entity<TextareaState>,
    pub(super) output: VecDeque<(bool, SharedString)>,
    pub(super) running: Option<SharedString>,
    /// The commit went through: empty the message on the next frame, which has a window.
    pub(super) clear_message: bool,
}

impl GitState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<DevView>) -> Self {
        Self {
            repo: Repo::Unknown,
            diff: Vec::new(),
            reading: false,
            again: false,
            installed: git::available(),
            message: crate::gui::ui::textarea(String::new(), (3, 8), window, cx),
            output: VecDeque::new(),
            running: None,
            clear_message: false,
        }
    }
}

/// A git command that changes the repository or its remote.
#[derive(Debug, Clone)]
pub(super) enum GitOp {
    Commit(String),
    Pull,
    Push { has_upstream: bool },
    Clone { url: String, dest: PathBuf },
}

impl GitOp {
    fn label(&self) -> SharedString {
        match self {
            GitOp::Commit(_) => t!("dev.git_committing"),
            GitOp::Pull => t!("dev.git_pulling"),
            GitOp::Push { .. } => t!("dev.git_pushing"),
            GitOp::Clone { .. } => t!("dev.git_cloning"),
        }
        .into()
    }

    fn command(&self) -> String {
        match self {
            GitOp::Commit(_) => "$ git add --all . && git commit".into(),
            GitOp::Pull => "$ git pull --ff-only".into(),
            GitOp::Push { has_upstream: true } => "$ git push".into(),
            GitOp::Push {
                has_upstream: false,
            } => "$ git push --set-upstream origin HEAD".into(),
            GitOp::Clone { url, .. } => format!("$ git clone {url}"),
        }
    }
}

fn state_mark(state: FileState) -> &'static str {
    match state {
        FileState::Conflict => "!",
        FileState::Modified => "M",
        FileState::Added => "A",
        FileState::Removed => "D",
        FileState::Renamed => "R",
        FileState::Untracked => "?",
    }
}

impl DevView {
    /// Reads the repository again in the background.
    pub(super) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.project.as_ref() else {
            return;
        };
        if self.git.reading {
            self.git.again = true;
            return;
        }
        let (dir, project) = (ws.dir.clone(), ws.project.clone());
        self.git.reading = true;
        if matches!(self.git.repo, Repo::Unknown) {
            self.git.repo = Repo::Loading;
        }
        cx.spawn(async move |this, cx| {
            let result = runtime::blocking(move || {
                let status = git::status(&dir)?;
                let diff = status
                    .as_ref()
                    .map(|s| git::describe(s.committed.as_ref(), &project))
                    .unwrap_or_default();
                Ok((status, diff))
            })
            .await
            .unwrap_or_else(|_| Err(GitError::Read(String::new())));
            let _ = this.update(cx, |this, cx| {
                this.git.reading = false;
                this.git.repo = match result {
                    Ok((Some(status), diff)) => {
                        this.git.diff = diff;
                        Repo::Ready(Box::new(status))
                    }
                    Ok((None, _)) => Repo::None,
                    Err(e) => Repo::Failed(e.to_string().into()),
                };
                if std::mem::take(&mut this.git.again) {
                    this.refresh_git(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn push_output(&mut self, error: bool, line: String) {
        if self.git.output.len() == OUTPUT_LINES {
            self.git.output.pop_front();
        }
        self.git.output.push_back((error, line.into()));
    }

    /// Runs a git command, streaming its output into the Git panel.
    pub(super) fn run_git(
        &mut self,
        op: GitOp,
        done: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.git.running.is_some() {
            return;
        }
        let dir = self.project.as_ref().map(|ws| ws.dir.clone());
        if dir.is_none() && !matches!(op, GitOp::Clone { .. }) {
            return;
        }
        self.git.running = Some(op.label());
        self.panel = Panel::Git;
        self.push_output(false, op.command());
        cx.notify();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let finished = runtime::spawn(async move {
            let line = move |text: String| {
                let _ = tx.send(text);
            };
            match op {
                GitOp::Commit(message) => {
                    git::commit(dir.as_deref().expect("checked"), &message, &line).await
                }
                GitOp::Pull => git::pull(dir.as_deref().expect("checked"), &line).await,
                GitOp::Push { has_upstream } => {
                    git::push(dir.as_deref().expect("checked"), has_upstream, &line).await
                }
                GitOp::Clone { url, dest } => git::clone(&url, &dest, &line).await,
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let alive = this.update(cx, |this, cx| {
                    for line in batch {
                        this.push_output(false, line);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = finished
                .await
                .unwrap_or_else(|_| Err(GitError::Read(String::new())));
            let _ = this.update(cx, |this, cx| {
                this.git.running = None;
                match result {
                    Ok(()) => {
                        this.push_output(false, t!("dev.git_done").to_string());
                        done(this, cx);
                    }
                    Err(e) => {
                        this.push_output(true, e.to_string());
                        this.error = Some(e.to_string().into());
                    }
                }
                this.refresh_git(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let message = self.git.message.read(cx).value().trim().to_string();
        if message.is_empty() {
            self.error = Some(t!("dev.git_no_message").into());
            cx.notify();
            return;
        }
        self.run_git(
            GitOp::Commit(message),
            |this, cx| {
                this.git.clear_message = true;
                this.notice = Some(t!("dev.git_committed").into());
                cx.notify();
            },
            cx,
        );
    }

    /// Empties the commit message after a commit went through.
    pub(super) fn settle_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.git.clear_message) {
            let input = self.git.message.clone();
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
    }

    /// After a pull the files on disk may differ: read the project and its tree again.
    fn reload_project(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.project.as_ref().map(|ws| ws.dir.clone()) else {
            return;
        };
        match riven_build::workspace::Workspace::open(&dir) {
            Ok(ws) => {
                self.project = Some(ws);
                self.forget_project_file();
                self.refresh_tree(cx);
                self.refresh_dist();
                self.refresh_rows(cx);
                self.run_check(cx);
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
    }

    fn open_changed(&mut self, path: &str, cx: &mut Context<Self>) {
        let Ok(pack) = PackPath::new(path) else {
            return;
        };
        let editable = path == riven_build::workspace::PROJECT_FILE
            || self
                .project
                .as_ref()
                .is_some_and(|ws| ws.override_path(&pack).is_ok_and(|p| p.is_file()));
        if editable {
            self.show(Tab::File(pack), cx);
        }
    }

    pub(super) fn render_git(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let busy = self.git.running.is_some();
        let installed = self.git.installed;
        let status = match &self.git.repo {
            Repo::Ready(status) => status,
            other => {
                let text = match other {
                    Repo::None => t!("dev.git_none").to_string(),
                    Repo::Failed(e) => e.to_string(),
                    _ => t!("dev.git_reading").to_string(),
                };
                return v_flex()
                    .size_full()
                    .child(section_head("Git".into(), t!("dev.git_hint").into(), cx))
                    .child(div().p(px(18.)).text_color(c.muted).child(text))
                    .into_any_element();
            }
        };
        let has_upstream = status.upstream.is_some();
        let branch = match (&status.branch, &status.upstream) {
            (Some(b), Some(u)) => format!("{b} → {u}  ↑{} ↓{}", status.ahead, status.behind),
            (Some(b), None) => format!("{b} · {}", t!("dev.git_no_upstream")),
            (None, _) => t!("dev.git_detached").to_string(),
        };
        let head = section_head("Git".into(), branch.into(), cx)
            .child(
                Button::new("git-refresh")
                    .ghost()
                    .icon(IconName::Refresh)
                    .tooltip(t!("dev.recheck"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_git(cx))),
            )
            .child(
                Button::new("git-pull")
                    .icon(IconName::ArrowDown)
                    .label(if status.behind > 0 {
                        t!("dev.git_pull_n", n = status.behind)
                    } else {
                        t!("dev.git_pull")
                    })
                    .disabled(busy || !installed || !has_upstream)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run_git(GitOp::Pull, |this, cx| this.reload_project(cx), cx)
                    })),
            )
            .child(
                Button::new("git-push")
                    .icon(IconName::ArrowUp)
                    .label(if status.ahead > 0 {
                        t!("dev.git_push_n", n = status.ahead)
                    } else {
                        t!("dev.git_push")
                    })
                    .disabled(busy || !installed || status.branch.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run_git(GitOp::Push { has_upstream }, |_, _| {}, cx)
                    })),
            );

        let more = |n: usize| {
            (n > SHOWN).then(|| {
                div()
                    .px(px(8.))
                    .text_color(c.muted)
                    .child(t!("dev.git_more", n = n - SHOWN).to_string())
            })
        };
        let diff: Vec<AnyElement> = self
            .git
            .diff
            .iter()
            .take(SHOWN)
            .map(|line| {
                let (mark, color, text) = match line {
                    DiffLine::Added(t) => ("+", c.ok, t.clone()),
                    DiffLine::Changed(t) => ("~", c.warn, t.clone()),
                    DiffLine::Removed(t) => ("-", crate::gui::theme::danger(), t.clone()),
                };
                h_flex()
                    .items_start()
                    .gap(px(8.))
                    .child(div().flex_none().text_color(color).child(mark))
                    .child(div().flex_1().min_w_0().child(text))
                    .into_any_element()
            })
            .collect();
        let files: Vec<AnyElement> = status
            .files
            .iter()
            .take(SHOWN)
            .enumerate()
            .map(|(i, f)| {
                let color = match f.state {
                    FileState::Untracked | FileState::Added => c.ok,
                    FileState::Removed | FileState::Conflict => crate::gui::theme::danger(),
                    _ => c.warn,
                };
                let path = f.path.clone();
                h_flex()
                    .id(("git-file", i))
                    .h(px(24.))
                    .px(px(8.))
                    .gap(px(10.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c.row))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_changed(&path, cx)))
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .font_family(mono.clone())
                            .text_color(color)
                            .child(state_mark(f.state)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(f.path.clone()))
                    .when(f.staged, |r| {
                        r.child(
                            div()
                                .text_size(px(11.))
                                .text_color(c.muted)
                                .child(t!("dev.git_staged").to_string()),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        let log: Vec<AnyElement> = status
            .log
            .iter()
            .map(|commit| {
                let when = std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs(commit.time.max(0) as u64);
                h_flex()
                    .h(px(24.))
                    .gap(px(10.))
                    .px(px(8.))
                    .child(
                        div()
                            .w(px(60.))
                            .flex_none()
                            .font_family(mono.clone())
                            .text_color(c.muted)
                            .child(commit.id.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(commit.summary.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .child(format!(
                                "{} · {}",
                                commit.author,
                                crate::gui::time::ago(when)
                            )),
                    )
                    .into_any_element()
            })
            .collect();
        let clean = files.is_empty();
        let block = |title: String, body: gpui_kit::Div| {
            v_flex().gap(px(6.)).child(caption(title, cx)).child(body)
        };
        let commit_box = v_flex()
            .gap(px(8.))
            .child(TextArea::new(&self.git.message))
            .child(
                h_flex()
                    .gap(px(8.))
                    .when(!installed, |row| {
                        row.child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .text_color(c.warn)
                                .child(t!("dev.git_missing").to_string()),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("git-commit")
                            .primary()
                            .icon(IconName::Check)
                            .label(t!("dev.git_commit_n", n = status.files.len()))
                            .disabled(busy || clean || !installed)
                            .on_click(cx.listener(|this, _, _, cx| this.commit(cx))),
                    ),
            );
        v_flex()
            .size_full()
            .child(head)
            .child(
                v_flex()
                    .id("dev-git")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .p(px(18.))
                            .gap(px(16.))
                            .max_w(px(820.))
                            .child(block(
                                t!("dev.git_pack_changes").to_string(),
                                v_flex()
                                    .gap(px(3.))
                                    .p(px(10.))
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(c.border)
                                    .bg(c.panel)
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .text_color(c.text2)
                                    .map(|col| {
                                        if diff.is_empty() {
                                            col.child(
                                                div()
                                                    .text_color(c.muted)
                                                    .child(t!("dev.git_pack_same").to_string()),
                                            )
                                        } else {
                                            col.children(diff).children(more(self.git.diff.len()))
                                        }
                                    }),
                            ))
                            .child(block(
                                t!("dev.git_files", n = status.files.len()).to_string(),
                                v_flex().gap(px(1.)).map(|col| {
                                    if clean {
                                        col.child(
                                            div()
                                                .px(px(8.))
                                                .text_color(c.muted)
                                                .child(t!("dev.git_clean").to_string()),
                                        )
                                    } else {
                                        col.children(files).children(more(status.files.len()))
                                    }
                                }),
                            ))
                            .child(block(t!("dev.git_commit").to_string(), commit_box))
                            .child(block(
                                t!("dev.git_log").to_string(),
                                v_flex().gap(px(1.)).children(log),
                            )),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn git_clear_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("git-clear")
            .ghost()
            .size(ButtonSize::Xs)
            .icon(IconName::Trash)
            .tooltip(t!("dev.clear"))
            .disabled(self.git.output.is_empty())
            .on_click(cx.listener(|this, _, _, cx| {
                this.git.output.clear();
                cx.notify();
            }))
    }
}
