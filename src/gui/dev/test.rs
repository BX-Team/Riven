use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement as _, Styled as _, px};
use riven_build::ship;
use riven_format::InstallSide;
use riven_sync::update::Request;
use rust_i18n::t;

use super::{DevView, Panel, dialogs};
use crate::gui::state::{AppState, Route};
use crate::gui::ui::{Button, IconName, h_flex};

/// The test instance id; ids made from names never contain `_`, so it cannot clash with one.
pub(super) fn test_id(pack: &str) -> String {
    format!("_dev-{pack}")
}

impl DevView {
    pub(super) fn test_instance(&self) -> Option<String> {
        self.project.as_ref().map(|ws| test_id(&ws.project.id))
    }

    /// Builds the project into a local archive, installs it into the test instance and starts it.
    fn run_test(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.project.clone() else {
            return;
        };
        let state = AppState::global(cx);
        let (store, account) = {
            let s = state.read(cx);
            (s.store.clone(), crate::gui::launch_bar::selected_account(s))
        };
        let Some(store) = store else {
            return;
        };
        let Some(account) = account else {
            self.error = Some(t!("dev.test_no_account").into());
            cx.notify();
            return;
        };
        let id = test_id(&ws.project.id);
        let name = t!("dev.test_name", name = ws.project.name).to_string();
        self.panel = Panel::Log;
        let job_id = id.clone();
        self.job(
            t!("dev.test_preparing").into(),
            async move {
                let archive = ship::test_archive(&ws).map_err(|e| e.to_string())?;
                store
                    .ensure(
                        &job_id,
                        &name,
                        &ws.project.minecraft,
                        Some(ws.project.loader.clone()),
                    )
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(archive)
            },
            move |_, archive, cx| {
                let request = Request {
                    source: Some(archive.display().to_string()),
                    side: Some(InstallSide::Client),
                    ..Request::default()
                };
                state.update(cx, |s, cx| {
                    s.reload_instances(cx);
                    s.install_and_play(&id, request, account, cx);
                });
            },
            cx,
        );
    }

    fn stop_test(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.test_instance() {
            AppState::global(cx).update(cx, |s, cx| s.stop(&id, cx));
        }
    }

    /// The top bar's test controls: run or stop, and taking configs back.
    pub(super) fn render_test_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = AppState::global(cx);
        let (busy, exists) = {
            let s = state.read(cx);
            let busy = self
                .test_instance()
                .and_then(|id| s.sessions.get(&id))
                .is_some_and(crate::gui::session::Session::busy);
            let exists = self
                .test_instance()
                .is_some_and(|id| s.instance(&id).is_some());
            (busy, exists)
        };
        let view = cx.entity().downgrade();
        h_flex()
            .gap(px(6.))
            .when(exists, |row| {
                row.child(
                    Button::new("dev-pull-configs")
                        .ghost()
                        .icon(IconName::ArrowDown)
                        .label(t!("dev.pull"))
                        .tooltip(t!("dev.pull_hint"))
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            dialogs::open_pull(view.clone(), window, cx)
                        }),
                )
            })
            .child(if busy {
                Button::new("dev-test-stop")
                    .icon(IconName::Stop)
                    .label(t!("dev.test_stop"))
                    .on_click(cx.listener(|this, _, _, cx| this.stop_test(cx)))
            } else {
                Button::new("dev-test")
                    .primary()
                    .icon(IconName::Play)
                    .label(t!("dev.test"))
                    .tooltip(t!("dev.test_hint"))
                    .disabled(self.busy.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.run_test(cx)))
            })
            .into_any_element()
    }

    /// The number of lines the test log has, to follow it while it grows.
    pub(super) fn log_len(&self, cx: &Context<Self>) -> usize {
        let state = AppState::global(cx);
        let s = state.read(cx);
        self.test_instance()
            .and_then(|id| s.sessions.get(&id))
            .map_or(0, |session| session.log.len())
    }

    pub(super) fn open_test_instance(&self, cx: &mut Context<Self>) {
        if let Some(id) = self.test_instance() {
            AppState::global(cx).update(cx, |s, cx| s.navigate(Route::Instance(id), cx));
        }
    }
}
