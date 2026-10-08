use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px, relative,
};
use riven_launch::update::{self, Edition, Install, Release};
use rust_i18n::t;
use tokio::sync::mpsc;

use super::dialogs;
use super::launch_bar::progress_bar;
use super::markdown::{self, Block};
use super::runtime;
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::toast::ToastKind;
use super::ui::{Button, ButtonSize, IconName, v_flex};

/// Where the search for a new Riven stands.
#[derive(Debug, Clone, Default)]
pub enum Update {
    #[default]
    Idle,
    Checking,
    Available(Release),
    Downloading {
        release: Release,
        done: u64,
        total: u64,
    },
    Failed {
        release: Release,
        error: SharedString,
    },
}

impl Update {
    fn release(&self) -> Option<&Release> {
        match self {
            Update::Idle | Update::Checking => None,
            Update::Available(release)
            | Update::Downloading { release, .. }
            | Update::Failed { release, .. } => Some(release),
        }
    }
}

impl AppState {
    /// Asks GitHub for a newer release; `manual` reports "up to date" and errors too.
    pub fn check_update(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(self.update, Update::Checking | Update::Downloading { .. }) {
            return;
        }
        let before = std::mem::replace(&mut self.update, Update::Checking);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = runtime::spawn(update::check())
                .await
                .unwrap_or_else(|_| Err(riven_launch::LaunchError::Update("cancelled".into())));
            let _ = this.update(cx, |s, cx| {
                s.update = match found {
                    Ok(Some(release)) => Update::Available(release),
                    Ok(None) => {
                        if manual {
                            s.toast(
                                ToastKind::Success,
                                t!("update.latest", version = update::CURRENT),
                                cx,
                            );
                        }
                        Update::Idle
                    }
                    Err(e) => {
                        tracing::warn!("update check failed: {e}");
                        if manual {
                            s.toast(ToastKind::Error, t!("update.check_failed", error = e), cx);
                        }
                        before
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Downloads the found release, puts it in place and restarts into it.
    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let release = match &self.update {
            Update::Available(release) | Update::Failed { release, .. } => release.clone(),
            _ => return,
        };
        self.update = Update::Downloading {
            release: release.clone(),
            done: 0,
            total: 0,
        };
        cx.notify();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let job = runtime::spawn({
            let release = release.clone();
            async move {
                let progress = |done, total| {
                    let _ = tx.send((done, total));
                };
                update::install(&release, &Install::detect(), Edition::Launcher, progress).await
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some((done, total)) = rx.recv().await {
                let _ = this.update(cx, |s, cx| {
                    if let Update::Downloading {
                        done: d, total: t, ..
                    } = &mut s.update
                    {
                        (*d, *t) = (done, total);
                        cx.notify();
                    }
                });
            }
            let result = job
                .await
                .unwrap_or_else(|_| Err(riven_launch::LaunchError::Update("cancelled".into())));
            let error = match result.map(|relaunch| relaunch.spawn()) {
                Ok(Ok(())) => {
                    cx.update(|cx| cx.quit());
                    return;
                }
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            tracing::error!("update failed: {error}");
            let _ = this.update(cx, |s, cx| {
                s.update = Update::Failed {
                    release,
                    error: error.into(),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

/// Looks for an update at startup unless the user turned that off.
pub fn check_at_startup(cx: &mut App) {
    let state = AppState::global(cx);
    if cfg!(debug_assertions) || state.read(cx).settings.manual_updates {
        return;
    }
    state.update(cx, |s, cx| s.check_update(false, cx));
}

/// The title bar button that appears once a new version is found.
pub fn title_button(cx: &App) -> Option<AnyElement> {
    let update = &AppState::global(cx).read(cx).update;
    let release = update.release()?;
    let label = match update {
        Update::Downloading { done, total, .. } if *total > 0 => {
            t!("update.percent", percent = done * 100 / total).to_string()
        }
        _ => t!("update.button").to_string(),
    };
    Some(
        Button::new("update")
            .primary()
            .icon(IconName::ArrowDown)
            .label(label)
            .tooltip(t!("update.available_title", version = release.version))
            .on_click(|_, _, cx| open(cx))
            .into_any_element(),
    )
}

pub fn open(cx: &mut App) {
    let view = cx.new(UpdateDialog::new);
    dialogs::open(view.into(), 520., cx);
}

/// The new version's notes and the button that installs it.
pub struct UpdateDialog {
    install: Install,
    notes: Vec<Block>,
    _state: Subscription,
}

impl UpdateDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        let notes = state
            .read(cx)
            .update
            .release()
            .map(|r| markdown::blocks(&r.notes))
            .unwrap_or_default();
        Self {
            install: Install::detect(),
            notes,
            _state: cx.observe(&state, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for UpdateDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let state = AppState::global(cx);
        let st = state.read(cx);
        let update = st.update.clone();
        let playing = st.sessions.values().any(|s| s.busy());
        let Some(release) = update.release().cloned() else {
            return dialogs::dialog_shell(
                t!("update.check"),
                t!("update.latest", version = update::CURRENT),
                div(),
                vec![close_button(t!("common.close"))],
                cx,
            );
        };
        let page = release.page.clone();
        let notes = if self.notes.is_empty() {
            vec![
                div()
                    .text_color(c.muted)
                    .child(t!("update.no_notes").to_string())
                    .into_any_element(),
            ]
        } else {
            markdown::view(&self.notes, cx)
        };
        let status = match &update {
            Update::Downloading { done, total, .. } => Some(
                v_flex()
                    .gap(px(6.))
                    .child(progress_bar(
                        "update",
                        (*total > 0).then(|| *done as f32 / *total as f32),
                        window,
                        cx,
                    ))
                    .child(
                        div().text_color(c.muted).text_size(px(12.)).child(
                            t!(
                                "update.downloading",
                                done = format!("{:.1}", *done as f64 / 1e6),
                                total = format!("{:.1}", *total as f64 / 1e6)
                            )
                            .to_string(),
                        ),
                    )
                    .into_any_element(),
            ),
            Update::Failed { error, .. } => Some(
                div()
                    .text_color(super::theme::danger())
                    .line_height(relative(1.5))
                    .child(t!("update.failed", error = error).to_string())
                    .into_any_element(),
            ),
            _ => None,
        };
        let blocker = if !self.install.self_updates() {
            Some(t!("update.managed"))
        } else if playing {
            Some(t!("update.game_running"))
        } else {
            None
        };
        let body = v_flex()
            .gap(px(6.))
            .gap(px(14.))
            .child(
                div()
                    .id("update-notes")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(notes),
            )
            .children(status)
            .when_some(blocker.clone(), |col, text| {
                col.child(div().text_color(c.warn).child(text.to_string()))
            });
        let downloading = matches!(update, Update::Downloading { .. });
        let mut buttons = vec![
            Button::new("release-page")
                .outline()
                .size(ButtonSize::Md)
                .icon(IconName::ExternalLink)
                .label(t!("update.release_page"))
                .on_click(move |_, _, cx| cx.open_url(&page)),
            close_button(t!("update.later")),
        ];
        if self.install.self_updates() {
            buttons.push(
                Button::new("install-update")
                    .primary()
                    .size(ButtonSize::Md)
                    .label(if downloading {
                        t!("update.installing")
                    } else {
                        t!("update.install")
                    })
                    .disabled(downloading || blocker.is_some())
                    .on_click(|_, _, cx| {
                        AppState::global(cx).update(cx, |s, cx| s.install_update(cx))
                    }),
            );
        }
        dialogs::dialog_shell(
            t!("update.available_title", version = release.version),
            t!("update.current", version = update::CURRENT),
            body,
            buttons,
            cx,
        )
    }
}

fn close_button(label: impl Into<SharedString>) -> Button {
    Button::new("close-update")
        .outline()
        .size(ButtonSize::Md)
        .label(label)
        .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx)))
}

/// Shows the dialog for a known update, else looks for one.
pub fn check_or_open(cx: &mut App) {
    let state = AppState::global(cx);
    if state.read(cx).update.release().is_some() {
        open(cx);
    } else {
        state.update(cx, |s, cx| s.check_update(true, cx));
    }
}
