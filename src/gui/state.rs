use std::collections::HashMap;

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, Global, Pixels};
use riven_format::{Account, Accounts, Instance, Settings};
use riven_launch::instances::Instances;

use super::session::Session;
use super::ui::Modal;

/// The screen shown next to the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Instance(String),
    Settings,
    Developer,
    Empty,
}

/// Everything the views share: launcher settings, instances and accounts.
pub struct AppState {
    pub settings: Settings,
    pub store: Option<Instances>,
    pub instances: Vec<(String, Instance)>,
    /// Installed pack version per instance id, for instances that came from a pack.
    pub packs: HashMap<String, String>,
    pub accounts: Accounts,
    pub route: Route,
    /// A load or save that failed, shown until the next success.
    pub error: Option<String>,
    pub modal: Option<Modal>,
    modal_serial: u64,
    /// Launches and pack installs by instance id, kept after they end for their log.
    pub sessions: HashMap<String, Session>,
    pub(super) ticking: bool,
    pub(super) toasts: Vec<super::toast::Toast>,
    pub(super) toast_serial: u64,
}

fn read_packs(
    store: Option<&Instances>,
    instances: &[(String, Instance)],
) -> HashMap<String, String> {
    let Some(store) = store else {
        return HashMap::new();
    };
    instances
        .iter()
        .filter_map(|(id, _)| {
            let state = riven_sync::install::load_state(&store.game_dir(id)).ok()??;
            Some((id.clone(), state.version))
        })
        .collect()
}

struct GlobalState(Entity<AppState>);

impl Global for GlobalState {}

impl AppState {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let state = cx.new(|_| Self::load());
        cx.set_global(GlobalState(state.clone()));
        state
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalState>().0.clone()
    }

    fn load() -> Self {
        let mut error = None;
        let mut note = |e: riven_launch::LaunchError| {
            tracing::error!("{e}");
            error = Some(e.to_string());
        };
        let settings = riven_launch::load_settings().unwrap_or_else(|e| {
            note(e);
            Settings::default()
        });
        let accounts = riven_launch::accounts::load().unwrap_or_else(|e| {
            note(e);
            Accounts::default()
        });
        let store = Instances::default_location().map_err(&mut note).ok();
        let instances = store
            .as_ref()
            .and_then(|s| s.list().map_err(&mut note).ok())
            .unwrap_or_default();
        let route = settings
            .selected_instance
            .clone()
            .filter(|id| instances.iter().any(|(i, _)| i == id))
            .or_else(|| instances.first().map(|(id, _)| id.clone()))
            .map(Route::Instance)
            .unwrap_or(Route::Empty);
        let packs = read_packs(store.as_ref(), &instances);
        Self {
            settings,
            store,
            instances,
            packs,
            accounts,
            route,
            error,
            modal: None,
            modal_serial: 0,
            sessions: HashMap::new(),
            ticking: false,
            toasts: Vec::new(),
            toast_serial: 0,
        }
    }

    pub fn instance(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|(i, _)| i == id).map(|(_, i)| i)
    }

    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if let Route::Instance(id) = &route
            && self.settings.selected_instance.as_deref() != Some(id)
        {
            self.settings.selected_instance = Some(id.clone());
            self.save_settings();
        }
        self.route = route;
        cx.notify();
    }

    /// Reads the instance list from disk again, keeping the selection when it still exists.
    pub fn reload_instances(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            match store.list() {
                Ok(list) => self.instances = list,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        self.packs = read_packs(self.store.as_ref(), &self.instances);
        cx.notify();
    }

    /// Writes a changed instance and refreshes the list, so the sidebar and header follow.
    pub fn save_instance(&mut self, id: &str, instance: &Instance, cx: &mut Context<Self>) {
        if let Some(store) = &self.store
            && let Err(e) = store.save(id, instance)
        {
            self.error = Some(e.to_string());
        }
        self.reload_instances(cx);
    }

    /// Copies an instance in the background and opens the copy.
    pub fn duplicate_instance(&mut self, id: &str, cx: &mut Context<Self>) {
        let (Some(store), Some(instance)) = (self.store.clone(), self.instance(id)) else {
            return;
        };
        let name = rust_i18n::t!("instance.copy_name", name = instance.name).to_string();
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let copied = super::runtime::blocking(move || store.duplicate(&id, &name)).await;
            let _ = this.update(cx, |s, cx| match copied {
                Ok(Ok(new_id)) => {
                    s.reload_instances(cx);
                    s.navigate(Route::Instance(new_id), cx);
                }
                Ok(Err(e)) => s.error = Some(e.to_string()),
                Err(_) => {}
            });
        })
        .detach();
    }

    /// Deletes an instance that is not running and selects a neighbour.
    pub fn delete_instance(&mut self, id: &str, cx: &mut Context<Self>) {
        if self
            .sessions
            .get(id)
            .is_some_and(super::session::Session::busy)
        {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        if let Err(e) = store.delete(id) {
            tracing::error!("{e}");
            self.error = Some(e.to_string());
            return;
        }
        self.sessions.remove(id);
        self.reload_instances(cx);
        let next = self
            .instances
            .first()
            .map(|(i, _)| Route::Instance(i.clone()));
        self.settings.selected_instance = None;
        self.navigate(next.unwrap_or(Route::Empty), cx);
    }

    pub fn open_modal(&mut self, view: AnyView, width: Pixels, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        self.modal_serial += 1;
        self.modal = Some(Modal {
            view,
            width,
            focus,
            serial: self.modal_serial,
            closing: false,
        });
        cx.notify();
    }

    /// Fades the modal out, then drops it.
    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = &mut self.modal else {
            return;
        };
        if cx.reduce_motion() {
            self.modal = None;
            cx.notify();
            return;
        }
        if modal.closing {
            return;
        }
        modal.closing = true;
        let serial = modal.serial;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(super::ui::motion::EXIT)
                .await;
            let _ = this.update(cx, |s, cx| {
                if s.modal.as_ref().is_some_and(|m| m.serial == serial) {
                    s.modal = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Where "back" leads from screens that hide the instance list.
    pub fn home(&self) -> Route {
        self.settings
            .selected_instance
            .clone()
            .filter(|id| self.instance(id).is_some())
            .map(Route::Instance)
            .unwrap_or(Route::Empty)
    }

    /// Adds an account, or replaces the one with the same id, and selects it.
    pub fn add_account(&mut self, account: Account, cx: &mut Context<Self>) {
        let id = account.id.clone();
        self.accounts.accounts.retain(|a| a.id != id);
        self.accounts.accounts.push(account);
        if let Err(e) = riven_launch::accounts::save(&self.accounts) {
            self.error = Some(e.to_string());
        }
        self.update_settings(|s| s.selected_account = Some(id), cx);
    }

    pub fn remove_account(&mut self, id: &str, cx: &mut Context<Self>) {
        self.accounts.accounts.retain(|a| a.id != id);
        if let Err(e) = riven_launch::accounts::save(&self.accounts) {
            self.error = Some(e.to_string());
        }
        if self.settings.selected_account.as_deref() == Some(id) {
            self.update_settings(|s| s.selected_account = None, cx);
        } else {
            cx.notify();
        }
    }

    /// Changes the settings and writes them to disk.
    pub fn update_settings(&mut self, edit: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        edit(&mut self.settings);
        self.save_settings();
        cx.notify();
    }

    fn save_settings(&mut self) {
        match riven_launch::save_settings(&self.settings) {
            Ok(()) => self.error = None,
            Err(e) => {
                tracing::error!("{e}");
                self.error = Some(e.to_string());
            }
        }
    }
}
