use std::collections::HashMap;

use gpui::{AnyView, App, AppContext as _, Context, Entity, Global, Pixels};
use riven_format::{Account, Accounts, Instance, Settings};
use riven_launch::instances::Instances;

use super::session::Session;
use moonglass::{ContextMenu, MenuEntry, Modal};

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
    /// What the last pack update changed, per instance that has had one.
    pub pack_changes: HashMap<String, riven_format::Changes>,
    /// Icon file per instance id, for instances that have one.
    pub icons: HashMap<String, std::path::PathBuf>,
    /// Skin heads by Minecraft UUID (a Microsoft account's id is its UUID), once fetched.
    pub skins: HashMap<String, std::path::PathBuf>,
    /// BX ID profile pictures by URL, as cached thumbnails once fetched.
    pub pictures: HashMap<String, std::path::PathBuf>,
    pictures_asked: std::collections::HashSet<String>,
    pub accounts: Accounts,
    pub route: Route,
    /// A load or save that failed, shown until the next success.
    pub error: Option<String>,
    pub modal: Option<Modal>,
    modal_serial: u64,
    pub context_menu: Option<ContextMenu>,
    /// Launches and pack installs by instance id, kept after they end for their log.
    pub sessions: HashMap<String, Session>,
    /// Bumped when an install or update changed an instance's files, so open views read them again.
    pub revisions: HashMap<String, u64>,
    /// Newer Modrinth versions of the modpacks instances were installed from, by instance id.
    pub modpack_updates: HashMap<String, riven_launch::modpack::Newer>,
    /// `(read, to read)` while an instance's content files are hashed, by instance id.
    pub scans: HashMap<String, (usize, usize)>,
    pub(super) ticking: bool,
    pub update: super::updater::Update,
    /// An instance whose view should switch to its Worlds tab; the view clears it.
    pub show_worlds: Option<String>,
}

/// A skin head saved earlier for an account, `<id>-<hash>.png`.
fn cached_skin(dir: &std::path::Path, id: &str) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{id}-")))
        })
}

/// Deletes the cached heads whose names start with `prefix`.
fn forget_heads(dir: &std::path::Path, prefix: &str) {
    for old in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if old.file_name().to_string_lossy().starts_with(prefix) {
            let _ = std::fs::remove_file(old.path());
        }
    }
}

/// A stable hash for file names that change with their contents.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

/// A profile picture as a small PNG, `<url hash>-<content hash>.png`, so a new picture gets a new path.
async fn fetch_picture(dir: &std::path::Path, url: &str) -> Option<std::path::PathBuf> {
    let bytes = riven_sources::client()
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .ok()?
        .bytes()
        .await
        .ok()?;
    let prefix = format!("{:016x}-", fnv(url.as_bytes()));
    let path = dir.join(format!("{prefix}{:016x}.png", fnv(&bytes)));
    if path.is_file() {
        return Some(path);
    }
    let png = tokio::task::spawn_blocking(move || super::mods::picture(&bytes))
        .await
        .ok()??;
    forget_heads(dir, &prefix);
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(&path, png).ok()?;
    Some(path)
}

fn read_icons(
    store: Option<&Instances>,
    instances: &[(String, Instance)],
) -> HashMap<String, std::path::PathBuf> {
    let Some(store) = store else {
        return HashMap::new();
    };
    instances
        .iter()
        .filter_map(|(id, _)| Some((id.clone(), store.icon(id)?)))
        .collect()
}

type Packs = (
    HashMap<String, String>,
    HashMap<String, riven_format::Changes>,
);

/// The installed pack version per instance, and what its last update changed.
fn read_packs(store: Option<&Instances>, instances: &[(String, Instance)]) -> Packs {
    let mut packs = Packs::default();
    let Some(store) = store else {
        return packs;
    };
    for (id, _) in instances {
        let Ok(Some(state)) = riven_sync::install::load_state(&store.game_dir(id)) else {
            continue;
        };
        packs.0.insert(id.clone(), state.version);
        if let Some(changes) = state.changes {
            packs.1.insert(id.clone(), changes);
        }
    }
    packs
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
        let (packs, pack_changes) = read_packs(store.as_ref(), &instances);
        let icons = read_icons(store.as_ref(), &instances);
        Self {
            settings,
            store,
            instances,
            packs,
            pack_changes,
            icons,
            skins: HashMap::new(),
            pictures: HashMap::new(),
            pictures_asked: Default::default(),
            accounts,
            route,
            error,
            modal: None,
            modal_serial: 0,
            context_menu: None,
            sessions: HashMap::new(),
            scans: HashMap::new(),
            modpack_updates: HashMap::new(),
            revisions: HashMap::new(),
            ticking: false,
            update: Default::default(),
            show_worlds: None,
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

    /// Opens an instance on its Worlds tab.
    pub fn show_worlds(&mut self, id: &str, cx: &mut Context<Self>) {
        self.show_worlds = Some(id.to_owned());
        self.navigate(Route::Instance(id.to_owned()), cx);
    }

    /// Reads the instance list from disk again, keeping the selection when it still exists.
    pub fn reload_instances(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            match store.list() {
                Ok(list) => self.instances = list,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        (self.packs, self.pack_changes) = read_packs(self.store.as_ref(), &self.instances);
        self.icons = read_icons(self.store.as_ref(), &self.instances);
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

    /// Opens a menu at the pointer, replacing one already open.
    pub fn open_context_menu(
        &mut self,
        position: gpui::Point<Pixels>,
        entries: Vec<MenuEntry>,
        cx: &mut Context<Self>,
    ) {
        self.modal_serial += 1;
        self.context_menu = Some(ContextMenu {
            position,
            entries,
            serial: self.modal_serial,
        });
        cx.notify();
    }

    pub fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
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
                .timer(moonglass::motion::EXIT)
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

    /// Shows the cached skin heads of Microsoft accounts, then fetches them again in the background.
    pub fn load_skins(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .accounts
            .accounts
            .iter()
            .filter(|a| a.kind == riven_format::AccountKind::Microsoft)
            .map(|a| a.id.clone())
            .collect();
        self.fetch_skins(ids, cx);
    }

    /// Heads of the offline nicks this BX ID holds, under `nick:<lower-cased name>` in `skins`.
    pub fn show_nick_skins(
        &mut self,
        worn: Vec<(String, Option<riven_play::skins::Skin>)>,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = super::mods::cache_dir().map(|d| d.join("skins")) else {
            return;
        };
        let worn: Vec<(String, Option<riven_play::skins::Skin>)> = worn
            .into_iter()
            .map(|(name, skin)| (name.to_ascii_lowercase(), skin))
            .collect();
        for (name, _) in &worn {
            if let Some(path) = cached_skin(&dir, &format!("nick-{name}")) {
                self.skins.insert(format!("nick:{name}"), path);
            }
        }
        cx.notify();
        let fetched = super::runtime::spawn(async move {
            let mut heads = Vec::new();
            for (name, skin) in worn {
                let prefix = format!("nick-{name}-");
                let path = match skin {
                    Some(skin) => {
                        let path = dir.join(format!(
                            "{prefix}{}.png",
                            &skin.hash[..16.min(skin.hash.len())]
                        ));
                        if path.is_file() {
                            Some(path)
                        } else {
                            let png = riven_play::skins::download(&skin.url).await.ok();
                            match png.as_deref().and_then(super::mods::skin_head) {
                                Some(head) => {
                                    forget_heads(&dir, &prefix);
                                    let _ = std::fs::create_dir_all(&dir);
                                    std::fs::write(&path, head).ok().map(|()| path)
                                }
                                None => None,
                            }
                        }
                    }
                    // The skin came off: the old head must go.
                    None => {
                        forget_heads(&dir, &prefix);
                        None
                    }
                };
                heads.push((name, path));
            }
            heads
        });
        cx.spawn(async move |this, cx| {
            if let Ok(heads) = fetched.await {
                let _ = this.update(cx, |s, cx| {
                    for (name, path) in heads {
                        match path {
                            Some(path) => s.skins.insert(format!("nick:{name}"), path),
                            None => s.skins.remove(&format!("nick:{name}")),
                        };
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Shows the cached pictures at `urls` at once, then fetches each once per run:
    /// a GitHub avatar keeps its address when the picture behind it changes.
    pub fn want_pictures(&mut self, urls: Vec<String>, cx: &mut Context<Self>) {
        let Some(dir) = super::mods::cache_dir().map(|d| d.join("pictures")) else {
            return;
        };
        let new: Vec<String> = urls
            .into_iter()
            .filter(|url| url.starts_with("https://") && self.pictures_asked.insert(url.clone()))
            .collect();
        if new.is_empty() {
            return;
        }
        for url in &new {
            if let Some(path) = cached_skin(&dir, &format!("{:016x}", fnv(url.as_bytes()))) {
                self.pictures.insert(url.clone(), path);
            }
        }
        cx.notify();
        for url in new {
            let dir = dir.clone();
            let fetched = super::runtime::spawn(async move {
                let path = fetch_picture(&dir, &url).await?;
                Some((url, path))
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Some((url, path))) = fetched.await {
                    let _ = this.update(cx, |s, cx| {
                        s.pictures.insert(url, path);
                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }

    /// Fetches a Microsoft account's head again, after its skin changed.
    pub fn refresh_skin(&mut self, id: String, cx: &mut Context<Self>) {
        self.fetch_skins(vec![id], cx);
    }

    /// Shows the cached heads of `ids` at once, then fetches them again in the background.
    fn fetch_skins(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let Some(dir) = super::mods::cache_dir().map(|d| d.join("skins")) else {
            return;
        };
        for id in &ids {
            if let Some(path) = cached_skin(&dir, id) {
                self.skins.insert(id.clone(), path);
            }
        }
        cx.notify();
        for id in ids {
            let dir = dir.clone();
            let fetched = super::runtime::spawn(async move {
                let skin = riven_launch::accounts::skin(&id).await.ok()?;
                let head = super::mods::skin_head(&skin)?;
                std::fs::create_dir_all(&dir).ok()?;
                let path = dir.join(format!("{id}-{:016x}.png", fnv(&head)));
                if !path.is_file() {
                    for old in std::fs::read_dir(&dir).ok()?.flatten() {
                        if old
                            .file_name()
                            .to_string_lossy()
                            .starts_with(&format!("{id}-"))
                        {
                            let _ = std::fs::remove_file(old.path());
                        }
                    }
                    std::fs::write(&path, head).ok()?;
                }
                Some((id, path))
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Some((id, path))) = fetched.await {
                    let _ = this.update(cx, |s, cx| {
                        s.skins.insert(id, path);
                        cx.notify();
                    });
                }
            })
            .detach();
        }
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
        self.load_skins(cx);
    }

    /// Adds an offline account for each nick not here yet, leaving the selected account as it is.
    pub fn adopt_nicks(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        let mut added = false;
        for name in names {
            let known = self.accounts.accounts.iter().any(|a| {
                a.kind == riven_format::AccountKind::Offline && a.name.eq_ignore_ascii_case(&name)
            });
            if known {
                continue;
            }
            if let Ok(account) = riven_launch::accounts::offline(&name) {
                self.accounts.accounts.push(account);
                added = true;
            }
        }
        if !added {
            return;
        }
        if let Err(e) = riven_launch::accounts::save(&self.accounts) {
            self.error = Some(e.to_string());
        }
        cx.notify();
    }

    pub fn remove_account(&mut self, id: &str, cx: &mut Context<Self>) {
        self.accounts.accounts.retain(|a| a.id != id);
        let forget = id.to_owned();
        let forgot =
            super::runtime::spawn(async move { riven_launch::accounts::forget(&forget).await });
        cx.background_spawn(async move {
            if let Ok(Err(e)) = forgot.await {
                tracing::warn!("{e}");
            }
        })
        .detach();
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
