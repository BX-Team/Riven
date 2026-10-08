use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{Context, Window};
use notify::{RecursiveMode, Watcher as _};
use riven_build::workspace::{OVERRIDES, PROJECT_FILE, Workspace};
use tokio::sync::mpsc;

use super::{DevView, Tab, git};

/// Changes arriving within this window are handled together.
const SETTLE: Duration = Duration::from_millis(150);

/// Watches `riven.json` and `overrides/` of the open project.
pub(super) struct Watch {
    watcher: notify::RecommendedWatcher,
    dir: PathBuf,
    overrides: bool,
}

impl Watch {
    /// Watches `overrides/` once it exists; until then the project folder reports its creation.
    fn cover_overrides(&mut self) {
        let overrides = self.dir.join(OVERRIDES);
        if !self.overrides && overrides.is_dir() {
            self.overrides = self
                .watcher
                .watch(&overrides, RecursiveMode::Recursive)
                .is_ok();
        }
    }
}

fn relevant(dir: &Path, path: &Path) -> bool {
    let temporary = path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().ends_with(".tmp"));
    !temporary && (path == dir.join(PROJECT_FILE) || path.starts_with(dir.join(OVERRIDES)))
}

impl DevView {
    /// Starts watching the open project, replacing the previous watch.
    pub(super) fn watch(&mut self, cx: &mut Context<Self>) {
        self.watch = None;
        let Some(dir) = self.project.as_ref().map(|ws| ws.dir.clone()) else {
            return;
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            // Reads report events too, and the project is read on every change: ignore them.
            if let Ok(event) = event
                && !matches!(event.kind, notify::EventKind::Access(_))
            {
                for path in event.paths {
                    let _ = tx.send(path);
                }
            }
        });
        let Ok(mut watcher) = watcher else {
            return;
        };
        if watcher.watch(&dir, RecursiveMode::NonRecursive).is_err() {
            return;
        }
        let mut watch = Watch {
            watcher,
            dir: dir.clone(),
            overrides: false,
        };
        watch.cover_overrides();
        self.watch = Some(watch);
        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.recv().await {
                cx.background_executor().timer(SETTLE).await;
                let mut paths = vec![first];
                while let Ok(more) = rx.try_recv() {
                    paths.push(more);
                }
                if !paths.iter().any(|p| relevant(&dir, p)) {
                    continue;
                }
                let project = paths.iter().any(|p| *p == dir.join(PROJECT_FILE));
                let alive = this.update(cx, |this, cx| this.on_disk_change(project, cx));
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Something under the project changed outside the editor: read what is affected again.
    fn on_disk_change(&mut self, project: bool, cx: &mut Context<Self>) {
        if let Some(watch) = &mut self.watch {
            watch.cover_overrides();
        }
        if project && let Some(ws) = &self.project {
            match Workspace::open(&ws.dir) {
                Ok(fresh) if fresh.project != ws.project => {
                    self.project = Some(fresh);
                    self.forget_project_file();
                    self.updates = super::Updates::Unchecked;
                    self.refresh_rows(cx);
                    self.run_check(cx);
                }
                _ => {}
            }
        }
        self.refresh_tree(cx);
        self.reload_files = true;
        if !matches!(self.git.repo, git::Repo::Loading) {
            self.refresh_git(cx);
        }
        cx.notify();
    }

    /// Takes changes on disk into open editors that have no unsaved edits.
    pub(super) fn reload_open_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.reload_files) {
            return;
        }
        let Some(ws) = self.project.clone() else {
            return;
        };
        let mut gone = Vec::new();
        for (path, file) in &mut self.files {
            if file.dirty || path.as_str() == PROJECT_FILE {
                continue;
            }
            match ws.read_override(path) {
                Ok(text) if text != file.saved => {
                    file.saved = text.clone();
                    file.editor
                        .update(cx, |s, cx| s.set_value(text, window, cx));
                }
                Ok(_) => {}
                Err(_) => gone.push(path.clone()),
            }
        }
        for path in gone {
            self.drop_tab(Tab::File(path), cx);
        }
    }
}
