use gpui::prelude::FluentBuilder as _;
use gpui::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, Global, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px, relative,
};
use gpui_base::Popover;
use gpui_base::input::{InputEvent, InputState};
use moonglass::theme::ActiveTheme as _;
use moonglass::toast::ToastKind;
use moonglass::{
    Button, ButtonSize, IconName, Section, TextField, UiText as _, W_SEMIBOLD, h_flex, icon, v_flex,
};
use std::collections::HashSet;
use std::sync::Arc;

use riven_format::AccountKind;
use riven_play::friends::{self, AddStatus};
use riven_play::id::{self, DeviceCode, Me};
use riven_play::lobby::{Lobby, LobbyGame};
use riven_play::presence::{self, Event, ServerEvent};
use riven_play::skins::{self, MyProfiles};
use riven_play::tunnel::{Guest, Host, HostEvent, Signals};
use riven_play::{Activity, Friend, Friends, PlayError, PlayUser, Status as Visibility, lan};
use rust_i18n::t;

use super::dialogs::{Back, dialog_frame, leave};
use super::runtime;
use super::session::Phase;
use super::state::AppState;

/// How long an invite stays as a card in the corner of the window.
const INVITE_CARD: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether this launcher is signed in to BX ID.
#[derive(Clone, PartialEq)]
pub enum Status {
    /// Asking BX ID at start-up.
    Checking,
    SignedOut,
    SignedIn(Me),
}

/// A world of a running instance shared with friends through a lobby.
pub struct Hosting {
    pub instance: String,
    /// `None` while the lobby opens.
    host: Option<Arc<Host>>,
    /// Invited BX ID ids, including those asked while the lobby was still opening.
    pub invited: Vec<String>,
    /// Guests whose connection is up.
    pub joined: Vec<String>,
    /// Every friend may join without an invite.
    pub open: bool,
    epoch: u64,
}

pub enum Joining {
    Connecting(Lobby),
    Joined { lobby: Lobby, guest: Arc<Guest> },
}

impl Joining {
    pub fn lobby(&self) -> &Lobby {
        match self {
            Self::Connecting(lobby) | Self::Joined { lobby, .. } => lobby,
        }
    }
}

/// BX ID, friends and presence: one per app, alive while the launcher runs.
pub struct PlayState {
    pub status: Status,
    pub friends: Option<Friends>,
    /// The offline nicks this BX ID holds, with their skins.
    pub nicks: Option<MyProfiles>,
    /// Offline accounts here whose nick another BX ID holds, lower-cased.
    pub taken: HashSet<String>,
    /// Nicks of accounts removed here whose release is under way, lower-cased: never added back meanwhile.
    releasing: HashSet<String>,
    /// Whether the presence socket is up; friends see this launcher online only then.
    pub connected: bool,
    pub error: Option<SharedString>,
    /// The running instance whose world is open to LAN, and its port.
    pub lan: Option<(String, u16)>,
    /// Whether a game runs at all, to tell the player to open the world to LAN.
    pub playing: bool,
    pub hosting: Option<Hosting>,
    pub joining: Option<Joining>,
    /// Lobbies friends invited this player to and the friend who asked.
    pub invites: Vec<(Lobby, PlayUser)>,
    /// Invites still shown as cards in the corner; they stay in the friends list after.
    pub fresh: Vec<String>,
    /// What this account shows friends; the server keeps it.
    pub visibility: Visibility,
    connection: Option<presence::Connection>,
    signals: Signals,
    /// The running instance, before the open lobby is added to it.
    game: Option<Activity>,
    /// What was last sent to friends.
    activity: Option<Activity>,
    epoch: u64,
    /// The `bxid_off` setting last seen, so switching it connects or disconnects once.
    enabled: bool,
    _sessions: Option<Subscription>,
}

struct GlobalPlay(Entity<PlayState>);

impl Global for GlobalPlay {}

impl PlayState {
    pub fn init(cx: &mut App) {
        let state = cx.new(|cx| {
            let sessions = cx.observe(&AppState::global(cx), |this: &mut Self, app, cx| {
                this.follow_setting(&app, cx);
                this.follow_game(&app, cx)
            });
            let enabled = !AppState::global(cx).read(cx).settings.bxid_off;
            let mut this = Self {
                status: Status::Checking,
                friends: None,
                nicks: None,
                taken: HashSet::new(),
                releasing: HashSet::new(),
                connected: false,
                error: None,
                lan: None,
                playing: false,
                hosting: None,
                joining: None,
                invites: Vec::new(),
                fresh: Vec::new(),
                visibility: Visibility::Online,
                connection: None,
                signals: Signals::default(),
                game: None,
                activity: None,
                epoch: 0,
                enabled,
                _sessions: Some(sessions),
            };
            if enabled {
                this.check(cx);
            } else {
                this.status = Status::SignedOut;
            }
            this
        });
        cx.set_global(GlobalPlay(state));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPlay>().0.clone()
    }

    /// Whether BX ID is on; off, the launcher shows and connects nothing of it.
    pub fn enabled(cx: &App) -> bool {
        !AppState::global(cx).read(cx).settings.bxid_off
    }

    /// Connects or drops everything when BX ID is switched on or off; the session is kept.
    fn follow_setting(&mut self, app: &Entity<AppState>, cx: &mut Context<Self>) {
        let enabled = !app.read(cx).settings.bxid_off;
        if enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        if enabled {
            self.status = Status::Checking;
            self.check(cx);
        } else {
            self.forget(cx);
        }
    }

    pub fn me(&self) -> Option<&Me> {
        match &self.status {
            Status::SignedIn(me) => Some(me),
            _ => None,
        }
    }

    fn check(&mut self, cx: &mut Context<Self>) {
        let me = runtime::spawn(async {
            // scripts/bxid-dev.sh hands each sandboxed launcher the session `bun dev:id` seeded.
            match std::env::var("RIVEN_BXID_TOKEN") {
                Ok(token) if !token.is_empty() => id::sign_in_with_token(token).await.map(Some),
                _ => id::me().await,
            }
        });
        cx.spawn(async move |this, cx| {
            let me = me.await;
            let _ = this.update(cx, |s, cx| match me {
                Ok(Ok(Some(me))) => s.signed_in(me, cx),
                Ok(Ok(None)) => {
                    s.status = Status::SignedOut;
                    cx.notify();
                }
                Ok(Err(e)) => {
                    // Offline or BX Team down: keep the session and try again on the next start.
                    tracing::warn!("BX ID: {e}");
                    s.status = Status::SignedOut;
                    s.error = Some(super::problem::play_error(&e).into());
                    cx.notify();
                }
                Err(_) => {}
            });
        })
        .detach();
    }

    pub fn signed_in(&mut self, me: Me, cx: &mut Context<Self>) {
        self.status = Status::SignedIn(me);
        self.error = None;
        self.connect(cx);
        self.refresh(cx);
        self.sync_nicks(cx);
        cx.notify();
    }

    /// Whether this BX ID holds `name`, so the account can wear a skin.
    pub fn owns(&self, name: &str) -> bool {
        self.nicks.as_ref().is_some_and(|n| n.get(name).is_some())
    }

    pub fn is_taken(&self, name: &str) -> bool {
        self.taken.contains(&name.to_ascii_lowercase())
    }

    /// Claims the free nicks of this computer's offline accounts, then shows the skin of each nick held.
    /// A nick released on the site stays released: only adding the account again claims it.
    pub fn sync_nicks(&mut self, cx: &mut Context<Self>) {
        if self.me().is_none() {
            return;
        }
        let names: Vec<String> = AppState::global(cx)
            .read(cx)
            .accounts
            .accounts
            .iter()
            .filter(|a| a.kind == AccountKind::Offline)
            .map(|a| a.name.clone())
            .collect();
        let synced = runtime::spawn(async move {
            let mut mine = skins::profiles().await?;
            let mut taken = HashSet::new();
            for name in names {
                let released = mine.held.iter().any(|h| h.name.eq_ignore_ascii_case(&name));
                if released || mine.get(&name).is_some() || mine.profiles.len() >= mine.limit {
                    continue;
                }
                match skins::claim(&name).await {
                    Ok(profile) => mine.profiles.push(profile),
                    Err(PlayError::Taken(_)) => {
                        taken.insert(name.to_ascii_lowercase());
                    }
                    Err(e) => tracing::warn!("nick {name}: {e}"),
                }
            }
            Ok::<_, PlayError>((mine, taken))
        });
        cx.spawn(async move |this, cx| {
            let Ok(synced) = synced.await else {
                return;
            };
            let _ = this.update(cx, |s, cx| match synced {
                Ok((mine, taken)) => {
                    // A nick claimed on bxteam.org or another computer becomes an offline account here too.
                    let missing: Vec<String> = mine
                        .profiles
                        .iter()
                        .filter(|p| !s.releasing.contains(&p.name.to_ascii_lowercase()))
                        .map(|p| p.name.clone())
                        .collect();
                    AppState::global(cx).update(cx, |app, cx| app.adopt_nicks(missing, cx));
                    let worn = mine
                        .profiles
                        .iter()
                        .map(|p| (p.name.clone(), p.skin.clone()))
                        .collect();
                    AppState::global(cx).update(cx, |app, cx| app.show_nick_skins(worn, cx));
                    s.nicks = Some(mine);
                    s.taken = taken;
                    cx.notify();
                }
                Err(PlayError::SignedOut) => s.forget(cx),
                Err(e) => tracing::warn!("nicks: {e}"),
            });
        })
        .detach();
    }

    /// Lets go of a nick when its offline account leaves this launcher; held a few days for this BX ID.
    pub fn release_nick(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.owns(&name) {
            return;
        }
        self.releasing.insert(name.to_ascii_lowercase());
        let released = runtime::spawn(async move { skins::release(&name).await });
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = released.await {
                tracing::warn!("{e}");
            }
            let _ = this.update(cx, |s, cx| s.sync_nicks(cx));
        })
        .detach();
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let activity = self.activity.clone();
        let connection = runtime::spawn(async move {
            let connection = presence::connect(move |event| {
                let _ = tx.send(event);
            });
            connection.set_activity(activity);
            connection
        });
        cx.spawn(async move |this, cx| {
            let Ok(connection) = connection.await else {
                return;
            };
            if this
                .update(cx, |s, _| s.connection = Some(connection))
                .is_err()
            {
                return;
            }
            while let Some(event) = rx.recv().await {
                if this.update(cx, |s, cx| s.on_event(event, cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Connected => {
                self.connected = true;
                self.refresh(cx);
                self.check_invites(cx);
            }
            Event::Disconnected => self.connected = false,
            Event::SignedOut => self.forget(cx),
            Event::Server(ServerEvent::Status { status }) => self.visibility = status,
            Event::Server(ServerEvent::Presence { user_id, presence }) => {
                if let Some(friend) = self.friend_mut(&user_id) {
                    friend.presence = presence;
                }
            }
            Event::Server(ServerEvent::FriendRequest { user }) => {
                self.alert(t!("friends.toast_request", name = user.shown()).into(), cx);
                self.refresh(cx);
            }
            Event::Server(ServerEvent::FriendAdded { user }) => {
                if !panel_open(cx) {
                    moonglass::toast::show(
                        ToastKind::Info,
                        t!("friends.toast_added", name = user.shown()).to_string(),
                        cx,
                    );
                }
                self.refresh(cx);
            }
            Event::Server(ServerEvent::FriendRemoved { .. }) => self.refresh(cx),
            Event::Server(ServerEvent::Skin { user_id }) => {
                if self.me().is_some_and(|me| me.id == user_id) {
                    self.sync_nicks(cx);
                }
            }
            Event::Server(ServerEvent::Signal { lobby, from, data }) => {
                self.signals.deliver(&lobby, from, data)
            }
            Event::Server(ServerEvent::Invite { lobby, from }) => {
                self.ring(cx);
                if let Some(url) = from.image.clone() {
                    AppState::global(cx).update(cx, |app, cx| app.want_pictures(vec![url], cx));
                }
                let id = lobby.id.clone();
                self.invites.retain(|(l, _)| l.id != lobby.id);
                self.invites.push((*lobby, from));
                self.fresh.retain(|l| *l != id);
                if panel_open(cx) {
                    cx.notify();
                    return;
                }
                self.fresh.push(id.clone());
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(INVITE_CARD).await;
                    let _ = this.update(cx, |s, cx| {
                        s.fresh.retain(|l| *l != id);
                        cx.notify();
                    });
                })
                .detach();
            }
            Event::Server(ServerEvent::Lobby { lobby }) => {
                // Joining or declining from another launcher uses the invite up.
                if let Status::SignedIn(me) = &self.status
                    && !lobby.invited.contains(&me.id)
                {
                    self.invites.retain(|(l, _)| l.id != lobby.id);
                }
                if let Some(hosting) = &mut self.hosting
                    && hosting
                        .host
                        .as_ref()
                        .is_some_and(|h| h.lobby.id == lobby.id)
                {
                    hosting.invited = lobby.invited.clone();
                    hosting.open = lobby.open;
                }
                if let Status::SignedIn(me) = &self.status
                    && !lobby.members.contains(&me.id)
                    && lobby.host != me.id
                {
                    let id = lobby.id.clone();
                    self.kicked_out(&id, cx);
                }
            }
            Event::Server(ServerEvent::LobbyClosed { lobby }) => {
                self.invites.retain(|(l, _)| l.id != lobby);
                if let Some(joining) = &self.joining
                    && joining.lobby().id == lobby
                {
                    moonglass::toast::show(
                        ToastKind::Info,
                        t!("friends.toast_closed", name = joining.lobby().name).to_string(),
                        cx,
                    );
                    self.leave(cx);
                }
            }
            Event::Server(_) => {}
        }
        cx.notify();
    }

    /// A toast for something addressed to this player, with the launcher's sound;
    /// the open friends panel already shows it, so then only the sound.
    fn alert(&self, text: String, cx: &mut Context<Self>) {
        if !panel_open(cx) {
            moonglass::toast::show(ToastKind::Info, text, cx);
        }
        self.ring(cx);
    }

    /// The chime alone, for news shown some other way than a toast.
    fn ring(&self, cx: &App) {
        let sound_off = AppState::global(cx).read(cx).settings.alert_sound_off;
        if self.visibility != Visibility::Busy && !sound_off {
            super::sound::chime();
        }
    }

    /// Shows friends this status; the server tells this account's other launchers.
    pub fn set_visibility(&mut self, visibility: Visibility, cx: &mut Context<Self>) {
        self.visibility = visibility;
        if let Some(connection) = &self.connection {
            connection.set_status(visibility);
        }
        cx.notify();
    }

    fn friend_mut(&mut self, user_id: &str) -> Option<&mut Friend> {
        self.friends
            .as_mut()?
            .friends
            .iter_mut()
            .find(|f| f.user.id == user_id)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let fetched = runtime::spawn(friends::list());
        cx.spawn(async move |this, cx| {
            let Ok(list) = fetched.await else {
                return;
            };
            let _ = this.update(cx, |s, cx| {
                match list {
                    Ok(list) => {
                        let mut pictures: Vec<String> = list
                            .friends
                            .iter()
                            .map(|f| &f.user)
                            .chain(&list.incoming)
                            .chain(&list.outgoing)
                            .filter_map(|u| u.image.clone())
                            .collect();
                        pictures.extend(s.me().and_then(|me| me.image.clone()));
                        AppState::global(cx).update(cx, |app, cx| app.want_pictures(pictures, cx));
                        s.friends = Some(list);
                    }
                    Err(PlayError::SignedOut) => return s.forget(cx),
                    Err(e) => s.error = Some(super::problem::play_error(&e).into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Tells friends which instance is running and follows its world being opened to LAN.
    fn follow_game(&mut self, app: &Entity<AppState>, cx: &mut Context<Self>) {
        let st = app.read(cx);
        let running = || {
            st.sessions
                .iter()
                .filter(|(_, s)| matches!(s.phase, Phase::Running { .. }))
        };
        self.playing = running().next().is_some();
        let lan = running().find_map(|(id, session)| {
            let port = session.log.iter().rev().find_map(|(_, line)| {
                lan::opened_port(line)
                    .map(Some)
                    .or_else(|| lan::stopped(line).then_some(None))
            })??;
            Some((id.clone(), port))
        });
        if lan != self.lan {
            self.lan = lan.clone();
            match (&self.hosting, &lan) {
                (Some(h), Some((instance, port))) if &h.instance == instance => {
                    if let Some(host) = &h.host {
                        host.set_port(*port);
                    }
                }
                (Some(_), _) => self.stop_hosting(cx),
                (None, _) => {}
            }
            cx.notify();
        }
        let st = app.read(cx);
        self.game = st.sessions.iter().find_map(|(id, session)| {
            matches!(session.phase, Phase::Running { .. }).then(|| {
                let instance = st.instances.iter().find(|(i, _)| i == id).map(|(_, i)| i)?;
                Some(Activity {
                    name: instance.name.clone(),
                    minecraft: instance.minecraft.clone(),
                    lobby: None,
                    players: 0,
                })
            })?
        });
        self.send_activity();
    }

    /// The running game, with the lobby friends may walk into when it is open.
    fn send_activity(&mut self) {
        let open = self
            .hosting
            .as_ref()
            .filter(|h| h.open)
            .and_then(|h| Some((h.host.as_ref()?.lobby.id.clone(), h.joined.len() as u32 + 1)));
        let activity = self.game.clone().map(|game| Activity {
            players: open.as_ref().map_or(0, |(_, n)| *n),
            lobby: open.map(|(id, _)| id),
            ..game
        });
        if activity == self.activity {
            return;
        }
        self.activity = activity.clone();
        if let Some(connection) = &self.connection {
            connection.set_activity(activity);
        }
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        let done = runtime::spawn(id::sign_out());
        cx.background_spawn(async move {
            if let Ok(Err(e)) = done.await {
                tracing::warn!("BX ID sign-out: {e}");
            }
        })
        .detach();
        self.forget(cx);
    }

    /// Invites a friend to the world open to LAN, opening its lobby first if needed.
    pub fn invite(&mut self, user_id: String, cx: &mut Context<Self>) {
        if let Some(hosting) = &mut self.hosting {
            if hosting.invited.contains(&user_id) {
                return;
            }
            hosting.invited.push(user_id.clone());
            if let Some(host) = hosting.host.clone() {
                let done = runtime::spawn(async move { host.invite(&user_id).await.map(|_| ()) });
                self.report(done, cx);
            }
            cx.notify();
            return;
        }
        self.host(false, vec![user_id], cx);
    }

    /// Shares the world open to LAN with every friend (`open`) or only those invited,
    /// opening its lobby first if needed.
    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let Some(hosting) = &mut self.hosting else {
            self.host(open, Vec::new(), cx);
            return;
        };
        hosting.open = open;
        if let Some(host) = hosting.host.clone() {
            let id = host.lobby.id.clone();
            let done =
                runtime::spawn(
                    async move { riven_play::lobby::set_open(&id, open).await.map(|_| ()) },
                );
            self.report(done, cx);
        }
        self.send_activity();
        cx.notify();
    }

    /// Opens a lobby for the world open to LAN.
    fn host(&mut self, open: bool, invited: Vec<String>, cx: &mut Context<Self>) {
        let Some((instance_id, port)) = self.lan.clone() else {
            return;
        };
        let Some(signaller) = self.connection.as_ref().map(|c| c.signaller()) else {
            return;
        };
        let app = AppState::global(cx);
        let Some(instance) = app
            .read(cx)
            .instances
            .iter()
            .find(|(id, _)| *id == instance_id)
            .map(|(_, i)| i.clone())
        else {
            return;
        };
        self.epoch += 1;
        let epoch = self.epoch;
        let name = instance.name.clone();
        self.hosting = Some(Hosting {
            instance: instance_id,
            host: None,
            invited,
            joined: Vec::new(),
            open,
            epoch,
        });
        let game = LobbyGame::of(&instance);
        let signals = self.signals.clone();
        let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let opened = runtime::spawn(async move {
            Host::open(&name, &game, open, port, signaller, &signals, move |e| {
                let _ = events_tx.send(e);
            })
            .await
            .map(Arc::new)
        });
        cx.spawn(async move |this, cx| {
            let opened = opened.await;
            let invited = this.update(cx, |s, cx| {
                let current = s.hosting.as_mut().filter(|h| h.epoch == epoch);
                match (opened, current) {
                    (Ok(Ok(host)), Some(hosting)) => {
                        hosting.host = Some(host.clone());
                        let invited = hosting.invited.clone();
                        s.send_activity();
                        cx.notify();
                        Some((host, invited))
                    }
                    (Ok(Ok(host)), None) => {
                        runtime::spawn(async move { host.close().await });
                        None
                    }
                    (Ok(Err(e)), _) => {
                        moonglass::toast::show(
                            ToastKind::Error,
                            super::problem::play_error(&e),
                            cx,
                        );
                        if s.hosting.as_ref().is_some_and(|h| h.epoch == epoch) {
                            s.hosting = None;
                        }
                        cx.notify();
                        None
                    }
                    (Err(_), _) => None,
                }
            });
            let Ok(Some((host, invited))) = invited else {
                return;
            };
            let done = runtime::spawn(async move {
                for user in invited {
                    host.invite(&user).await?;
                }
                Ok(())
            });
            let _ = this.update(cx, |s, cx| s.report(done, cx));
            while let Some(event) = events.recv().await {
                let alive = this.update(cx, |s, cx| {
                    let name = |s: &Self, id: &str| {
                        s.friends
                            .as_ref()
                            .and_then(|f| f.friends.iter().find(|f| f.user.id == id))
                            .map(|f| f.user.shown().to_owned())
                            .unwrap_or_default()
                    };
                    match event {
                        HostEvent::Joined(user) => {
                            let shown = name(s, &user);
                            if let Some(h) = s.hosting.as_mut().filter(|h| h.epoch == epoch) {
                                h.joined.push(user);
                                moonglass::toast::show(
                                    ToastKind::Info,
                                    t!("friends.toast_joined", name = shown).to_string(),
                                    cx,
                                );
                                s.send_activity();
                            }
                        }
                        HostEvent::Left(user) => {
                            if let Some(h) = s.hosting.as_mut().filter(|h| h.epoch == epoch) {
                                h.joined.retain(|u| *u != user);
                                h.invited.retain(|u| *u != user);
                                s.send_activity();
                            }
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub fn stop_hosting(&mut self, cx: &mut Context<Self>) {
        if let Some(host) = self.hosting.take().and_then(|h| h.host) {
            let done = runtime::spawn(async move { host.close().await });
            self.report(done, cx);
        }
        self.send_activity();
        cx.notify();
    }

    /// Whether this player is in, or joining, the world `friend` opened to friends.
    pub fn in_world_of(&self, friend: &Friend) -> bool {
        let lobby = friend
            .presence
            .activity
            .as_ref()
            .and_then(|a| a.lobby.as_ref());
        self.joining
            .as_ref()
            .is_some_and(|j| lobby.is_some_and(|l| *l == j.lobby().id))
    }

    /// Joins the lobby a friend opened to all friends, from their activity.
    pub fn join_friend(&mut self, friend: &Friend, cx: &mut Context<Self>) {
        let Some(activity) = &friend.presence.activity else {
            return;
        };
        let Some(id) = activity.lobby.clone() else {
            return;
        };
        let lobby = Lobby {
            id,
            name: activity.name.clone(),
            host: friend.user.id.clone(),
            game: LobbyGame {
                minecraft: activity.minecraft.clone(),
                loader: None,
                pack: None,
            },
            members: Vec::new(),
            invited: Vec::new(),
            open: true,
            created_at: String::new(),
        };
        self.join(lobby, cx);
    }

    /// Drops the invites whose lobby closed or that were used while this launcher was offline.
    fn check_invites(&mut self, cx: &mut Context<Self>) {
        let Status::SignedIn(me) = &self.status else {
            return;
        };
        let (me, ids) = (
            me.id.clone(),
            self.invites
                .iter()
                .map(|(l, _)| l.id.clone())
                .collect::<Vec<_>>(),
        );
        if ids.is_empty() {
            return;
        }
        let checked = runtime::spawn(async move {
            let mut gone = Vec::new();
            for id in ids {
                match riven_play::lobby::get(&id).await {
                    Ok(lobby) if lobby.invited.contains(&me) => {}
                    Ok(_) | Err(PlayError::NotFound(_)) => gone.push(id),
                    Err(e) => tracing::debug!("play: cannot check invite {id}: {e}"),
                }
            }
            gone
        });
        cx.spawn(async move |this, cx| {
            let Ok(gone) = checked.await else {
                return;
            };
            let _ = this.update(cx, |s, cx| {
                s.invites.retain(|(l, _)| !gone.contains(&l.id));
                cx.notify();
            });
        })
        .detach();
    }

    /// Joins a lobby a friend invited this player to, leaving the one it is in.
    pub fn join(&mut self, lobby: Lobby, cx: &mut Context<Self>) {
        match &self.joining {
            // Leaving and joining the same lobby at once races the old tunnel's teardown on the host.
            Some(j) if j.lobby().id == lobby.id => return,
            Some(Joining::Connecting(_)) => return,
            Some(Joining::Joined { .. }) => self.leave(cx),
            None => {}
        }
        let Some(signaller) = self.connection.as_ref().map(|c| c.signaller()) else {
            return;
        };
        self.invites.retain(|(l, _)| l.id != lobby.id);
        self.fresh.retain(|l| *l != lobby.id);
        self.joining = Some(Joining::Connecting(lobby.clone()));
        let signals = self.signals.clone();
        let (lost_tx, lost) = tokio::sync::oneshot::channel();
        let id = lobby.id.clone();
        let joined = runtime::spawn(async move {
            Guest::join(&id, signaller, &signals, move |kicked| {
                let _ = lost_tx.send(kicked);
            })
            .await
            .map(Arc::new)
        });
        cx.spawn(async move |this, cx| {
            let joined = joined.await;
            let ok = this.update(cx, |s, cx| {
                let current = s.joining.as_ref().is_some_and(|j| j.lobby().id == lobby.id);
                let ok = match joined {
                    Ok(Ok(guest)) if current => {
                        s.joining = Some(Joining::Joined {
                            lobby: guest.lobby.clone(),
                            guest,
                        });
                        true
                    }
                    Ok(Ok(guest)) => {
                        runtime::spawn(async move { guest.leave().await });
                        false
                    }
                    Ok(Err(e)) => {
                        let text = match e {
                            PlayError::NotFound(_) => {
                                t!("friends.lobby_gone", name = lobby.name).to_string()
                            }
                            e => super::problem::play_error(&e),
                        };
                        moonglass::toast::show(ToastKind::Error, text, cx);
                        if current {
                            s.joining = None;
                        }
                        false
                    }
                    Err(_) => false,
                };
                cx.notify();
                ok
            });
            if !ok.unwrap_or(false) {
                return;
            }
            let Ok(kicked) = lost.await else {
                return;
            };
            let _ = this.update(cx, |s, cx| {
                if kicked {
                    s.kicked_out(&lobby.id, cx);
                } else if s.joining.as_ref().is_some_and(|j| j.lobby().id == lobby.id) {
                    s.joining = None;
                    moonglass::toast::show(
                        ToastKind::Warning,
                        t!("friends.toast_lost", name = lobby.name).to_string(),
                        cx,
                    );
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// The host sent this player out of `lobby_id`: the lobby no longer has them, so nothing is left to leave.
    fn kicked_out(&mut self, lobby_id: &str, cx: &mut Context<Self>) {
        if let Some(joining) = self.joining.take_if(|j| j.lobby().id == lobby_id) {
            moonglass::toast::show(
                ToastKind::Warning,
                t!("friends.toast_kicked", name = joining.lobby().name).to_string(),
                cx,
            );
            cx.notify();
        }
    }

    /// Sends a guest out of the world this player hosts, or takes their invite back.
    pub fn kick(&mut self, user_id: String, cx: &mut Context<Self>) {
        let Some(hosting) = &mut self.hosting else {
            return;
        };
        hosting.invited.retain(|u| *u != user_id);
        hosting.joined.retain(|u| *u != user_id);
        if let Some(host) = hosting.host.clone() {
            let done = runtime::spawn(async move { host.kick(&user_id).await.map(|_| ()) });
            self.report(done, cx);
        }
        self.send_activity();
        cx.notify();
    }

    pub fn leave(&mut self, cx: &mut Context<Self>) {
        if let Some(Joining::Joined { guest, .. }) = self.joining.take() {
            let done = runtime::spawn(async move { guest.leave().await });
            self.report(done, cx);
        }
        cx.notify();
    }

    /// Declines an invite: the host's "Invited" mark goes too.
    pub fn dismiss_invite(&mut self, lobby_id: &str, cx: &mut Context<Self>) {
        self.invites.retain(|(l, _)| l.id != lobby_id);
        self.fresh.retain(|l| l != lobby_id);
        let id = lobby_id.to_owned();
        let done = runtime::spawn(async move {
            match riven_play::lobby::leave(&id).await {
                // The host closed the world meanwhile; there is nothing left to decline.
                Err(PlayError::NotFound(_)) => Ok(()),
                other => other,
            }
        });
        self.report(done, cx);
        cx.notify();
    }

    /// Shows the error of background play work, if it fails.
    fn report(
        &self,
        done: tokio::sync::oneshot::Receiver<Result<(), PlayError>>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = done.await {
                let _ = this.update(cx, |_, cx| {
                    moonglass::toast::show(ToastKind::Error, super::problem::play_error(&e), cx)
                });
            }
        })
        .detach();
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        self.stop_hosting(cx);
        self.leave(cx);
        self.invites.clear();
        self.status = Status::SignedOut;
        self.friends = None;
        self.nicks = None;
        self.taken.clear();
        self.connected = false;
        self.connection = None;
        cx.notify();
    }

    /// Runs a friends action, then reads the list again.
    fn act<F, Fut>(&mut self, work: F, cx: &mut Context<Self>)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), PlayError>> + Send + 'static,
    {
        let done = runtime::spawn(async move { work().await });
        cx.spawn(async move |this, cx| {
            let result = done.await;
            let _ = this.update(cx, |s, cx| {
                if let Ok(Err(e)) = result {
                    s.error = Some(super::problem::play_error(&e).into());
                }
                s.refresh(cx);
            });
        })
        .detach();
    }

    pub fn accept(&mut self, user_id: String, cx: &mut Context<Self>) {
        self.act(move || async move { friends::accept(&user_id).await }, cx);
    }

    pub fn remove(&mut self, user_id: String, cx: &mut Context<Self>) {
        self.act(move || async move { friends::remove(&user_id).await }, cx);
    }
}

struct GlobalFriends(Entity<FriendsStack>);

impl Global for GlobalFriends {}

/// Whether the friends panel is open, where requests and invites already show.
fn panel_open(cx: &App) -> bool {
    cx.try_global::<GlobalFriends>()
        .and_then(|g| g.0.read(cx).panel.read(cx).popover.as_ref()?.upgrade())
        .is_some_and(|p| p.read(cx).is_open())
}

/// The friends list that opens above the faces in the launch bar; one per window,
/// so the add field keeps its text while it is closed.
pub struct FriendsPanel {
    /// A BX ID username; Enter or the check button sends a friend request.
    username: Entity<InputState>,
    note: Option<(bool, SharedString)>,
    adding: bool,
    add_open: bool,
    status_open: bool,
    offline_open: bool,
    sent_open: bool,
    /// The popover the panel shows in, once it has been opened.
    popover: Option<gpui::WeakEntity<gpui_base::PopoverState>>,
    _username: Subscription,
    _play: Subscription,
}

const PANEL_WIDTH: f32 = 360.;
const FOOTER_HEIGHT: f32 = 56.;

impl FriendsPanel {
    pub fn init(window: &mut Window, cx: &mut App) {
        let panel = cx.new(|cx| Self::new(window, cx));
        let stack = cx.new(|cx| FriendsStack {
            panel,
            _play: cx.observe(&PlayState::global(cx), |_, _, cx| cx.notify()),
            _app: cx.observe(&AppState::global(cx), |_, _, cx| cx.notify()),
        });
        cx.set_global(GlobalFriends(stack));
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let username =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("friends.add_placeholder")));
        let _username = cx.subscribe_in(&username, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.add(window, cx);
            }
        });
        Self {
            username,
            note: None,
            adding: false,
            add_open: false,
            status_open: false,
            offline_open: false,
            sent_open: false,
            popover: None,
            _username,
            _play: cx.observe(&PlayState::global(cx), |_, _, cx| cx.notify()),
        }
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.username.read(cx).value().trim().to_string();
        if name.is_empty() || self.adding {
            return;
        }
        self.adding = true;
        self.note = None;
        let added = runtime::spawn(async move { friends::add(&name).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = added.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.adding = false;
                if matches!(result, Ok(Ok(_))) {
                    this.username
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
                // A request that went through shows as a row in the list, so the field just closes.
                match result {
                    Ok(Ok(added)) => match added.status {
                        AddStatus::Requested | AddStatus::Accepted => {
                            this.add_open = false;
                            this.sent_open |= added.status == AddStatus::Requested;
                        }
                        AddStatus::Exists => {
                            let name = added.user.shown().to_owned();
                            this.note = Some((false, t!("friends.already", name = name).into()));
                        }
                    },
                    Ok(Err(e)) => this.note = Some((true, super::problem::play_error(&e).into())),
                    Err(_) => return,
                }
                PlayState::global(cx).update(cx, |s, cx| s.refresh(cx));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors;
        h_flex()
            .px(px(14.))
            .h(px(46.))
            .border_b_1()
            .border_color(c.border)
            .font_weight(W_SEMIBOLD)
            .child(t!("friends.title").to_string())
    }

    fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_open = !self.add_open;
        self.status_open = false;
        self.note = None;
        if self.add_open {
            self.username
                .update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn close_add(&mut self, cx: &mut Context<Self>) {
        self.add_open = false;
        self.note = None;
        cx.notify();
    }

    /// The friend code field above the footer, where this player's own code is; its answer under it.
    /// Over an empty list it floats, so the centered hint does not move when it opens.
    fn add_row(&self, floating: bool, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.add_open {
            return None;
        }
        let c = cx.theme().colors;
        Some(
            v_flex()
                .flex_none()
                .when(floating, |d| d.absolute().bottom_0().left_0().right_0())
                .bg(c.panel)
                .border_t_1()
                .border_color(c.border)
                .children(self.add_note(cx))
                .child(
                    h_flex()
                        .gap(px(4.))
                        .pl(px(8.))
                        .pr(px(10.))
                        .h(px(46.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                // Escape closes the field instead of the whole popover.
                                .on_action(cx.listener(
                                    |this, _: &gpui_base::actions::Cancel, _, cx| {
                                        this.close_add(cx)
                                    },
                                ))
                                .child(TextField::new(&self.username)),
                        )
                        .child(
                            Button::new("friends-add")
                                .primary()
                                .size(ButtonSize::Xs)
                                .icon(IconName::Check)
                                .loading(self.adding)
                                .tooltip(t!("friends.add"))
                                .disabled(self.adding)
                                .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                        )
                        .child(
                            Button::new("friends-add-close")
                                .ghost()
                                .size(ButtonSize::Xs)
                                .icon(IconName::Close)
                                .tooltip(t!("common.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| this.close_add(cx))),
                        ),
                ),
        )
    }

    /// A clear layer under the status menu: a click anywhere else, in the panel or outside it, closes the menu.
    fn status_dismiss(&self, cx: &mut Context<Self>) -> gpui::Div {
        let close =
            |this: &mut Self, _: &gpui::MouseDownEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.status_open = false;
                cx.notify();
            };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(close))
            .on_mouse_down_out(cx.listener(close))
    }

    /// The three statuses, floating above the footer; a popover inside the popover would close it.
    fn status_menu(&self, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors;
        let current = PlayState::global(cx).read(cx).visibility;
        v_flex()
            .absolute()
            // Without this, clicks and hovers fall through to the rows under the menu.
            .occlude()
            .left(px(8.))
            .bottom(px(FOOTER_HEIGHT + 4.))
            .w(px(200.))
            .p(px(4.))
            .rounded(px(10.))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .shadow(super::dialogs::popup_shadow(cx))
            .children(
                [Visibility::Online, Visibility::Busy, Visibility::Invisible].map(|v| {
                    row()
                        .id(SharedString::from(format!("status-{v:?}")))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.sel.opacity(0.5)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.status_open = false;
                            PlayState::global(cx).update(cx, |s, cx| s.set_visibility(v, cx));
                            cx.notify();
                        }))
                        .child(status_dot(visibility_color(v, cx), c.panel))
                        .child(div().flex_1().text_size(px(13.)).child(visibility_label(v)))
                        .when(v == current, |r| {
                            r.child(icon(IconName::Check, c.ok).size(px(13.)))
                        })
                }),
            )
    }

    /// Why the last friend request did not go through, above the field so the field stays put.
    fn add_note(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let c = cx.theme().colors;
        let (bad, note) = self.note.clone()?;
        Some(div().px(px(8.)).pt(px(8.)).map(|d| {
            if bad {
                d.child(super::problem::inline(note, cx))
            } else {
                d.px(px(14.))
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .line_height(relative(1.4))
                    .child(note)
            }
        }))
    }

    /// This player: head, name and status, which opens the status menu, and the ID a friend types to add them.
    fn footer(&self, me: &Me, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors;
        let st = PlayState::global(cx).read(cx);
        let shown = if st.connected {
            st.visibility
        } else {
            Visibility::Invisible
        };
        let name = me.shown().to_owned();
        let status = if st.connected {
            visibility_label(st.visibility)
        } else {
            t!("friends.connecting").into()
        };
        let row = h_flex()
            .gap(px(8.))
            .px(px(10.))
            .h(px(FOOTER_HEIGHT))
            .border_t_1()
            .border_color(c.border)
            .child(
                h_flex()
                    .id("my-status")
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.))
                    .px(px(4.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c.sel.opacity(0.5)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.status_open = !this.status_open;
                        cx.notify();
                    }))
                    .child(avatar(
                        &name,
                        me.image.as_deref(),
                        Some(visibility_color(shown, cx)),
                        32.,
                        cx,
                    ))
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(W_SEMIBOLD)
                                    .truncate()
                                    .child(name.clone()),
                            )
                            .child(
                                h_flex()
                                    .gap(px(3.))
                                    .text_size(px(11.))
                                    .text_color(visibility_color(shown, cx))
                                    .child(status)
                                    .child(
                                        icon(
                                            if self.status_open {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronUp
                                            },
                                            visibility_color(shown, cx),
                                        )
                                        .size(px(11.)),
                                    ),
                            ),
                    ),
            );
        match me.shown_code() {
            Some(id) => row.child({
                let label = t!("friends.your_id", code = id).to_string();
                let view = cx.entity_id();
                let copied = moonglass::copy::copied("friends-my-id", cx);
                h_flex()
                    .id("friends-my-id")
                    .flex_none()
                    .gap(px(5.))
                    .px(px(4.))
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(c.text))
                    .tooltip(|window, cx| {
                        moonglass::tooltip(t!("friends.copy_id").into())(window, cx)
                    })
                    .on_click(move |_, _, cx| {
                        moonglass::copy::copy("friends-my-id".into(), id.clone(), view, cx);
                    })
                    .child(label)
                    .child(if copied {
                        icon(IconName::Check, c.ok).size(px(11.))
                    } else {
                        icon(IconName::Copy, c.muted).size(px(11.))
                    })
            }),
            None => row,
        }
        .child(
            Button::new("friends-add-toggle")
                .ghost()
                .size(ButtonSize::Xs)
                .icon(super::assets::USER_PLUS)
                .tooltip(t!("friends.add"))
                .on_click(cx.listener(|this, _, window, cx| this.open_add(window, cx))),
        )
    }

    fn signed_out(&self, checking: bool, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors;
        v_flex()
            .gap(px(10.))
            .p(px(14.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(c.muted)
                    .line_height(relative(1.5))
                    .child(t!("friends.signed_out_hint").to_string()),
            )
            .child(
                Button::new("friends-sign-in")
                    .primary()
                    .size(ButtonSize::Md)
                    .w_full()
                    .label(t!("bxid.sign_in"))
                    .disabled(checking)
                    .on_click(|_, _, cx| open_sign_in(None, cx)),
            )
    }

    /// Friend requests and invites to answer, as slim bars above the list.
    fn banners(&self, friends: &Friends, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let st = PlayState::global(cx).read(cx);
        let busy = matches!(st.joining, Some(Joining::Connecting(_)));
        let mut bars: Vec<AnyElement> = friends
            .incoming
            .iter()
            .map(|user| {
                let (accept, decline) = (user.id.clone(), user.id.clone());
                banner(
                    user,
                    t!("friends.asks_you", name = user.shown()).into(),
                    vec![
                        Button::new(SharedString::from(format!("accept-{}", user.id)))
                            .ghost()
                            .size(ButtonSize::Xs)
                            .icon(IconName::Check)
                            .tooltip(t!("friends.accept"))
                            .on_click(move |_, _, cx| {
                                let id = accept.clone();
                                PlayState::global(cx).update(cx, |s, cx| s.accept(id, cx));
                            })
                            .into_any_element(),
                        Button::new(SharedString::from(format!("decline-{}", user.id)))
                            .ghost()
                            .size(ButtonSize::Xs)
                            .icon(IconName::Close)
                            .tooltip(t!("friends.decline"))
                            .on_click(move |_, _, cx| {
                                let id = decline.clone();
                                PlayState::global(cx).update(cx, |s, cx| s.remove(id, cx));
                            })
                            .into_any_element(),
                    ],
                    cx,
                )
            })
            .collect();
        for (lobby, from) in &st.invites {
            let (join, dismiss) = (lobby.clone(), lobby.id.clone());
            bars.push(banner(
                from,
                t!(
                    "friends.invites_you",
                    name = from.shown(),
                    world = lobby.name
                )
                .into(),
                vec![
                    Button::new(SharedString::from(format!("panel-join-{}", lobby.id)))
                        .primary()
                        .size(ButtonSize::Xs)
                        .label(t!("friends.join"))
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            let lobby = join.clone();
                            PlayState::global(cx).update(cx, |s, cx| s.join(lobby, cx));
                        })
                        .into_any_element(),
                    Button::new(SharedString::from(format!("panel-decline-{}", lobby.id)))
                        .ghost()
                        .size(ButtonSize::Xs)
                        .icon(IconName::Close)
                        .tooltip(t!("friends.decline"))
                        .on_click(move |_, _, cx| {
                            let id = dismiss.clone();
                            PlayState::global(cx).update(cx, |s, cx| s.dismiss_invite(&id, cx));
                        })
                        .into_any_element(),
                ],
                cx,
            ));
        }
        bars
    }

    /// The empty list: what friends give and the way to the friend code field.
    fn no_friends(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .px(px(24.))
            .py(px(24.))
            .child(icon(super::assets::USER_PLUS, c.text2).size(px(28.)))
            .child(
                v_flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .font_weight(W_SEMIBOLD)
                            .child(t!("friends.none").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(c.muted)
                            .text_center()
                            .line_height(relative(1.5))
                            .child(t!("friends.none_hint").to_string()),
                    ),
            )
            // Hidden rather than removed, so the text above keeps its place.
            .child(
                div().when(self.add_open, |d| d.invisible()).child(
                    Button::new("friends-empty-add")
                        .primary()
                        .size(ButtonSize::Sm)
                        .icon(super::assets::USER_PLUS)
                        .label(t!("friends.add_friend"))
                        .disabled(self.add_open)
                        .on_click(cx.listener(|this, _, window, cx| this.open_add(window, cx))),
                ),
            )
            .into_any_element()
    }

    /// Friends grouped by what one can do with them: join those playing, call those in the launcher.
    fn lists(&self, friends: &Friends, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut sorted = friends.friends.clone();
        sorted.sort_by_key(|f| f.user.shown().to_lowercase());
        let (playing, rest): (Vec<_>, Vec<_>) = sorted
            .into_iter()
            .partition(|f| f.presence.online && f.presence.activity.is_some());
        let (online, offline): (Vec<_>, Vec<_>) = rest.into_iter().partition(|f| f.presence.online);
        let mut rows = Vec::new();
        if friends.friends.is_empty() {
            rows.push(self.no_friends(cx));
        }
        if !playing.is_empty() {
            rows.push(group_caption(
                t!("friends.group_playing", n = playing.len()).into(),
                cx,
            ));
            for friend in &playing {
                rows.push(self.friend_row(friend, true, cx));
            }
        }
        if !online.is_empty() {
            rows.push(group_caption(
                t!("friends.group_launcher", n = online.len()).into(),
                cx,
            ));
            for friend in &online {
                rows.push(self.friend_row(friend, false, cx));
            }
        }
        if !offline.is_empty() {
            let open = self.offline_open;
            rows.push(
                fold_heading(
                    "friends-offline",
                    t!("friends.group_offline", n = offline.len()).into(),
                    open,
                    cx,
                )
                .pt(px(10.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.offline_open = !this.offline_open;
                    cx.notify();
                }))
                .into_any_element(),
            );
            if open {
                for friend in &offline {
                    rows.push(self.friend_row(friend, false, cx));
                }
            }
        }
        if !friends.outgoing.is_empty() {
            let open = self.sent_open;
            rows.push(
                fold_heading(
                    "friends-sent",
                    t!("friends.sent_n", n = friends.outgoing.len()).into(),
                    open,
                    cx,
                )
                .pt(px(10.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.sent_open = !this.sent_open;
                    cx.notify();
                }))
                .into_any_element(),
            );
            if open {
                for (i, user) in friends.outgoing.iter().enumerate() {
                    let id = user.id.clone();
                    rows.push(
                        person_row(user, None, t!("friends.waiting").into(), cx)
                            .child(
                                Button::new(SharedString::from(format!("cancel-{i}")))
                                    .ghost()
                                    .size(ButtonSize::Xs)
                                    .icon(IconName::Close)
                                    .tooltip(t!("friends.cancel"))
                                    .on_click(move |_, _, cx| {
                                        let id = id.clone();
                                        PlayState::global(cx).update(cx, |s, cx| s.remove(id, cx));
                                    }),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        rows
    }

    /// A friend: head, name and what they do, with Join or Invite and, on hover, Remove.
    /// For one `playing` the line under the name is their world; Invite shows only on hover until sent.
    fn friend_row(&self, friend: &Friend, playing: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let st = PlayState::global(cx).read(cx);
        let id = friend.user.id.clone();
        let group = SharedString::from(format!("friend-row-{id}"));
        let on_hover = |d: gpui::Div| d.invisible().group_hover(group.clone(), |s| s.visible());
        let action = friend_action(friend, st)
            .map(|(button, always)| div().flex_none().when(!always, on_hover).child(button));
        let (remove_id, name) = (id.clone(), friend.user.shown().to_owned());
        let remove = on_hover(div().flex_none()).child(
            Button::new(SharedString::from(format!("remove-{id}")))
                .ghost()
                .size(ButtonSize::Xs)
                .icon(IconName::Trash)
                .tooltip(t!("friends.remove"))
                .on_click(move |_, _, cx| confirm_remove(remove_id.clone(), name.clone(), cx)),
        );
        let sub = match friend.presence.activity.as_ref().filter(|_| playing) {
            Some(_) if st.in_world_of(friend) => div()
                .text_size(px(11.))
                .text_color(c.ok)
                .truncate()
                .child(t!("friends.in_their_world").to_string())
                .into_any_element(),
            Some(a) => world_line(a, cx),
            None => div()
                .text_size(px(11.))
                .text_color(c.muted)
                .truncate()
                .child(presence_line(friend))
                .into_any_element(),
        };
        row()
            .group(group)
            .min_h(px(48.))
            .rounded(px(8.))
            .hover(|s| s.bg(c.sel.opacity(0.5)))
            .child(person_with(
                &friend.user,
                Some(presence_color(&friend.presence, cx)),
                sub,
                32.,
                cx,
            ))
            .children(action)
            .child(remove)
            .into_any_element()
    }
}

impl Render for FriendsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let st = PlayState::global(cx).read(cx);
        let me = st.me().cloned();
        let checking = st.status == Status::Checking;
        let friends = st.friends.clone().unwrap_or_default();
        let error = st.error.clone();
        // The launch bar and the window chrome take about this much; the body gets the rest.
        // A fixed height keeps the popover still when a group folds or a request comes in.
        let body = (f32::from(window.viewport_size().height) - 300.).clamp(200., 420.);

        v_flex()
            .ui_text(cx)
            .w(px(PANEL_WIDTH))
            .rounded(px(12.))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .text_color(c.text)
            .relative()
            .shadow(super::dialogs::popup_shadow(cx))
            .child(self.header(cx))
            .map(|d| match &me {
                None => d.child(self.signed_out(checking, cx)),
                Some(me) => {
                    let banners = self.banners(&friends, cx);
                    let rows = self.lists(&friends, cx);
                    let floating = friends.friends.is_empty();
                    d.child(
                        v_flex()
                            .relative()
                            .h(px(body))
                            .child(
                                v_flex()
                                    .id("friends-list")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .when(!banners.is_empty(), |d| {
                                        d.child(
                                            v_flex()
                                                .gap(px(4.))
                                                .px(px(8.))
                                                .pt(px(8.))
                                                .children(banners),
                                        )
                                    })
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .px(px(6.))
                                            .pb(px(6.))
                                            .gap(px(1.))
                                            .children(rows),
                                    ),
                            )
                            .children(self.add_row(floating, cx)),
                    )
                    .child(self.footer(me, cx))
                    .when(self.status_open, |d| {
                        d.child(self.status_dismiss(cx)).child(self.status_menu(cx))
                    })
                }
            })
            .when_some(error, |d, error| {
                d.child(
                    div()
                        .p(px(8.))
                        .border_t_1()
                        .border_color(c.border)
                        .child(super::problem::inline(error, cx)),
                )
            })
    }
}

/// The friends button in the launch bar with the online count; a view of its own so it updates on presence.
pub struct FriendsStack {
    panel: Entity<FriendsPanel>,
    _play: Subscription,
    _app: Subscription,
}

/// The friends in the launch bar, while BX ID is on.
pub fn friends_stack(cx: &App) -> Option<Entity<FriendsStack>> {
    PlayState::enabled(cx).then(|| cx.global::<GlobalFriends>().0.clone())
}

impl Render for FriendsStack {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let st = PlayState::global(cx).read(cx);
        let online = st
            .friends
            .iter()
            .flat_map(|f| &f.friends)
            .filter(|f| f.presence.online)
            .count();
        let news = st.invites.len() + st.friends.as_ref().map_or(0, |f| f.incoming.len());
        let open_world = st.friends.iter().flat_map(|f| &f.friends).any(|f| {
            f.presence
                .activity
                .as_ref()
                .is_some_and(|a| a.lobby.is_some())
                && !st.in_world_of(f)
        });
        let trigger = h_flex()
            .id("friends-stack")
            .h(px(40.))
            .px(px(12.))
            .gap(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(c.border)
            .cursor_pointer()
            .hover(|s| s.bg(c.row))
            .child(
                div()
                    .relative()
                    .child(icon(super::assets::USERS, c.text2).size(px(16.)))
                    .when(open_world, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(-3.))
                                .right(px(-4.))
                                .child(status_dot(c.ok, c.panel).size(px(9.))),
                        )
                    }),
            )
            .when(online > 0, |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .font_weight(W_SEMIBOLD)
                        .text_color(c.ok)
                        .child(online.to_string()),
                )
            })
            .when(news > 0, |d| {
                d.child(
                    div()
                        .min_w(px(16.))
                        .h(px(16.))
                        .px(px(4.))
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(c.accent)
                        .text_color(c.on_accent)
                        .text_size(px(10.))
                        .font_weight(W_SEMIBOLD)
                        .child(news.to_string()),
                )
            })
            .tooltip(move |window, cx| {
                let text = if open_world {
                    t!("friends.world_to_join")
                } else {
                    t!("friends.title")
                };
                moonglass::tooltip(text.into())(window, cx)
            });
        let panel = self.panel.clone();
        Popover::new("friends-popover")
            .anchor(Anchor::BottomRight)
            .offset(px(10.))
            // `trigger` takes only Selectable elements such as Button; the stack is a plain div.
            .trigger_with(move |_, _, _| trigger.into_any_element())
            .content(move |_, window, cx| {
                let popover = cx.entity().downgrade();
                panel.update(cx, |p, _| p.popover = Some(popover));
                // A dialog opened from the panel paints under it, so the panel steps aside.
                if AppState::global(cx).read(cx).modal.is_some() {
                    cx.defer_in(window, |popover, window, cx| popover.dismiss(window, cx));
                }
                moonglass::motion::enter("friends-in", div().child(panel.clone()), 10., window, cx)
            })
    }
}

/// Who may walk into the world this player opened to LAN.
#[derive(Clone, Copy, PartialEq)]
enum Access {
    /// No lobby: the world stays on this network.
    Me,
    Invited,
    Friends,
}

impl Access {
    const ALL: [Self; 3] = [Self::Me, Self::Invited, Self::Friends];

    fn of(st: &PlayState) -> Self {
        match &st.hosting {
            None => Self::Me,
            Some(h) if h.open => Self::Friends,
            Some(_) => Self::Invited,
        }
    }

    fn label(self) -> SharedString {
        match self {
            Self::Me => t!("friends.access.me"),
            Self::Invited => t!("friends.access.invited"),
            Self::Friends => t!("friends.access.friends"),
        }
        .into()
    }

    fn hint(self) -> SharedString {
        match self {
            Self::Me => t!("friends.access.me_hint"),
            Self::Invited => t!("friends.access.invited_hint"),
            Self::Friends => t!("friends.access.friends_hint"),
        }
        .into()
    }

    fn icon(self) -> IconName {
        match self {
            Self::Me => IconName::Lock,
            Self::Invited => super::assets::MAIL,
            Self::Friends => super::assets::USERS,
        }
    }

    fn color(self, cx: &App) -> gpui::Hsla {
        let c = cx.theme().colors;
        match self {
            Self::Me => c.text2,
            Self::Invited => c.accent,
            Self::Friends => c.ok,
        }
    }

    fn set(self, cx: &mut App) {
        PlayState::global(cx).update(cx, |s, cx| match self {
            Self::Me => s.stop_hosting(cx),
            Self::Invited => s.set_open(false, cx),
            Self::Friends => s.set_open(true, cx),
        });
    }
}

/// The friends in this player's world, as far as the friends list knows them.
fn guests(st: &PlayState) -> Vec<PlayUser> {
    let Some(hosting) = &st.hosting else {
        return Vec::new();
    };
    hosting
        .joined
        .iter()
        .filter_map(|id| friend_user(st, id))
        .collect()
}

fn friend_user(st: &PlayState, id: &str) -> Option<PlayUser> {
    st.friends
        .as_ref()?
        .friends
        .iter()
        .find(|f| f.user.id == id)
        .map(|f| f.user.clone())
}

/// Overlapping pictures of players, their names on hover.
fn heads(id: &str, users: &[PlayUser], size: f32, cx: &App) -> Option<AnyElement> {
    const SHOWN: usize = 3;
    if users.is_empty() {
        return None;
    }
    let c = cx.theme().colors;
    let names: SharedString = users
        .iter()
        .map(|u| u.shown())
        .collect::<Vec<_>>()
        .join(", ")
        .into();
    Some(
        h_flex()
            .id(SharedString::from(id.to_owned()))
            .children(users.iter().take(SHOWN).enumerate().map(|(i, user)| {
                div()
                    .when(i > 0, |d| d.ml(px(-size / 3.)))
                    .rounded(px(size / 4. + 2.))
                    .border_2()
                    .border_color(c.panel)
                    .child(
                        super::launch_bar::picture_tile(
                            user.shown(),
                            user.image.as_deref(),
                            size,
                            size / 4.,
                            cx,
                        )
                        .text_size(px(size * 0.45))
                        .text_color(c.accent),
                    )
            }))
            .when(users.len() > SHOWN, |d| {
                d.child(
                    div()
                        .ml(px(4.))
                        .text_size(px(11.))
                        .text_color(c.muted)
                        .child(format!("+{}", users.len() - SHOWN)),
                )
            })
            .tooltip(move |window, cx| moonglass::tooltip(names.clone())(window, cx))
            .into_any_element(),
    )
}

/// The pill in the launch bar: where this player's world or the friend's world stands, and who is in.
fn pill(
    glyph: IconName,
    label: SharedString,
    color: gpui::Hsla,
    players: Option<AnyElement>,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let spins = glyph == IconName::Loader;
    let glyph = icon(glyph, color).size(px(14.));
    let glyph = if spins {
        moonglass::motion::spinner(glyph, cx)
    } else {
        glyph.into_any_element()
    };
    h_flex()
        .id("session-pill")
        .flex_none()
        .h(px(32.))
        .pl(px(10.))
        .pr(px(if players.is_some() { 4. } else { 8. }))
        .gap(px(6.))
        .rounded(px(16.))
        .border_1()
        .border_color(color.opacity(0.35))
        .bg(color.opacity(0.1))
        .cursor_pointer()
        .hover(|s| s.bg(color.opacity(0.18)))
        .child(glyph)
        .child(
            div()
                .text_size(px(12.))
                .font_weight(W_SEMIBOLD)
                .text_color(color)
                .whitespace_nowrap()
                .child(label),
        )
        .child(icon(IconName::ChevronDown, color).size(px(12.)))
        .when_some(players, |d, players| {
            d.child(div().w(px(1.)).h(px(16.)).mx(px(2.)).bg(color.opacity(0.3)))
                .child(players)
        })
}

fn menu_frame(cx: &App) -> gpui::Div {
    let c = cx.theme().colors;
    v_flex()
        .ui_text(cx)
        .w(px(280.))
        .p(px(6.))
        .gap(px(1.))
        .rounded(px(12.))
        .border_1()
        .border_color(c.border)
        .bg(c.panel)
        .text_color(c.text)
        .shadow(super::dialogs::popup_shadow(cx))
}

fn menu_caption(text: SharedString, cx: &App) -> gpui::Div {
    div()
        .px(px(8.))
        .pt(px(6.))
        .pb(px(4.))
        .text_size(px(11.))
        .text_color(cx.theme().colors.muted)
        .child(text)
}

fn menu_divider(cx: &App) -> gpui::Div {
    div()
        .h(px(1.))
        .mx(px(4.))
        .my(px(4.))
        .bg(cx.theme().colors.border)
}

/// A row of a session menu; a click closes the menu, then acts.
#[allow(clippy::too_many_arguments)]
fn menu_item(
    id: &'static str,
    glyph: IconName,
    label: SharedString,
    hint: Option<SharedString>,
    color: Option<gpui::Hsla>,
    checked: bool,
    popover: Entity<gpui_base::PopoverState>,
    act: impl Fn(&mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let c = cx.theme().colors;
    h_flex()
        .id(id)
        .gap(px(10.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|s| s.bg(c.sel.opacity(0.5)))
        .on_click(move |_, window, cx| {
            popover.update(cx, |p, cx| p.dismiss(window, cx));
            act(cx);
        })
        .child(icon(glyph, color.unwrap_or(c.text2)).size(px(15.)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(color.filter(|_| hint.is_none()).unwrap_or(c.text))
                        .child(label),
                )
                .when_some(hint, |d, hint| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(c.muted)
                            .line_height(relative(1.35))
                            .child(hint),
                    )
                }),
        )
        .when(checked, |d| {
            d.child(icon(IconName::Check, c.ok).size(px(14.)))
        })
        .into_any_element()
}

/// Who may join this player's world, and who is in it.
fn access_menu(popover: Entity<gpui_base::PopoverState>, cx: &App) -> gpui::Div {
    let st = PlayState::global(cx).read(cx);
    let current = Access::of(st);
    let players = guests(st);
    let c = cx.theme().colors;
    menu_frame(cx)
        .child(menu_caption(t!("friends.access.title").into(), cx))
        .children(Access::ALL.map(|access| {
            let id = match access {
                Access::Me => "access-me",
                Access::Invited => "access-invited",
                Access::Friends => "access-friends",
            };
            menu_item(
                id,
                access.icon(),
                access.label(),
                Some(access.hint()),
                Some(access.color(cx)),
                access == current,
                popover.clone(),
                move |cx| access.set(cx),
                cx,
            )
        }))
        .when(!players.is_empty(), |d| {
            d.child(menu_divider(cx))
                .child(menu_caption(
                    t!("friends.in_world", n = players.len()).into(),
                    cx,
                ))
                // A lobby takes up to 15 guests; past about five the list scrolls inside the menu.
                .child(
                    v_flex()
                        .id("world-players")
                        .max_h(px(200.))
                        .overflow_y_scroll()
                        .children(players.iter().map(|user| {
                            let (id, name) = (user.id.clone(), user.shown().to_owned());
                            let popover = popover.clone();
                            row()
                                .child(person(
                                    user,
                                    Some(c.ok),
                                    t!("friends.in_your_world").into(),
                                    cx,
                                ))
                                .child(
                                    Button::new(SharedString::from(format!("world-kick-{id}")))
                                        .ghost()
                                        .size(ButtonSize::Xs)
                                        .icon(IconName::Close)
                                        .tooltip(t!("friends.kick"))
                                        .on_click(move |_, window, cx| {
                                            popover.update(cx, |p, cx| p.dismiss(window, cx));
                                            confirm_kick(id.clone(), name.clone(), cx);
                                        }),
                                )
                        })),
                )
        })
}

/// The friend's world this player is in: copying its address and leaving.
fn guest_menu(
    lobby: &Lobby,
    host: Option<PlayUser>,
    address: String,
    popover: Entity<gpui_base::PopoverState>,
    cx: &App,
) -> gpui::Div {
    let sub: SharedString = match &host {
        Some(host) => format!("{} · Minecraft {}", host.shown(), lobby.game.minecraft),
        None => format!("Minecraft {}", lobby.game.minecraft),
    }
    .into();
    let copy = address.clone();
    menu_frame(cx)
        .child(
            v_flex()
                .px(px(8.))
                .pt(px(6.))
                .pb(px(6.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(W_SEMIBOLD)
                        .truncate()
                        .child(lobby.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(cx.theme().colors.muted)
                        .truncate()
                        .child(sub),
                ),
        )
        .child(menu_divider(cx))
        .child(menu_item(
            "session-copy",
            IconName::Copy,
            t!("friends.copy_address").into(),
            Some(t!("friends.copy_address_hint", address = address).into()),
            None,
            false,
            popover.clone(),
            move |cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()));
                moonglass::toast::show(ToastKind::Info, t!("friends.copied").to_string(), cx);
            },
            cx,
        ))
        .child(menu_item(
            "session-leave",
            IconName::Power,
            t!("friends.leave").into(),
            None,
            Some(moonglass::theme::danger()),
            false,
            popover,
            |cx| PlayState::global(cx).update(cx, |s, cx| s.leave(cx)),
            cx,
        ))
}

/// A pill that opens `menu` above it.
fn pill_menu(
    trigger: gpui::Stateful<gpui::Div>,
    menu: impl Fn(Entity<gpui_base::PopoverState>, &App) -> gpui::Div + 'static,
) -> AnyElement {
    // The popover wraps its trigger in a block that fills its parent; a flex row keeps the pill its own size.
    h_flex()
        .justify_center()
        .child(
            Popover::new("session-popover")
                .anchor(Anchor::BottomCenter)
                .offset(px(10.))
                // `trigger` takes only Selectable elements such as Button; the pill is a plain div.
                .trigger_with(move |_, _, _| trigger.into_any_element())
                .content(move |_, window, cx| {
                    let panel = menu(cx.entity(), cx);
                    moonglass::motion::enter("session-in", panel, 10., window, cx)
                }),
        )
        .into_any_element()
}

/// What the launch bar shows in place of the play time while friends are involved:
/// the world this player opened to LAN and who may join it, or the friend's world they are in.
pub fn session_status(instance: &str, cx: &App) -> Option<AnyElement> {
    if !PlayState::enabled(cx) {
        return None;
    }
    let c = cx.theme().colors;
    let st = PlayState::global(cx).read(cx);
    if let Some(joining) = &st.joining {
        return Some(match joining {
            Joining::Connecting(lobby) => h_flex()
                .gap(px(8.))
                .min_w_0()
                .child(moonglass::motion::spinner(
                    icon(IconName::Loader, c.muted).size(px(14.)),
                    cx,
                ))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(c.muted)
                        .truncate()
                        .child(t!("friends.connecting_to", name = lobby.name).to_string()),
                )
                .into_any_element(),
            Joining::Joined { lobby, guest } => {
                let host = friend_user(st, &lobby.host);
                let players = heads("session-heads", host.as_slice(), 18., cx);
                let trigger = pill(
                    IconName::Globe,
                    lobby.name.clone().into(),
                    c.ok,
                    players,
                    cx,
                );
                let (lobby, address) = (lobby.clone(), format!("127.0.0.1:{}", guest.port));
                pill_menu(trigger, move |popover, cx| {
                    guest_menu(&lobby, host.clone(), address.clone(), popover, cx)
                })
            }
        });
    }
    let hosting = st.hosting.as_ref().filter(|h| h.instance == instance);
    let lan_here = st.lan.as_ref().is_some_and(|(id, _)| id == instance);
    if st.me().is_none() || !(hosting.is_some() || (st.hosting.is_none() && lan_here)) {
        return None;
    }
    let access = Access::of(st);
    let opening = hosting.is_some_and(|h| h.host.is_none());
    let players = heads("session-heads", &guests(st), 18., cx);
    let trigger = pill(
        if opening {
            IconName::Loader
        } else {
            access.icon()
        },
        access.label(),
        access.color(cx),
        players,
        cx,
    )
    .tooltip(|window, cx| moonglass::tooltip(t!("friends.access.title").into())(window, cx));
    Some(pill_menu(trigger, access_menu))
}

/// Height of an invite in the corner; fixed, so the notifications above it know where to start.
const INVITE_HEIGHT: f32 = 92.;

/// Fresh invites in the bottom right corner, framed like the notifications stacked above them,
/// and how much room they take there.
pub fn invite_cards(window: &mut Window, cx: &mut App) -> (Option<AnyElement>, f32) {
    if !PlayState::enabled(cx) || panel_open(cx) {
        return (None, 0.);
    }
    let st = PlayState::global(cx).read(cx);
    let busy = matches!(st.joining, Some(Joining::Connecting(_)));
    let fresh: Vec<(Lobby, PlayUser)> = st
        .invites
        .iter()
        .filter(|(l, _)| st.fresh.contains(&l.id))
        .cloned()
        .collect();
    if fresh.is_empty() {
        return (None, 0.);
    }
    let room = fresh.len() as f32 * (INVITE_HEIGHT + 8.);
    let cards: Vec<AnyElement> = fresh
        .iter()
        .map(|(lobby, from)| {
            let card = invite_toast(lobby, from, busy, cx);
            moonglass::motion::enter(
                SharedString::from(format!("invite-{}", lobby.id)),
                card,
                -6.,
                window,
                cx,
            )
        })
        .collect();
    (
        Some(
            v_flex()
                .gap(px(8.))
                .items_end()
                .children(cards)
                .into_any_element(),
        ),
        room,
    )
}

/// An invite in the frame of a notification: the friend's picture where the icon goes, Join and Decline under the text.
fn invite_toast(lobby: &Lobby, from: &PlayUser, busy: bool, cx: &App) -> gpui::Div {
    let c = cx.theme().colors;
    let (join, decline, hide) = (lobby.clone(), lobby.id.clone(), lobby.id.clone());
    h_flex()
        .w(px(360.))
        .h(px(INVITE_HEIGHT))
        .items_start()
        .gap(px(10.))
        .pl(px(12.))
        .pr(px(6.))
        .py(px(6.))
        .rounded(px(10.))
        .border_1()
        .border_color(c.border)
        .bg(c.panel)
        .shadow(super::dialogs::popup_shadow(cx))
        .child(h_flex().h(px(28.)).flex_none().child(avatar(
            from.shown(),
            from.image.as_deref(),
            None,
            20.,
            cx,
        )))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div().pt(px(4.)).truncate().child(
                        t!(
                            "friends.invites_you",
                            name = from.shown(),
                            world = lobby.name
                        )
                        .to_string(),
                    ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(c.muted)
                        .truncate()
                        .child(format!("Minecraft {}", lobby.game.minecraft)),
                )
                .child(
                    h_flex()
                        .mt(px(8.))
                        .gap(px(6.))
                        .child(
                            Button::new(SharedString::from(format!("corner-join-{}", lobby.id)))
                                .primary()
                                .size(ButtonSize::Xs)
                                .label(t!("friends.join"))
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    let lobby = join.clone();
                                    PlayState::global(cx).update(cx, |s, cx| s.join(lobby, cx));
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("corner-decline-{}", lobby.id)))
                                .ghost()
                                .size(ButtonSize::Xs)
                                .label(t!("friends.decline"))
                                .on_click(move |_, _, cx| {
                                    let id = decline.clone();
                                    PlayState::global(cx)
                                        .update(cx, |s, cx| s.dismiss_invite(&id, cx));
                                }),
                        ),
                ),
        )
        .child(
            Button::new(SharedString::from(format!("corner-hide-{}", lobby.id)))
                .ghost()
                .size(ButtonSize::Xs)
                .icon(IconName::Close)
                .tooltip(t!("friends.hide_invite"))
                .on_click(move |_, _, cx| {
                    let id = hide.clone();
                    PlayState::global(cx).update(cx, |s, cx| {
                        s.fresh.retain(|l| *l != id);
                        cx.notify();
                    });
                }),
        )
}

/// Join when the friend opened their world, Invite when this player's world is open to LAN;
/// the flag tells whether the button stays shown or waits for a hover.
fn friend_action(friend: &Friend, st: &PlayState) -> Option<(Button, bool)> {
    let id = friend.user.id.clone();
    let joinable = friend
        .presence
        .activity
        .as_ref()
        .is_some_and(|a| a.lobby.is_some());
    if joinable && st.in_world_of(friend) {
        return Some((
            Button::new(SharedString::from(format!("leave-{id}")))
                .ghost()
                .size(ButtonSize::Xs)
                .label(t!("friends.leave"))
                .on_click(|_, _, cx| PlayState::global(cx).update(cx, |s, cx| s.leave(cx))),
            true,
        ));
    }
    if joinable {
        let target = friend.clone();
        return Some((
            Button::new(SharedString::from(format!("join-{id}")))
                .outline()
                .size(ButtonSize::Xs)
                .label(t!("friends.join"))
                .disabled(matches!(st.joining, Some(Joining::Connecting(_))))
                .on_click(move |_, _, cx| {
                    let target = target.clone();
                    PlayState::global(cx).update(cx, |s, cx| s.join_friend(&target, cx));
                }),
            true,
        ));
    }
    let (invited, joined) = st.hosting.as_ref().map_or((false, false), |h| {
        (h.invited.contains(&id), h.joined.contains(&id))
    });
    if joined {
        let name = friend.user.shown().to_owned();
        return Some((
            Button::new(SharedString::from(format!("kick-{id}")))
                .ghost()
                .size(ButtonSize::Xs)
                .label(t!("friends.kick_short"))
                .tooltip(t!("friends.kick"))
                .on_click(move |_, _, cx| confirm_kick(id.clone(), name.clone(), cx)),
            false,
        ));
    }
    if st.lan.is_none() || !friend.presence.online {
        return None;
    }
    Some((
        Button::new(SharedString::from(format!("invite-{id}")))
            .ghost()
            .size(ButtonSize::Xs)
            .label(if invited {
                t!("friends.invited")
            } else {
                t!("friends.invite")
            })
            .disabled(invited)
            .on_click(move |_, _, cx| {
                let id = id.clone();
                PlayState::global(cx).update(cx, |s, cx| s.invite(id, cx));
            }),
        invited,
    ))
}

/// The world a friend plays: its name and version, and how many are in when it is open.
fn world_line(activity: &Activity, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    h_flex()
        .min_w_0()
        .gap(px(4.))
        .text_size(px(11.))
        .text_color(c.muted)
        .child(div().min_w_0().truncate().child(
            // An instance is often named after its version, "Minecraft 26.3"; say the version once.
            if activity.name.contains(&activity.minecraft) {
                activity.name.clone()
            } else {
                format!("{} · {}", activity.name, activity.minecraft)
            },
        ))
        .when(activity.players > 0, |d| {
            d.child("·")
                .child(icon(super::assets::USERS, c.muted).size(px(11.)))
                .child(activity.players.to_string())
        })
        .into_any_element()
}

/// A muted title over a group of friends.
fn group_caption(text: SharedString, cx: &App) -> AnyElement {
    div()
        .px(px(6.))
        .pt(px(10.))
        .pb(px(2.))
        .text_size(px(11.))
        .text_color(cx.theme().colors.muted)
        .child(text)
        .into_any_element()
}

/// A slim bar above the friends: who asks what, and the answers.
fn banner(user: &PlayUser, text: SharedString, actions: Vec<AnyElement>, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    h_flex()
        .gap(px(8.))
        .pl(px(8.))
        .pr(px(4.))
        .py(px(4.))
        .rounded(px(8.))
        .border_1()
        .border_color(c.accent.opacity(0.25))
        .bg(c.accent.opacity(0.1))
        .child(avatar(user.shown(), user.image.as_deref(), None, 22., cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .truncate()
                .child(text),
        )
        .children(actions)
        .into_any_element()
}

/// A muted group title that folds the rows under it.
fn fold_heading(
    id: &'static str,
    text: SharedString,
    open: bool,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let c = cx.theme().colors;
    h_flex()
        .id(id)
        .gap(px(4.))
        .px(px(6.))
        .pt(px(6.))
        .pb(px(2.))
        .cursor_pointer()
        .text_size(px(11.))
        .text_color(c.muted)
        .child(text)
        .child(
            icon(
                if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                c.muted,
            )
            .size(px(11.)),
        )
}

fn status_dot(color: gpui::Hsla, ring: gpui::Hsla) -> gpui::Div {
    div()
        .size(px(10.))
        .flex_none()
        .rounded(px(5.))
        .border_2()
        .border_color(ring)
        .bg(color)
}

/// A player's picture, or their initials, with a status dot in the corner.
fn avatar(
    name: &str,
    picture: Option<&str>,
    dot: Option<gpui::Hsla>,
    size: f32,
    cx: &App,
) -> gpui::Div {
    let c = cx.theme().colors;
    div()
        .relative()
        .flex_none()
        .child(
            super::launch_bar::picture_tile(name, picture, size, size / 4., cx)
                .text_color(c.accent),
        )
        .when_some(dot, |d, color| {
            d.child(
                div()
                    .absolute()
                    .right(px(-2.))
                    .bottom(px(-2.))
                    .child(status_dot(color, c.panel).size(px(11.)).rounded(px(6.))),
            )
        })
}

fn row() -> gpui::Div {
    h_flex().gap(px(8.)).px(px(6.)).py(px(4.)).rounded(px(6.))
}

/// One person, then buttons the caller adds on the right.
fn person_row(user: &PlayUser, dot: Option<gpui::Hsla>, sub: SharedString, cx: &App) -> gpui::Div {
    row().child(person(user, dot, sub, cx))
}

/// A head, a name and a one-line status, taking the row's free width.
fn person(user: &PlayUser, dot: Option<gpui::Hsla>, sub: SharedString, cx: &App) -> gpui::Div {
    let c = cx.theme().colors;
    let sub = div()
        .text_size(px(11.))
        .text_color(c.muted)
        .truncate()
        .child(sub)
        .into_any_element();
    person_with(user, dot, sub, 28., cx)
}

fn person_with(
    user: &PlayUser,
    dot: Option<gpui::Hsla>,
    sub: AnyElement,
    size: f32,
    cx: &App,
) -> gpui::Div {
    let c = cx.theme().colors;
    h_flex()
        .flex_1()
        .min_w_0()
        .gap(px(8.))
        .child(avatar(user.shown(), user.image.as_deref(), dot, size, cx))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .min_w_0()
                        .gap(px(5.))
                        .items_baseline()
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(13.))
                                .font_weight(W_SEMIBOLD)
                                .truncate()
                                .child(user.shown().to_owned()),
                        )
                        .when_some(user.handle(), |d, handle| {
                            d.child(
                                div()
                                    .min_w_0()
                                    .text_size(px(11.))
                                    .text_color(c.muted)
                                    .truncate()
                                    .child(handle),
                            )
                        }),
                )
                .child(sub),
        )
}

fn visibility_label(v: Visibility) -> SharedString {
    match v {
        Visibility::Online => t!("friends.status.online"),
        Visibility::Busy => t!("friends.status.busy"),
        Visibility::Invisible => t!("friends.status.invisible"),
    }
    .into()
}

fn visibility_color(v: Visibility, cx: &App) -> gpui::Hsla {
    let c = cx.theme().colors;
    match v {
        Visibility::Online => c.ok,
        Visibility::Busy => moonglass::theme::danger(),
        Visibility::Invisible => c.muted,
    }
}

fn presence_color(p: &riven_play::Presence, cx: &App) -> gpui::Hsla {
    match (p.online, p.status) {
        (false, _) => cx.theme().colors.muted,
        (true, status) => visibility_color(status, cx),
    }
}

fn last_seen(p: &riven_play::Presence) -> Option<String> {
    let secs = riven_launch::parse_rfc3339(p.last_seen.as_deref()?)?;
    Some(super::time::since(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
    ))
}

fn presence_line(friend: &Friend) -> SharedString {
    let p = &friend.presence;
    match (&p.online, &p.activity) {
        (false, _) => match last_seen(p) {
            Some(time) => t!("friends.last_seen", time = time),
            None => t!("friends.offline"),
        },
        (true, Some(a)) => t!("friends.playing", name = a.name, version = a.minecraft),
        (true, None) if p.status == Visibility::Busy => t!("friends.status.busy"),
        (true, None) => t!("friends.online"),
    }
    .into()
}

fn confirm_kick(id: String, name: String, cx: &mut App) {
    super::dialogs::confirm(
        t!("friends.kick_title", name = name),
        t!("friends.kick_body"),
        t!("friends.kick"),
        move |_, cx| {
            let id = id.clone();
            PlayState::global(cx).update(cx, |s, cx| s.kick(id, cx));
        },
        cx,
    )
}

fn confirm_remove(id: String, name: String, cx: &mut App) {
    super::dialogs::confirm(
        t!("friends.remove_title", name = name),
        t!("friends.remove_body"),
        t!("friends.remove"),
        move |_, cx| {
            let id = id.clone();
            PlayState::global(cx).update(cx, |s, cx| s.remove(id, cx));
        },
        cx,
    )
}

/// The BX ID card on the Accounts settings page: who is signed in.
pub fn accounts_card(cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    let play = PlayState::global(cx).read(cx);
    let Some(me) = play.me().cloned() else {
        return Section::new()
            .row(
                h_flex()
                    .gap(px(12.))
                    .px(px(18.))
                    .py(px(14.))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(W_SEMIBOLD).child("BX ID"))
                            .child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(12.))
                                    .text_color(c.muted)
                                    .line_height(relative(1.45))
                                    .child(t!("bxid.pitch").to_string()),
                            ),
                    )
                    .child(
                        Button::new("bxid-sign-in")
                            .primary()
                            .label(t!("bxid.sign_in"))
                            .disabled(play.status == Status::Checking)
                            .on_click(|_, _, cx| open_sign_in(None, cx)),
                    ),
            )
            .into_any_element();
    };
    Section::new()
        .row(
            h_flex()
                .gap(px(12.))
                .px(px(18.))
                .py(px(12.))
                .child(
                    super::launch_bar::picture_tile(me.shown(), me.image.as_deref(), 36., 18., cx)
                        .text_color(c.accent),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(W_SEMIBOLD).child(me.shown().to_owned()))
                        .child(
                            div()
                                .mt(px(2.))
                                .text_size(px(12.))
                                .text_color(c.muted)
                                .child(match me.handle() {
                                    Some(handle) => format!("{handle} · {}", me.email),
                                    None => format!("BX ID · {}", me.email),
                                }),
                        ),
                )
                .child(
                    Button::new("bxid-sign-out")
                        .ghost()
                        .label(t!("bxid.sign_out"))
                        .on_click(|_, _, cx| {
                            PlayState::global(cx).update(cx, |s, cx| s.sign_out(cx))
                        }),
                ),
        )
        .into_any_element()
}

/// "Sign in with BX ID": shows the code, opens bxteam.org, waits for the approval there.
pub struct SignIn {
    code: Option<DeviceCode>,
    error: Option<(SharedString, SharedString)>,
    back: Option<Back>,
    _task: Option<gpui::Task<()>>,
    polling: Option<runtime::Abort>,
}

impl Drop for SignIn {
    // The poll runs on tokio, which outlives this dialog unless told to stop.
    fn drop(&mut self) {
        if let Some(polling) = self.polling.take() {
            polling.abort();
        }
    }
}

impl SignIn {
    fn new(back: Option<Back>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            code: None,
            error: None,
            back,
            _task: None,
            polling: None,
        };
        this.start(cx);
        this
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.code = None;
        self.error = None;
        if let Some(polling) = self.polling.take() {
            polling.abort();
        }
        let (code_tx, code_rx) = tokio::sync::oneshot::channel();
        let code_tx = std::sync::Mutex::new(Some(code_tx));
        let (signed_in, polling) = runtime::spawn_abortable(async move {
            id::sign_in(move |code| {
                if let Some(tx) = code_tx.lock().ok().and_then(|mut t| t.take()) {
                    let _ = tx.send(code);
                }
            })
            .await
        });
        self.polling = Some(polling);
        self._task = Some(cx.spawn(async move |this, cx| {
            if let Ok(code) = code_rx.await {
                let _ = this.update(cx, |this, cx| {
                    cx.open_url(&code.url);
                    this.code = Some(code);
                    cx.notify();
                });
            }
            let result = signed_in.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(me)) => {
                    PlayState::global(cx).update(cx, |s, cx| s.signed_in(me, cx));
                    leave(this.back.as_ref(), cx);
                }
                Ok(Err(e)) => {
                    tracing::warn!("{e}");
                    this.error = Some(bxid_failure(&e));
                    cx.notify();
                }
                Err(_) => {}
            });
        }));
        cx.notify();
    }
}

/// A failed BX ID sign-in in the player's language; anything unforeseen keeps its own text.
fn bxid_failure(e: &PlayError) -> (SharedString, SharedString) {
    let (title, hint) = match e {
        PlayError::Denied => (t!("bxid.failed.denied"), t!("bxid.failed.denied_hint")),
        PlayError::Expired => (t!("bxid.failed.expired"), t!("bxid.failed.expired_hint")),
        PlayError::Network(_) => (t!("bxid.failed.offline"), t!("bxid.failed.offline_hint")),
        e => (
            t!("bxid.failed.other"),
            super::problem::play_error(e).into(),
        ),
    };
    (title.into(), hint.into())
}

impl Render for SignIn {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let waiting = |text: String, cx: &mut Context<Self>| {
            h_flex()
                .gap(px(10.))
                .text_color(c.muted)
                .child(moonglass::motion::spinner(
                    moonglass::icon(IconName::Loader, c.muted),
                    cx,
                ))
                .child(text)
        };
        let body = match (&self.error, &self.code) {
            (Some(error), _) => super::problem::card(error, cx),
            (None, None) => waiting(t!("bxid.requesting").to_string(), cx).into_any_element(),
            (None, Some(code)) => {
                let url = code.url.clone();
                v_flex()
                    .gap(px(12.))
                    .child(super::dialogs::device_code(
                        "bxid-copy",
                        code.code.clone(),
                        cx,
                    ))
                    .child(
                        Button::new("bxid-open")
                            .outline()
                            .size(ButtonSize::Md)
                            .label(t!("bxid.open_again"))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .child(waiting(t!("bxid.waiting").to_string(), cx))
                    .into_any_element()
            }
        };
        let retry = Button::new("bxid-retry")
            .primary()
            .size(ButtonSize::Md)
            .label(t!("accounts.ms_retry"))
            .disabled(self.error.is_none())
            .on_click(cx.listener(|this, _, _, cx| this.start(cx)));
        dialog_frame(
            t!("bxid.sign_in"),
            if self.error.is_none() && self.code.is_some() {
                t!("bxid.hint")
            } else {
                t!("bxid.pitch")
            },
            body,
            retry,
            self.back.clone(),
            cx,
        )
    }
}

pub fn open_sign_in(back: Option<Back>, cx: &mut App) {
    let form = cx.new(|cx| SignIn::new(back, cx));
    super::dialogs::open(form.into(), 420., cx);
}
