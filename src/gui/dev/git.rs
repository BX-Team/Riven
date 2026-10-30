use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::base::input::TextareaState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window,
    div, px, uniform_list,
};
use riven_build::git::{self, DiffLine, DiffRow, FileState, GitError, Status};
use riven_format::PackPath;
use rust_i18n::t;
use tokio::sync::mpsc;

use super::{DevView, Panel, Tab};
use crate::gui::runtime;
use moonglass::theme::ActiveTheme as _;
use moonglass::{Button, ButtonSize, IconName, TextArea, h_flex, scrollbar, v_flex};

/// Lines of git output the panel keeps.
const OUTPUT_LINES: usize = 500;
const DIFF_ROW: f32 = 22.;
const FILE_ROW: f32 = 26.;

pub(super) enum Repo {
    Unknown,
    Loading,
    /// The project folder is not inside a git repository.
    None,
    Ready(Box<Status>),
    Failed(SharedString),
}

pub(super) enum FileDiff {
    Loading,
    Ready(Rc<Vec<DiffRow>>),
    /// One side is not text.
    Binary,
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
    diff_list: UniformListScrollHandle,
    /// Changed files read against `HEAD`, by path.
    files: HashMap<String, FileDiff>,
    file_diff: UniformListScrollHandle,
    files_list: UniformListScrollHandle,
    log_list: UniformListScrollHandle,
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
        cx.spawn(async move |this, cx| {
            let installed = runtime::blocking(git::available).await.unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                this.git.installed = installed;
                cx.notify();
            });
        })
        .detach();
        Self {
            repo: Repo::Unknown,
            diff: Vec::new(),
            reading: false,
            again: false,
            diff_list: UniformListScrollHandle::new(),
            files: HashMap::new(),
            file_diff: UniformListScrollHandle::new(),
            files_list: UniformListScrollHandle::new(),
            log_list: UniformListScrollHandle::new(),
            installed: true,
            message: moonglass::textarea(String::new(), (3, 8), window, cx),
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
    Fetch,
    Switch { branch: String, create: bool },
    Clone { url: String, dest: PathBuf },
}

impl GitOp {
    fn label(&self) -> SharedString {
        match self {
            GitOp::Commit(_) => t!("dev.git_committing"),
            GitOp::Pull => t!("dev.git_pulling"),
            GitOp::Push { .. } => t!("dev.git_pushing"),
            GitOp::Fetch => t!("dev.git_fetching"),
            GitOp::Switch { .. } => t!("dev.git_switching"),
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
            GitOp::Fetch => "$ git fetch --prune".into(),
            GitOp::Switch {
                branch,
                create: true,
            } => format!("$ git switch -c {branch}"),
            GitOp::Switch { branch, .. } => format!("$ git switch {branch}"),
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
                let open: Vec<String> = this
                    .tabs
                    .iter()
                    .filter_map(|t| match t {
                        Tab::Diff(path) => Some(path.clone()),
                        _ => None,
                    })
                    .collect();
                this.git.files.retain(|path, _| open.contains(path));
                for path in open {
                    this.load_diff(path, cx);
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
        self.panel = Panel::Output;
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
                GitOp::Fetch => git::fetch(dir.as_deref().expect("checked"), &line).await,
                GitOp::Switch { branch, create } => {
                    git::switch(dir.as_deref().expect("checked"), &branch, create, &line).await
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

    /// Another branch is checked out: its files replace the open ones.
    pub(super) fn after_checkout(&mut self, cx: &mut Context<Self>) {
        self.files.clear();
        self.tabs.retain(|t| !matches!(t, Tab::File(_)));
        if matches!(self.active, Tab::File(_)) {
            self.active = Tab::Pack;
        }
        self.pages.clear();
        self.reload_project(cx);
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
        self.load_diff(path.to_owned(), cx);
        self.show(Tab::Diff(path.to_owned()), cx);
    }

    /// The file as an editor tab, when it is one the developer section edits.
    fn editable(&self, path: &str) -> Option<PackPath> {
        let pack = PackPath::new(path).ok()?;
        let editable = path == riven_build::workspace::PROJECT_FILE
            || self
                .project
                .as_ref()
                .is_some_and(|ws| ws.override_path(&pack).is_ok_and(|p| p.is_file()));
        editable.then_some(pack)
    }

    fn load_diff(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(dir) = self.project.as_ref().map(|ws| ws.dir.clone()) else {
            return;
        };
        self.git
            .files
            .entry(path.clone())
            .or_insert(FileDiff::Loading);
        cx.spawn(async move |this, cx| {
            let read = path.clone();
            let result = runtime::blocking(move || git::file_diff(&dir, &read))
                .await
                .unwrap_or_else(|_| Err(GitError::Read(String::new())));
            let _ = this.update(cx, |this, cx| {
                let diff = match result {
                    Ok(Some(rows)) => FileDiff::Ready(Rc::new(rows)),
                    Ok(None) => FileDiff::Binary,
                    Err(e) => FileDiff::Failed(e.to_string().into()),
                };
                this.git.files.insert(path, diff);
                cx.notify();
            });
        })
        .detach();
    }

    /// A Diff tab: the file's changed lines against `HEAD`, with their numbers on both sides.
    pub(super) fn render_diff(&self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let open = self.editable(path);
        let head = h_flex()
            .flex_none()
            .gap(px(10.))
            .px(px(14.))
            .py(px(6.))
            .border_b_1()
            .border_color(c.row)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono.clone())
                    .text_size(px(12.))
                    .text_color(c.text2)
                    .child(format!("{path} · HEAD → {}", t!("dev.git_working"))),
            )
            .when_some(open, |row, pack| {
                row.child(
                    Button::new("diff-open-file")
                        .size(ButtonSize::Xs)
                        .icon(IconName::Pencil)
                        .label(t!("dev.open_file"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show(Tab::File(pack.clone()), cx)
                        })),
                )
            });
        let note = |text: SharedString| {
            div()
                .p(px(18.))
                .text_color(c.muted)
                .child(text)
                .into_any_element()
        };
        let body = match self.git.files.get(path) {
            None | Some(FileDiff::Loading) => note(t!("dev.git_reading").into()),
            Some(FileDiff::Binary) => note(t!("dev.git_binary").into()),
            Some(FileDiff::Failed(e)) => note(e.clone()),
            Some(FileDiff::Ready(rows)) if rows.is_empty() => note(t!("dev.git_no_changes").into()),
            Some(FileDiff::Ready(rows)) => {
                let rows = rows.clone();
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .font_family(mono)
                    .text_size(px(12.5))
                    .child(
                        uniform_list("file-diff", rows.len(), move |range, _, cx| {
                            range
                                .filter_map(|ix| Some(diff_row_view(rows.get(ix)?, cx)))
                                .collect::<Vec<_>>()
                        })
                        .size_full()
                        .track_scroll(&self.git.file_diff),
                    )
                    .child(scrollbar(&self.git.file_diff))
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .child(head)
            .child(body)
            .into_any_element()
    }

    /// The Git sidebar: branch sync, changed files and the commit box.
    pub(super) fn render_git_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let busy = self.git.running.is_some();
        let installed = self.git.installed;
        let head = self.side_head("Git".into(), cx).child(
            Button::new("git-refresh")
                .ghost()
                .size(ButtonSize::Xs)
                .icon(IconName::Refresh)
                .tooltip(t!("dev.recheck"))
                .on_click(cx.listener(|this, _, _, cx| this.refresh_git(cx))),
        );
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
                    .child(head)
                    .child(
                        div()
                            .px(px(14.))
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .child(text),
                    )
                    .into_any_element();
            }
        };
        let has_upstream = status.upstream.is_some();
        let branch = match (&status.branch, &status.upstream) {
            (Some(b), Some(u)) => format!("{b} → {u}"),
            (Some(b), None) => format!("{b} · {}", t!("dev.git_no_upstream")),
            (None, _) => t!("dev.git_detached").to_string(),
        };
        let sync = h_flex()
            .flex_none()
            .px(px(10.))
            .pb(px(8.))
            .gap(px(6.))
            .child(
                Button::new("git-fetch")
                    .ghost()
                    .size(ButtonSize::Xs)
                    .icon(IconName::Refresh)
                    .tooltip(t!("dev.git_fetch"))
                    .disabled(busy || !installed || !has_upstream)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.run_git(GitOp::Fetch, |_, _| {}, cx)),
                    ),
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
        let clean = status.files.is_empty();
        let commit_box = v_flex()
            .flex_none()
            .px(px(10.))
            .py(px(10.))
            .gap(px(8.))
            .border_t_1()
            .border_color(c.border)
            .child(TextArea::new(&self.git.message))
            .when(!installed, |col| {
                col.child(
                    div()
                        .text_size(px(12.))
                        .text_color(c.warn)
                        .child(t!("dev.git_missing").to_string()),
                )
            })
            .child(
                Button::new("git-commit")
                    .primary()
                    .w_full()
                    .icon(IconName::Check)
                    .label(t!("dev.git_commit_n", n = status.files.len()))
                    .disabled(busy || clean || !installed)
                    .on_click(cx.listener(|this, _, _, cx| this.commit(cx))),
            );
        let files = if clean {
            div()
                .px(px(14.))
                .py(px(6.))
                .text_size(px(12.))
                .text_color(c.muted)
                .child(t!("dev.git_clean").to_string())
                .into_any_element()
        } else {
            self.git_list(
                "git-files",
                status.files.len(),
                &self.git.files_list,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|ix| this.file_row(ix, cx))
                        .collect::<Vec<_>>()
                }),
            )
        };
        v_flex()
            .size_full()
            .child(head)
            .child(
                div()
                    .flex_none()
                    .px(px(14.))
                    .pb(px(6.))
                    .truncate()
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .child(branch),
            )
            .child(sync)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(26.))
                    .px(px(14.))
                    .border_t_1()
                    .border_color(c.border)
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .child(div().flex_1().child(t!("dev.git_changed").to_uppercase()))
                    .child(status.files.len().to_string()),
            )
            .child(div().relative().flex_1().min_h_0().child(files))
            .child(commit_box)
            .child(
                h_flex().flex_none().px(px(10.)).pb(px(10.)).child(
                    Button::new("git-history")
                        .ghost()
                        .w_full()
                        .icon(IconName::History)
                        .label(t!("dev.history"))
                        .on_click(cx.listener(|this, _, _, cx| this.show(Tab::History, cx))),
                ),
            )
            .into_any_element()
    }

    /// The History tab: what changed in the pack since `HEAD`, and the commits.
    pub(super) fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let Repo::Ready(status) = &self.git.repo else {
            let text = match &self.git.repo {
                Repo::None => t!("dev.git_none").to_string(),
                Repo::Failed(e) => e.to_string(),
                _ => t!("dev.git_reading").to_string(),
            };
            return div()
                .p(px(18.))
                .text_color(c.muted)
                .child(text)
                .into_any_element();
        };
        let empty = |text: String| {
            div()
                .p(px(12.))
                .text_color(c.muted)
                .child(text)
                .into_any_element()
        };
        let pack = if self.git.diff.is_empty() {
            empty(t!("dev.git_pack_same").to_string())
        } else {
            self.git_list(
                "git-diff",
                self.git.diff.len(),
                &self.git.diff_list,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|ix| this.diff_row(ix, cx))
                        .collect::<Vec<_>>()
                }),
            )
        };
        let log = self.git_list(
            "git-log",
            status.log.len(),
            &self.git.log_list,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|ix| this.log_row(ix, cx))
                    .collect::<Vec<_>>()
            }),
        );
        let card = |title: String, count: Option<usize>, body: AnyElement| {
            v_flex()
                .min_h_0()
                .rounded(px(8.))
                .border_1()
                .border_color(c.border)
                .bg(c.panel)
                .overflow_hidden()
                .child(
                    h_flex()
                        .flex_none()
                        .h(px(34.))
                        .px(px(12.))
                        .gap(px(8.))
                        .border_b_1()
                        .border_color(c.border)
                        .font_family(mono.clone())
                        .text_size(px(11.))
                        .text_color(c.muted)
                        .child(title.to_uppercase())
                        .children(count.map(|n| div().text_color(c.text2).child(n.to_string()))),
                )
                .child(div().relative().flex_1().min_h_0().child(body))
        };
        h_flex()
            .size_full()
            .items_start()
            .gap(px(14.))
            .p(px(18.))
            .child(
                card(
                    t!("dev.git_pack_changes").to_string(),
                    (!self.git.diff.is_empty()).then_some(self.git.diff.len()),
                    pack,
                )
                .flex_1()
                .h_full(),
            )
            .child(
                card(t!("dev.git_log").to_string(), None, log)
                    .flex_1()
                    .h_full(),
            )
            .into_any_element()
    }

    /// A virtual list filling its card, with its own scrollbar.
    fn git_list<R: IntoElement>(
        &self,
        id: &'static str,
        len: usize,
        handle: &UniformListScrollHandle,
        rows: impl Fn(std::ops::Range<usize>, &mut Window, &mut gpui_kit::App) -> Vec<R> + 'static,
    ) -> AnyElement {
        div()
            .absolute()
            .inset_0()
            .child(
                uniform_list(id, len, rows)
                    .size_full()
                    .py(px(4.))
                    .track_scroll(handle),
            )
            .child(scrollbar(handle))
            .into_any_element()
    }

    fn log_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Repo::Ready(status) = &self.git.repo else {
            return None;
        };
        let commit = status.log.get(ix)?;
        let c = cx.theme().colors;
        let when =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(commit.time.max(0) as u64);
        Some(
            h_flex()
                .w_full()
                .h(px(FILE_ROW))
                .gap(px(10.))
                .px(px(12.))
                .child(
                    div()
                        .w(px(60.))
                        .flex_none()
                        .font_family(cx.theme().mono.clone())
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
                .into_any_element(),
        )
    }

    fn diff_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors;
        let (mark, color, text) = match self.git.diff.get(ix)? {
            DiffLine::Added(t) => ("+", c.ok, t.clone()),
            DiffLine::Changed(t) => ("~", c.warn, t.clone()),
            DiffLine::Removed(t) => ("-", moonglass::theme::danger(), t.clone()),
        };
        Some(
            h_flex()
                .w_full()
                .h(px(DIFF_ROW))
                .px(px(12.))
                .gap(px(8.))
                .font_family(cx.theme().mono.clone())
                .text_size(px(12.))
                .text_color(c.text2)
                .child(div().flex_none().text_color(color).child(mark))
                .child(div().flex_1().min_w_0().truncate().child(text))
                .into_any_element(),
        )
    }

    fn file_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Repo::Ready(status) = &self.git.repo else {
            return None;
        };
        let f = status.files.get(ix)?;
        let c = cx.theme().colors;
        let color = match f.state {
            FileState::Untracked | FileState::Added => c.ok,
            FileState::Removed | FileState::Conflict => moonglass::theme::danger(),
            _ => c.warn,
        };
        let (folder, name) = f.path.rsplit_once('/').unwrap_or(("", &f.path));
        let path = f.path.clone();
        Some(
            h_flex()
                .id(("git-file", ix))
                .w_full()
                .h(px(FILE_ROW))
                .px(px(12.))
                .gap(px(10.))
                .cursor_pointer()
                .hover(|s| s.bg(c.row))
                .on_click(cx.listener(move |this, _, _, cx| this.open_changed(&path, cx)))
                .child(
                    div()
                        .w(px(14.))
                        .flex_none()
                        .font_family(cx.theme().mono.clone())
                        .text_color(color)
                        .child(state_mark(f.state)),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.))
                        .child(div().flex_none().child(name.to_owned()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(c.muted)
                                .child(folder.to_owned()),
                        ),
                )
                .when(f.staged, |r| {
                    r.child(
                        div()
                            .text_size(px(11.))
                            .text_color(c.muted)
                            .child(t!("dev.git_staged").to_string()),
                    )
                })
                .into_any_element(),
        )
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

fn diff_row_view(row: &DiffRow, cx: &gpui_kit::App) -> AnyElement {
    let c = cx.theme().colors;
    let DiffRow::Line { old, new, text } = row else {
        return h_flex()
            .w_full()
            .h(px(DIFF_ROW))
            .px(px(12.))
            .bg(c.row)
            .text_color(c.muted)
            .child("⋯")
            .into_any_element();
    };
    let number = |n: &Option<usize>| {
        div()
            .w(px(44.))
            .flex_none()
            .pr(px(8.))
            .flex()
            .justify_end()
            .text_color(c.muted)
            .child(n.map(|n| n.to_string()).unwrap_or_default())
    };
    let (sign, tint) = if row.added() {
        ("+", Some(c.ok))
    } else if row.removed() {
        ("-", Some(moonglass::theme::danger()))
    } else {
        (" ", None)
    };
    h_flex()
        .w_full()
        .h(px(DIFF_ROW))
        .when_some(tint, |r, t| r.bg(t.opacity(0.12)))
        .child(number(old))
        .child(number(new))
        .child(
            div()
                .w(px(16.))
                .flex_none()
                .text_color(tint.unwrap_or(c.muted))
                .child(sign),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(text.replace('\t', "    ")),
        )
        .into_any_element()
}
