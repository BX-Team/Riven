use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, SharedString};
use riven_format::Account;
use riven_launch::LaunchError;
use riven_launch::game::{self, Progress, Stage};
use riven_sync::SyncError;
use riven_sync::update::Request;
use tokio::sync::mpsc;

use super::runtime;
use super::state::AppState;
use super::toast::ToastKind;

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
    /// Why the pack could not be checked, shown under the status.
    pub notice: Option<SharedString>,
    /// Why the game most likely crashed, once it exited.
    pub crash: Option<riven_launch::crash::Diagnosis>,
    /// Stops the work while it has not started the game yet.
    abort: Option<runtime::Abort>,
    /// The instance's first pack install, which leaves nothing worth keeping when cancelled.
    fresh: bool,
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
            crash: None,
            abort: None,
            fresh: false,
        }
    }

    /// Whether the work can still be stopped: it has not begun starting the game.
    pub fn cancellable(&self) -> bool {
        matches!(&self.phase, Phase::Working { stage, .. } if *stage != Stage::Starting)
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
            Progress::Offline { .. } => self.notice = Some(rust_i18n::t!("launch.offline").into()),
            Progress::KeyChanged { .. } => {
                self.notice = Some(rust_i18n::t!("launch.key_changed").into())
            }
            Progress::Installed { .. } => self.fresh = false,
            Progress::Updated { .. } | Progress::Replaced { .. } => {}
            Progress::Started { pid, played } => {
                let now = Instant::now();
                self.phase = Phase::Running {
                    pid,
                    since: now.checked_sub(Duration::from_secs(played)).unwrap_or(now),
                }
            }
            Progress::Line { stderr, text } => {
                if self.log.len() == LOG_LINES {
                    self.log.pop_front();
                }
                self.log.push_back((stderr, text.into()));
            }
            Progress::Exited { code, crash, .. } => {
                self.phase = Phase::Finished { code };
                self.crash = crash;
            }
        }
    }
}

impl AppState {
    /// Starts `job` on tokio and folds its progress into the instance's session.
    fn run_session<F, Fut>(&mut self, id: &str, fresh: bool, job: F, cx: &mut Context<Self>)
    where
        F: FnOnce(mpsc::UnboundedSender<Progress>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), LaunchError>>,
    {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (done, abort) = runtime::pinned_abortable(move || job(tx));
        let mut session = Session::new();
        session.abort = Some(abort);
        session.fresh = fresh;
        self.sessions.insert(id.to_owned(), session);
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
                        let key_changed = batch.iter().find_map(|p| match p {
                            Progress::KeyChanged { url } => Some(url.clone()),
                            _ => None,
                        });
                        let mut news = Vec::new();
                        for p in batch {
                            news.extend(match &p {
                                Progress::Installed { version } => {
                                    Some(rust_i18n::t!("launch.installed", version = version))
                                }
                                Progress::Updated { version } => {
                                    Some(rust_i18n::t!("launch.updated", version = version))
                                }
                                Progress::Replaced { names } => {
                                    Some(rust_i18n::t!("launch.replaced", names = names.join(", ")))
                                }
                                _ => None,
                            });
                            session.apply(p);
                        }
                        let changed = !news.is_empty();
                        for text in news {
                            s.toast(ToastKind::Info, text.to_string(), cx);
                        }
                        if exited {
                            s.reload_instances(cx);
                        }
                        if changed {
                            *s.revisions.entry(id.clone()).or_default() += 1;
                        }
                        if let Some(url) = key_changed {
                            s.ask_new_key(&id, url, cx);
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let Ok(result) = done.await else {
                let _ = this.update(cx, |s, cx| s.cancelled(&id, cx));
                return;
            };
            let _ = this.update(cx, |s, cx| {
                if let Err(LaunchError::Sync(SyncError::KeyChanged(url))) = &result {
                    s.ask_new_key(&id, url.clone(), cx);
                }
                if let Some(session) = s.sessions.get_mut(&id) {
                    session.abort = None;
                    match result {
                        Err(e) => {
                            tracing::error!("{e}");
                            session.phase = Phase::Failed(e.to_string().into());
                        }
                        Ok(()) if session.busy() => session.phase = Phase::Finished { code: None },
                        Ok(()) => {}
                    }
                }
                *s.revisions.entry(id.clone()).or_default() += 1;
                s.reload_instances(cx);
                s.check_modpack(&id, cx);
            });
        })
        .detach();
        self.tick(cx);
        cx.notify();
    }

    /// Asks whether the pack's new signing key is the author's; on yes forgets the old one and updates.
    fn ask_new_key(&mut self, id: &str, url: String, cx: &mut Context<Self>) {
        let name = self
            .instance(id)
            .map_or_else(|| id.to_owned(), |i| i.name.clone());
        let id = id.to_owned();
        super::dialogs::confirm(
            rust_i18n::t!("trust.changed_title"),
            rust_i18n::t!("trust.changed_body", name = name),
            rust_i18n::t!("trust.accept"),
            move |_, cx| {
                let forgotten = riven_sync::trust::forget(&url);
                AppState::global(cx).update(cx, |s, cx| match forgotten {
                    Err(e) => s.toast(ToastKind::Error, e.to_string(), cx),
                    Ok(_) if s.sessions.get(&id).is_some_and(Session::busy) => {
                        s.toast(ToastKind::Info, rust_i18n::t!("trust.next_launch"), cx)
                    }
                    Ok(_) => s.install_pack(&id, Request::default(), cx),
                });
            },
            cx,
        );
    }

    /// Stops an install or a launch that has not started the game yet.
    pub fn cancel(&mut self, id: &str) {
        if let Some(session) = self.sessions.get(id).filter(|s| s.cancellable())
            && let Some(abort) = &session.abort
        {
            abort.abort();
        }
    }

    fn cancelled(&mut self, id: &str, cx: &mut Context<Self>) {
        let fresh = self.sessions.get(id).is_some_and(|s| s.fresh);
        if let Some(session) = self.sessions.get_mut(id) {
            session.abort = None;
            session.phase = Phase::Finished { code: None };
            session.notice = Some(rust_i18n::t!("launch.cancelled").into());
        }
        if fresh {
            self.delete_instance(id, cx);
            return;
        }
        *self.revisions.entry(id.to_owned()).or_default() += 1;
        self.reload_instances(cx);
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
            false,
            move |tx| async move {
                game::play(&store, &instance, &account, &defaults, move |p| {
                    let _ = tx.send(p);
                })
                .await
            },
            cx,
        );
    }

    /// Picks up the games an earlier launcher started that still run.
    pub fn adopt_running(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let ids: Vec<String> = self.instances.iter().map(|(id, _)| id.clone()).collect();
        for id in ids {
            let Some(running) = game::running(&store, &id) else {
                continue;
            };
            let store = store.clone();
            let defaults = self.settings.launch.clone();
            let instance = id.clone();
            self.run_session(
                &id,
                false,
                move |tx| async move {
                    game::watch(&store, &instance, running, &defaults, move |p| {
                        let _ = tx.send(p);
                    })
                    .await
                },
                cx,
            );
        }
        self.reload_instances(cx);
    }

    /// Installs a pack into a just-created instance.
    pub fn install_pack(&mut self, id: &str, request: Request, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let fresh = !riven_launch::own::from_pack(&store.game_dir(id));
        let instance = id.to_owned();
        self.run_session(
            id,
            fresh,
            move |tx| async move {
                game::install_pack(&store, &instance, request, move |p| {
                    let _ = tx.send(p);
                })
                .await
            },
            cx,
        );
    }

    /// Installs a pack release into an instance, then starts it: the developer's test run.
    pub fn install_and_play(
        &mut self,
        id: &str,
        request: Request,
        account: Account,
        cx: &mut Context<Self>,
    ) {
        if self.sessions.get(id).is_some_and(Session::busy) {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let defaults = self.settings.launch.clone();
        let fresh = !riven_launch::own::from_pack(&store.game_dir(id));
        let instance = id.to_owned();
        self.run_session(
            id,
            fresh,
            move |tx| async move {
                let report = tx.clone();
                game::install_pack(&store, &instance, request, move |p| {
                    let _ = report.send(p);
                })
                .await?;
                game::play(&store, &instance, &account, &defaults, move |p| {
                    let _ = tx.send(p);
                })
                .await
            },
            cx,
        );
    }

    /// Looks for newer Modrinth versions of every instance's modpack.
    pub fn check_modpacks(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.instances.iter().map(|(id, _)| id.clone()).collect();
        for id in ids {
            self.check_modpack(&id, cx);
        }
    }

    /// Looks for a newer Modrinth version of the modpack an instance was installed from.
    pub fn check_modpack(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(instance) = self
            .instance(id)
            .filter(|i| riven_launch::modpack::follows(i))
            .cloned()
        else {
            self.modpack_updates.remove(id);
            return;
        };
        let found = runtime::spawn(async move { riven_launch::modpack::newer(&instance).await });
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let found = match found.await {
                Ok(Ok(found)) => found,
                Ok(Err(e)) => {
                    tracing::warn!("cannot check {id} for updates: {e}");
                    return;
                }
                Err(_) => return,
            };
            let _ = this.update(cx, |s, cx| {
                match found {
                    Some(version) => s.modpack_updates.insert(id, version),
                    None => s.modpack_updates.remove(&id),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Installs the newer Modrinth version found for an instance's modpack.
    pub fn update_modpack(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.sessions.get(id).is_some_and(Session::busy) {
            return;
        }
        let (Some(store), Some(version)) = (self.store.clone(), self.modpack_updates.remove(id))
        else {
            return;
        };
        let instance = id.to_owned();
        self.run_session(
            id,
            false,
            move |tx| async move {
                riven_launch::modpack::update(&store, &instance, version, move |p| {
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
        let pid = *pid;
        cx.background_spawn(async move {
            if let Err(e) = game::stop(pid) {
                tracing::warn!("{e}");
            }
        })
        .detach();
    }
}
