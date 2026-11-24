use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, Entity, Image, ImageFormat, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, img, px,
};
use gpui_base::PopoverState;
use moonglass::theme::ActiveTheme as _;
use moonglass::{IconName, UiText as _, h_flex, icon, motion, v_flex};
use riven_launch::game::{self, QuickPlay};
use riven_launch::worlds::{self, Server, World};
use riven_play::Friend;
use rust_i18n::t;

use super::play::PlayState;
use super::runtime;
use super::state::AppState;

/// How many worlds and servers the menu lists; the Worlds tab has the rest.
const SHOWN: usize = 4;
const ROW_ICON: f32 = 28.;
const WIDTH: f32 = 300.;
/// Opening the menu again within this reuses what was read.
const FRESH: Duration = Duration::from_secs(3);

/// A server of `servers.dat` with its icon ready to draw.
pub type ServerRow = (Server, Option<Arc<Image>>);

/// The game folder's servers, in the order the game lists them.
pub fn read_servers(instance_dir: &std::path::Path, game_dir: &std::path::Path) -> Vec<ServerRow> {
    worlds::servers(instance_dir, game_dir)
        .into_iter()
        .map(|server| {
            let icon = server
                .icon
                .clone()
                .map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)));
            (server, icon)
        })
        .collect()
}

struct Targets {
    worlds: Vec<World>,
    servers: Vec<ServerRow>,
    opens_worlds: Option<bool>,
}

fn read(instance_dir: PathBuf, game_dir: PathBuf, minecraft: String) -> Targets {
    let mut worlds = worlds::list_unsized(&game_dir);
    worlds.truncate(SHOWN);
    let mut servers = read_servers(&instance_dir, &game_dir);
    // Stable: servers never joined through Riven keep the game's order after the rest.
    servers.sort_by_key(|(s, _)| std::cmp::Reverse(s.joined));
    servers.truncate(SHOWN);
    Targets {
        worlds,
        servers,
        opens_worlds: game::opens_worlds(&minecraft),
    }
}

/// The menu behind the arrow on Play: the instance's latest worlds and its servers, each a launch.
#[derive(Default)]
pub struct QuickMenu {
    id: String,
    targets: Option<Targets>,
    read_at: Option<Instant>,
    loading: bool,
    pub popover: Option<Entity<PopoverState>>,
}

impl QuickMenu {
    /// Reads the instance's worlds and servers unless they were read a moment ago.
    pub fn refresh(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.loading || (self.id == id && self.read_at.is_some_and(|at| at.elapsed() < FRESH)) {
            return;
        }
        let st = AppState::global(cx).read(cx);
        let (Some(store), Some(instance)) = (st.store.as_ref(), st.instance(id)) else {
            return;
        };
        let (instance_dir, game_dir, minecraft) = (
            store.dir(id),
            store.game_dir(id),
            instance.minecraft.clone(),
        );
        if self.id != id {
            self.targets = None;
        }
        self.id = id.to_owned();
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let targets = runtime::blocking(move || read(instance_dir, game_dir, minecraft)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if let Ok(targets) = targets {
                    this.targets = Some(targets);
                    this.read_at = Some(Instant::now());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn dismiss(menu: &Entity<Self>, window: &mut Window, cx: &mut App) -> String {
        let (id, popover) = {
            let m = menu.read(cx);
            (m.id.clone(), m.popover.clone())
        };
        if let Some(popover) = popover {
            popover.update(cx, |p, cx| p.dismiss(window, cx));
        }
        id
    }

    fn go(menu: &Entity<Self>, target: QuickPlay, window: &mut Window, cx: &mut App) {
        let id = Self::dismiss(menu, window, cx);
        super::launch_bar::launch(&id, Some(target), window, cx);
    }
}

fn caption(text: String, cx: &App) -> gpui::Div {
    div()
        .px(px(8.))
        .pt(px(6.))
        .pb(px(2.))
        .text_size(px(11.))
        .text_color(cx.theme().colors.muted)
        .child(text)
}

fn picture(image: Option<gpui::ImageSource>, fallback: IconName, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    match image {
        Some(image) => img(image)
            .size(px(ROW_ICON))
            .flex_none()
            .rounded(px(6.))
            .into_any_element(),
        None => div()
            .size(px(ROW_ICON))
            .flex_none()
            .rounded(px(6.))
            .bg(c.sel)
            .flex()
            .items_center()
            .justify_center()
            .child(icon(fallback, c.muted).size(px(14.)))
            .into_any_element(),
    }
}

/// One launchable row: a picture, a name and a muted detail on the right.
fn row(
    key: SharedString,
    picture: AnyElement,
    name: String,
    detail: String,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let c = cx.theme().colors;
    let hover = motion::hover(SharedString::from(format!("{key}:hover")), window, cx);
    let bg = motion::animate(
        SharedString::from(format!("{key}:bg")),
        if hover.on { c.row } else { c.row.opacity(0.) },
        window,
        cx,
    );
    hover
        .track(h_flex().id(key))
        .flex_none()
        .h(px(40.))
        .px(px(6.))
        .gap(px(10.))
        .rounded(px(6.))
        .bg(bg)
        .cursor_pointer()
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(picture)
        .child(div().flex_1().min_w_0().truncate().child(name))
        .child(
            div()
                .flex_none()
                .max_w(px(120.))
                .truncate()
                .text_size(px(12.))
                .text_color(c.muted)
                .child(detail),
        )
        .into_any_element()
}

impl QuickMenu {
    /// Opening the world open to LAN to friends, and joining the worlds friends opened.
    fn friend_rows(
        &self,
        this: &Entity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if !PlayState::enabled(cx) {
            return Vec::new();
        }
        let st = PlayState::global(cx).read(cx);
        if st.me().is_none() {
            return Vec::new();
        }
        let glyph = |name: IconName, cx: &App| {
            div()
                .size(px(ROW_ICON))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(name, cx.theme().colors.accent).size(px(14.)))
                .into_any_element()
        };
        let joinable: Vec<Friend> = st
            .friends
            .iter()
            .flat_map(|f| &f.friends)
            .filter(|f| {
                f.presence
                    .activity
                    .as_ref()
                    .is_some_and(|a| a.lobby.is_some())
                    && !st.in_world_of(f)
            })
            .cloned()
            .collect();
        let lan_here = st.lan.as_ref().is_some_and(|(id, _)| *id == self.id);
        let hosting = st.hosting.as_ref().map(|h| h.open);
        let mut rows = Vec::new();
        if lan_here {
            // Each entry: the key, its icon, its label, and the lobby mode it sets (`None` closes).
            let entries = match hosting {
                None => vec![
                    (
                        "quick-open",
                        IconName::Network,
                        t!("quick.open_world"),
                        Some(true),
                    ),
                    (
                        "quick-private",
                        IconName::Lock,
                        t!("quick.open_private"),
                        Some(false),
                    ),
                ],
                Some(open) => vec![
                    if open {
                        (
                            "quick-private",
                            IconName::Lock,
                            t!("quick.make_private"),
                            Some(false),
                        )
                    } else {
                        (
                            "quick-open",
                            IconName::Network,
                            t!("quick.make_open"),
                            Some(true),
                        )
                    },
                    (
                        "quick-close",
                        IconName::Close,
                        t!("quick.close_world"),
                        None,
                    ),
                ],
            };
            for (key, name, label, mode) in entries {
                let menu = this.clone();
                rows.push(row(
                    key.into(),
                    glyph(name, cx),
                    label.to_string(),
                    String::new(),
                    move |window, cx| {
                        QuickMenu::dismiss(&menu, window, cx);
                        PlayState::global(cx).update(cx, |s, cx| match mode {
                            Some(open) => s.set_open(open, cx),
                            None => s.stop_hosting(cx),
                        });
                    },
                    window,
                    cx,
                ));
            }
        }
        for friend in joinable {
            let menu = this.clone();
            let version = friend
                .presence
                .activity
                .as_ref()
                .map(|a| a.minecraft.clone())
                .unwrap_or_default();
            rows.push(row(
                SharedString::from(format!("quick-join-{}", friend.user.id)),
                glyph(IconName::Play, cx),
                t!("quick.join_friend", name = friend.user.shown()).to_string(),
                version,
                move |window, cx| {
                    QuickMenu::dismiss(&menu, window, cx);
                    PlayState::global(cx).update(cx, |s, cx| s.join_friend(&friend, cx));
                },
                window,
                cx,
            ));
        }
        if !rows.is_empty() {
            rows.insert(
                0,
                caption(t!("quick.friends").to_string(), cx).into_any_element(),
            );
        }
        rows
    }
}

impl Render for QuickMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let this = cx.entity();
        let mut rows: Vec<AnyElement> = Vec::new();
        match &self.targets {
            None => rows.push(
                caption(t!("quick.loading").to_string(), cx)
                    .pb(px(6.))
                    .into_any_element(),
            ),
            Some(targets) => {
                if !targets.worlds.is_empty() {
                    rows.push(caption(t!("quick.worlds").to_string(), cx).into_any_element());
                    if targets.opens_worlds == Some(false) {
                        rows.push(
                            div()
                                .px(px(8.))
                                .pb(px(4.))
                                .text_size(px(12.))
                                .text_color(c.muted)
                                .child(t!("quick.worlds_unsupported").to_string())
                                .into_any_element(),
                        );
                    } else {
                        for world in &targets.worlds {
                            let target = QuickPlay::World(world.folder.clone());
                            let this = this.clone();
                            rows.push(row(
                                SharedString::from(format!("quick-world:{}", world.folder)),
                                picture(
                                    world.icon.clone().map(|p| super::sharp::image(p, ROW_ICON)),
                                    IconName::Globe,
                                    cx,
                                ),
                                world.name.clone(),
                                super::time::since(world.last_played),
                                move |window, cx| QuickMenu::go(&this, target.clone(), window, cx),
                                window,
                                cx,
                            ));
                        }
                    }
                }
                if !targets.servers.is_empty() {
                    rows.push(caption(t!("quick.servers").to_string(), cx).into_any_element());
                    for (n, (server, image)) in targets.servers.iter().enumerate() {
                        let target = QuickPlay::Server(server.address.clone());
                        let this = this.clone();
                        let detail = match server.joined {
                            Some(at) => super::time::since(at),
                            None if server.name == server.address => String::new(),
                            None => server.address.clone(),
                        };
                        rows.push(row(
                            SharedString::from(format!("quick-server:{n}")),
                            picture(image.clone().map(Into::into), IconName::Server, cx),
                            server.name.clone(),
                            detail,
                            move |window, cx| QuickMenu::go(&this, target.clone(), window, cx),
                            window,
                            cx,
                        ));
                    }
                }
                if targets.worlds.is_empty() && targets.servers.is_empty() {
                    rows.push(
                        div()
                            .px(px(8.))
                            .py(px(8.))
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .child(t!("quick.empty").to_string())
                            .into_any_element(),
                    );
                }
            }
        }
        rows.extend(self.friend_rows(&this, window, cx));
        let all = row(
            "quick-all".into(),
            div()
                .size(px(ROW_ICON))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconName::LayoutGrid, c.muted).size(px(14.)))
                .into_any_element(),
            t!("quick.all").to_string(),
            String::new(),
            move |window, cx| {
                let id = QuickMenu::dismiss(&this, window, cx);
                AppState::global(cx).update(cx, |s, cx| s.show_worlds(&id, cx));
            },
            window,
            cx,
        );
        v_flex()
            .ui_text(cx)
            .w(px(WIDTH))
            .p(px(4.))
            .gap(px(1.))
            .rounded(px(8.))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .shadow(super::dialogs::popup_shadow(cx))
            .children(rows)
            .child(div().my(px(4.)).mx(px(4.)).h(px(1.)).bg(c.border))
            .child(all)
    }
}
