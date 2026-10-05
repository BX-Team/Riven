use std::collections::BTreeMap;
use std::time::Duration;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div, px, relative,
};
use riven_format::{Loader, LoaderKind, Release};
use riven_sources::Cache;
use riven_sync::update::{self, Request};
use rust_i18n::t;

use super::runtime;
use super::state::{AppState, Route};
use super::theme::ActiveTheme as _;
use super::ui::{
    Button, ButtonSize, Dropdown, MenuItem, Switch, Tabs, TextField, h_flex, tile, v_flex,
};

const LOADERS: [Option<LoaderKind>; 5] = [
    None,
    Some(LoaderKind::NeoForge),
    Some(LoaderKind::Fabric),
    Some(LoaderKind::Quilt),
    Some(LoaderKind::Forge),
];

fn loader_label(kind: Option<LoaderKind>) -> SharedString {
    match kind {
        None => t!("instance.vanilla").into(),
        Some(kind) => super::launch_bar::loader_display(kind).into(),
    }
}

fn game_meta() -> riven_sources::GameMeta {
    let meta = riven_sources::GameMeta::new(riven_sources::client());
    match riven_sync::data_dir() {
        Some(dir) => meta.with_cache(Cache::new(
            dir.join("cache").join("api"),
            Duration::from_secs(3600),
        )),
        None => meta,
    }
}

/// The form behind "New instance": an empty game of a chosen version, or a pack from its link.
pub struct NewInstance {
    from_pack: bool,
    name: Entity<InputState>,
    search: Entity<InputState>,
    releases: Vec<SharedString>,
    minecraft: Option<SharedString>,
    loader: usize,
    link: Entity<InputState>,
    /// The release the link points at, once checked.
    release: Option<Release>,
    groups: BTreeMap<String, bool>,
    error: Option<SharedString>,
    busy: bool,
    _subs: Vec<Subscription>,
}

impl NewInstance {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("new_instance.name_hint").to_string())
        });
        name.update(cx, |s, cx| s.focus(window, cx));
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("new_instance.find_version").to_string())
        });
        let link = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("new_instance.link_hint").to_string())
        });
        let subs = vec![
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.subscribe(&link, |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.release = None;
                    this.error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.check(cx),
                _ => {}
            }),
        ];
        cx.spawn_in(window, async move |this, cx| {
            let releases = runtime::spawn(async { game_meta().minecraft_releases().await })
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.releases = releases.into_iter().map(Into::into).collect();
                this.minecraft = this.releases.first().cloned();
                if this.releases.is_empty() {
                    this.error = Some(t!("new_instance.offline").into());
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            from_pack: false,
            name,
            search,
            releases: Vec::new(),
            minecraft: None,
            loader: 0,
            link,
            release: None,
            groups: BTreeMap::new(),
            error: None,
            busy: false,
            _subs: subs,
        }
    }

    /// Reads the release behind the pack link to show it before installing.
    fn check(&mut self, cx: &mut Context<Self>) {
        let link = self.link.read(cx).value().trim().to_string();
        if link.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = runtime::spawn(async move {
                let store = riven_sync::Store::default_location()
                    .ok_or_else(|| t!("new_instance.no_data_dir").to_string())?;
                update::preview(&store, &riven_sources::client(), &link, None)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|_| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match found {
                    Ok(release) => {
                        this.groups = release
                            .groups
                            .iter()
                            .map(|g| (g.id.clone(), g.default))
                            .collect();
                        this.release = Some(release);
                    }
                    Err(e) => this.error = Some(e.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Creates an instance for the checked pack and starts installing it into the launch bar.
    fn install(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self.release.clone() else {
            return self.check(cx);
        };
        let state = AppState::global(cx);
        let Some(store) = state.read(cx).store.clone() else {
            return;
        };
        let name = self.name.read(cx).value().trim().to_string();
        let name = if name.is_empty() {
            release.name.clone()
        } else {
            name
        };
        let id = match store.create(&name, &release.minecraft, Some(release.loader.clone())) {
            Ok(id) => id,
            Err(e) => {
                self.error = Some(e.to_string().into());
                cx.notify();
                return;
            }
        };
        let request = Request {
            source: Some(self.link.read(cx).value().trim().to_string()),
            groups: self
                .groups
                .iter()
                .map(|(g, on)| format!("{}{g}", if *on { "+" } else { "-" }))
                .collect(),
            ..Request::default()
        };
        state.update(cx, |s, cx| {
            s.reload_instances(cx);
            s.navigate(Route::Instance(id.clone()), cx);
            s.close_modal(cx);
            s.install_pack(&id, request, cx);
        });
    }

    fn render_pack(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let view = cx.entity().downgrade();
        v_flex()
            .gap(px(14.))
            .child(field(
                t!("new_instance.link"),
                TextField::new(&self.link),
                cx,
            ))
            .when_some(self.release.as_ref(), |col, release| {
                let loader = format!(
                    "Minecraft {} · {} {}",
                    release.minecraft,
                    super::launch_bar::loader_display(release.loader.kind),
                    release.loader.version
                );
                let groups = release.groups.iter().enumerate().map(|(i, g)| {
                    let on = self.groups.get(&g.id).copied().unwrap_or(g.default);
                    let id = g.id.clone();
                    let view = view.clone();
                    h_flex()
                        .gap(px(12.))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().child(g.name.clone()))
                                .when_some(g.description.clone(), |col, d| {
                                    col.child(div().text_size(px(12.)).text_color(c.muted).child(d))
                                }),
                        )
                        .child(
                            Switch::new(SharedString::from(format!("group-{i}")), on)
                                .accessible(g.name.clone())
                                .on_change(move |on, _, cx| {
                                    let id = id.clone();
                                    let _ = view.update(cx, |this, cx| {
                                        this.groups.insert(id, on);
                                        cx.notify();
                                    });
                                }),
                        )
                });
                col.child(
                    v_flex()
                        .gap(px(12.))
                        .p(px(14.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(c.border)
                        .bg(c.bg)
                        .child(
                            h_flex()
                                .gap(px(12.))
                                .child(
                                    tile(super::launch_bar::initials(&release.name), 40., 8., cx)
                                        .text_color(c.accent),
                                )
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(
                                            h_flex()
                                                .gap(px(6.))
                                                .child(
                                                    div()
                                                        .font_weight(FontWeight::BOLD)
                                                        .truncate()
                                                        .child(release.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_color(c.muted)
                                                        .child(release.version.clone()),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .mt(px(2.))
                                                .text_size(px(12.))
                                                .text_color(c.muted)
                                                .child(format!(
                                                    "{loader} · {}",
                                                    t!(
                                                        "new_instance.files",
                                                        n = release.files.len()
                                                    )
                                                )),
                                        ),
                                ),
                        )
                        .when(!release.groups.is_empty(), |card| {
                            card.child(
                                v_flex()
                                    .gap(px(10.))
                                    .pt(px(12.))
                                    .border_t_1()
                                    .border_color(c.border)
                                    .children(groups),
                            )
                        }),
                )
            })
            .child(field(
                t!("new_instance.name"),
                TextField::new(&self.name),
                cx,
            ))
            .into_any_element()
    }

    /// Creates the instance in the background; the dialog stays open until it is done.
    fn create(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        let Some(minecraft) = self.minecraft.clone() else {
            self.error = Some(t!("new_instance.pick_version").into());
            cx.notify();
            return;
        };
        let loader = LOADERS[self.loader];
        let name = if name.is_empty() {
            match loader {
                Some(kind) => format!("{} {minecraft}", super::launch_bar::loader_display(kind)),
                None => format!("Minecraft {minecraft}"),
            }
        } else {
            name
        };
        let Some(store) = AppState::global(cx).read(cx).store.clone() else {
            return;
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let minecraft = minecraft.to_string();
        cx.spawn(async move |this, cx| {
            let created = runtime::spawn(async move {
                let loader = match loader {
                    Some(kind) => Some(Loader {
                        kind,
                        version: game_meta()
                            .loader_version(kind, &minecraft)
                            .await
                            .map_err(|e| e.to_string())?,
                    }),
                    None => None,
                };
                store
                    .create(&name, &minecraft, loader)
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|_| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| match created {
                Ok(id) => {
                    AppState::global(cx).update(cx, |s, cx| {
                        s.reload_instances(cx);
                        s.navigate(Route::Instance(id), cx);
                        s.close_modal(cx);
                    });
                }
                Err(e) => {
                    this.busy = false;
                    this.error = Some(e.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Render for NewInstance {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let modes = cx.entity().downgrade();
        let tabs = Tabs::new(
            "new-instance-mode",
            vec![
                t!("new_instance.empty").into(),
                t!("new_instance.from_pack").into(),
            ],
            usize::from(self.from_pack),
            move |ix, window, cx| {
                let _ = modes.update(cx, |this, cx| {
                    this.from_pack = ix == 1;
                    this.error = None;
                    if this.from_pack {
                        this.link.update(cx, |s, cx| s.focus(window, cx));
                    }
                    cx.notify();
                });
            },
        );
        let busy = self.busy;
        if self.from_pack {
            let ready = self.release.is_some();
            let body = v_flex()
                .gap(px(16.))
                .child(tabs)
                .child(self.render_pack(cx))
                .when_some(self.error.clone(), |this, error| {
                    this.child(div().text_color(cx.theme().colors.warn).child(error))
                });
            return dialog_frame(
                t!("new_instance.title"),
                t!("new_instance.pack_description"),
                body,
                Button::new("install")
                    .primary()
                    .size(ButtonSize::Md)
                    .disabled(busy)
                    .label(match (busy, ready) {
                        (true, _) => t!("new_instance.checking"),
                        (false, false) => t!("new_instance.check"),
                        (false, true) => t!("new_instance.install"),
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                cx,
            );
        }
        let view = cx.entity().downgrade();
        let pick = view.clone();
        let versions = self
            .releases
            .iter()
            .map(|r| MenuItem::new(r.clone(), r.clone()))
            .collect();
        let loaders = LOADERS
            .iter()
            .enumerate()
            .map(|(i, l)| MenuItem::new(i.to_string(), loader_label(*l)))
            .collect();
        let width = px(432.);
        let form = v_flex()
            .gap(px(14.))
            .child(tabs)
            .child(field(
                t!("new_instance.name"),
                TextField::new(&self.name),
                cx,
            ))
            .child(field(
                t!("new_instance.minecraft"),
                Dropdown::new(
                    "minecraft",
                    versions,
                    self.minecraft.clone(),
                    move |v, _, cx| {
                        let _ = pick.update(cx, |this, cx| {
                            this.minecraft = Some(v);
                            cx.notify();
                        });
                    },
                )
                .width(width)
                .placeholder(t!("new_instance.loading").to_string())
                .searchable(&self.search),
                cx,
            ))
            .child(field(
                t!("new_instance.loader"),
                Dropdown::new(
                    "loader",
                    loaders,
                    Some(self.loader.to_string().into()),
                    move |v, _, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.loader = v.parse().unwrap_or(0);
                            cx.notify();
                        });
                    },
                )
                .width(width),
                cx,
            ))
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_color(cx.theme().colors.warn).child(error))
            });
        dialog_frame(
            t!("new_instance.title"),
            t!("new_instance.description"),
            form,
            Button::new("create")
                .primary()
                .size(ButtonSize::Md)
                .disabled(busy)
                .label(if busy {
                    t!("new_instance.creating")
                } else {
                    t!("new_instance.create")
                })
                .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
            cx,
        )
    }
}

fn field(label: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(cx.theme().colors.muted)
                .child(label.into()),
        )
        .child(control)
}

/// What an account dialog opens when it is done, instead of closing: the first-run setup.
pub type Back = Rc<dyn Fn(&mut App)>;

fn leave(back: Option<&Back>, cx: &mut App) {
    match back {
        Some(back) => back(cx),
        None => AppState::global(cx).update(cx, |s, cx| s.close_modal(cx)),
    }
}

/// A dialog's title and description, its body, then Cancel and the main action.
fn dialog_frame(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    body: impl IntoElement,
    action: Button,
    back: Option<Back>,
    cx: &App,
) -> AnyElement {
    let c = cx.theme().colors;
    v_flex()
        .child(
            v_flex()
                .gap(px(4.))
                .px(px(20.))
                .pt(px(18.))
                .pb(px(14.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_color(c.muted)
                        .line_height(relative(1.5))
                        .child(description.into()),
                ),
        )
        .child(div().px(px(20.)).pb(px(18.)).child(body))
        .child(
            h_flex()
                .justify_end()
                .gap(px(8.))
                .px(px(20.))
                .py(px(14.))
                .border_t_1()
                .border_color(c.border)
                .child(
                    Button::new("cancel")
                        .outline()
                        .size(ButtonSize::Md)
                        .label(t!("common.cancel"))
                        .on_click(|_, _, cx| {
                            AppState::global(cx).update(cx, |s, cx| s.close_modal(cx))
                        }),
                )
                .child(action),
        )
        .into_any_element()
}

fn open(view: gpui_kit::AnyView, width: f32, cx: &mut App) {
    AppState::global(cx).update(cx, |s, cx| s.open_modal(view, px(width), cx));
}

pub fn open_new_instance(window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| NewInstance::new(window, cx));
    open(form.into(), 472., cx);
}

/// The form behind "Add offline account": just a player name.
pub struct AddOffline {
    name: Entity<InputState>,
    error: Option<SharedString>,
    back: Option<Back>,
    _name: Subscription,
}

impl AddOffline {
    fn new(back: Option<Back>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Steve"));
        name.update(cx, |s, cx| s.focus(window, cx));
        let _name = cx.subscribe(&name, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(cx);
            }
        });
        Self {
            name,
            error: None,
            back,
            _name,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        match riven_launch::accounts::offline(&name) {
            Ok(account) => {
                AppState::global(cx).update(cx, |s, cx| s.add_account(account, cx));
                leave(self.back.as_ref(), cx);
            }
            Err(_) => {
                self.error = Some(t!("accounts.bad_name").into());
                cx.notify();
            }
        }
    }
}

impl Render for AddOffline {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = v_flex()
            .gap(px(14.))
            .child(field(
                t!("accounts.player_name"),
                TextField::new(&self.name),
                cx,
            ))
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_color(cx.theme().colors.warn).child(error))
            });
        dialog_frame(
            t!("accounts.add_offline"),
            t!("accounts.offline_hint"),
            body,
            Button::new("add")
                .primary()
                .size(ButtonSize::Md)
                .label(t!("accounts.add"))
                .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
            self.back.clone(),
            cx,
        )
    }
}

pub fn open_add_offline(window: &mut Window, cx: &mut App) {
    open_add_offline_then(None, window, cx);
}

pub fn open_add_offline_then(back: Option<Back>, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| AddOffline::new(back, window, cx));
    open(form.into(), 400., cx);
}
