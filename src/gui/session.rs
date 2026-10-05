use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, SharedString};
use riven_format::Account;
use riven_launch::LaunchError;
use riven_launch::game::{self, Progress, Stage};
use riven_sync::update::Request;
use tokio::sync::mpsc;

use super::runtime;
use super::state::AppState;

/// How many game log lines an instance keeps in memory.
const LOG_LINES: usize = 20_000;

#[derive(Debug, Clone)]
pub enum Phase {
    Working { stage: Stage, done: u64, total: u64 },
    Running { pid: u32, since: Instant },
    Finished { code: Option<i32> },
    Failed(SharedString),
}

/// One launch or pack install of an instance and what it printed.
pub struct Session {
    pub phase: Phase,
    pub log: VecDeque<(bool, SharedString)>,
    /// "Pack updated to 1.4" or "Offline: …", shown next to the status.
    pub notice: Option<SharedString>,
}

impl Session {
    fn new() -> Self {
        Self {
            phase: Phase::Working {
                stage: Stage::Pack,
                done: 0,
                total: 0,
            },
            log: VecDeque::new(),
            notice: None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Working { .. } | Phase::Running { .. })
    }

    fn apply(&mut self, progress: Progress) {
        match progress {
            Progress::Stage(stage) => {
                self.phase = Phase::Working {
                    stage,
                    done: 0,
                    total: 0,
                }
            }
            Progress::Amount { done, total } => {
                if let Phase::Working {
                    done: d, total: t, ..
                } = &mut self.phase
                {
                    *d = done;
                    *t = total;
                }
            }
            Progress::Updated { version } => {
                self.notice = Some(rust_i18n::t!("launch.updated", version = version).into())
            }
            Progress::Offline { .. } => self.notice = Some(rust_i18n::t!("launch.offline").into()),
            Progress::Started { pid } => {
                self.phase = Phase::Running {
                    pid,
                    since: Instant::now(),
                }
            }
            Progress::Line { stderr, text } => {
                if self.log.len() == LOG_LINES {
                    self.log.pop_front();
                }
                self.log.push_back((stderr, text.into()));
            }
            Progress::Exited { code, .. } => self.phase = Phase::Finished { code },
        }
    }
}

impl AppState {
    /// Starts `job` on tokio and folds its progress into the instance's session.
    fn run_session<F, Fut>(&mut self, id: &str, job: F, cx: &mut Context<Self>)
    where
        F: FnOnce(mpsc::UnboundedSender<Progress>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), LaunchError>>,
    {
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.sessions.insert(id.to_owned(), Session::new());
        let done = runtime::pinned(move || job(tx));
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let alive = this.update(cx, |s, cx| {
                    if let Some(session) = s.sessions.get_mut(&id) {
                        let exited = batch.iter().any(|p| matches!(p, Progress::Exited { .. }));
                        for p in batch {
                            session.apply(p);
                        }
                        if exited {
                            s.reload_instances(cx);
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = done.await.unwrap_or(Ok(()));
            let _ = this.update(cx, |s, cx| {
                if let Some(session) = s.sessions.get_mut(&id) {
                    match result {
                        Err(e) => {
                            tracing::error!("{e}");
                            session.phase = Phase::Failed(e.to_string().into());
                        }
                        Ok(()) if session.busy() => session.phase = Phase::Finished { code: None },
                        Ok(()) => {}
                    }
                }
                s.reload_instances(cx);
            });
        })
        .detach();
        self.tick(cx);
        cx.notify();
    }

    /// Repaints once a second while a game runs, for the play time in the launch bar.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let more = this.update(cx, |s, cx| {
                    let busy = s.sessions.values().any(Session::busy);
                    if busy {
                        cx.notify();
                    } else {
                        s.ticking = false;
                    }
                    busy
                });
                if !matches!(more, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }

    pub fn play(&mut self, id: &str, account: Account, cx: &mut Context<Self>) {
        if self.sessions.get(id).is_some_and(Session::busy) {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let defaults = self.settings.launch.clone();
        let instance = id.to_owned();
        self.run_session(
            id,
            move |tx| async move {
                game::play(&store, &instance, &account, &defaults, move |p| {
                    let _ = tx.send(p);
                })
                .await
            },
            cx,
        );
    }

    /// Installs a pack into a just-created instance.
    pub fn install_pack(&mut self, id: &str, request: Request, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let instance = id.to_owned();
        self.run_session(
            id,
            move |tx| async move {
                game::install_pack(&store, &instance, request, move |p| {
                    let _ = tx.send(p);
                })
                .await
            },
            cx,
        );
    }

    pub fn stop(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(Phase::Running { pid, .. }) = self.sessions.get(id).map(|s| &s.phase) else {
            return;
        };
        let (pid, id) = (*pid, id.to_owned());
        cx.background_spawn(async move {
            if let Ok(Err(e)) =
                runtime::pinned(move || async move { game::stop(&id, pid).await }).await
            {
                tracing::warn!("{e}");
            }
        })
        .detach();
    }
}
