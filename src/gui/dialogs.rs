use std::time::Duration;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div, px, relative,
};
use riven_format::{Loader, LoaderKind};
use riven_sources::Cache;
use rust_i18n::t;

use super::runtime;
use super::state::{AppState, Route};
use super::theme::ActiveTheme as _;
use super::ui::{Button, ButtonSize, Dropdown, MenuItem, TextField, h_flex, v_flex};

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

/// The form behind "New instance": a name, a Minecraft release and a loader.
pub struct NewInstance {
    name: Entity<InputState>,
    search: Entity<InputState>,
    releases: Vec<SharedString>,
    minecraft: Option<SharedString>,
    loader: usize,
    error: Option<SharedString>,
    busy: bool,
    _search: Subscription,
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
        let _search = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
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
            name,
            search,
            releases: Vec::new(),
            minecraft: None,
            loader: 0,
            error: None,
            busy: false,
            _search,
        }
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
        let width = px(392.);
        let form = v_flex()
            .gap(px(14.))
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
        let busy = self.busy;
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
    open(form.into(), 432., cx);
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
