use gpui_kit::base::Selectable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px, relative,
};
use rust_i18n::t;

use super::chrome;
use super::dev::DevView;
use super::instance::InstanceView;
use super::launch_bar;
use super::settings::SettingsView;
use super::state::{AppState, Route};
use super::theme::ActiveTheme as _;
use super::ui::{self, Button, IconName, UiText as _, W_SEMIBOLD, caption, h_flex, icon, v_flex};

pub const SIDEBAR_WIDTH: f32 = 240.;

pub struct RivenApp {
    state: Entity<AppState>,
    instance: Option<Entity<InstanceView>>,
    settings: Entity<SettingsView>,
    /// Created the first time the developer section opens.
    dev: Option<Entity<DevView>>,
    _state: Subscription,
}

impl RivenApp {
    pub fn view(window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::new(window, cx))
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        let _state = cx.observe_in(&state, window, |this, _, window, cx| {
            this.sync(window, cx);
            cx.notify();
        });
        let settings = cx.new(|cx| SettingsView::new(window, cx));
        let mut app = Self {
            state,
            instance: None,
            settings,
            dev: None,
            _state,
        };
        app.sync(window, cx);
        app
    }

    /// Keeps the instance view in step with the selected instance.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).route == Route::Developer && self.dev.is_none() {
            self.dev = Some(cx.new(|cx| DevView::new(window, cx)));
        }
        let state = self.state.read(cx);
        let Route::Instance(id) = &state.route else {
            return;
        };
        if self.instance.as_ref().is_some_and(|v| &v.read(cx).id == id) {
            return;
        }
        let Some(store) = state.store.clone() else {
            return;
        };
        let id = id.clone();
        let name: SharedString = state
            .instance(&id)
            .map(|i| i.name.clone())
            .unwrap_or_default()
            .into();
        let version = state.packs.get(&id).cloned().map(SharedString::from);
        self.instance = Some(cx.new(|cx| InstanceView::new(id, name, version, store, window, cx)));
    }

    fn navigate(state: &Entity<AppState>, route: Route, cx: &mut App) {
        state.update(cx, |s, cx| s.navigate(route, cx));
    }

    /// The developer section's button, shown in the title bar in developer mode.
    fn render_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        let developer = state.settings.developer_mode;
        let in_dev = state.route == Route::Developer;
        let dev_handle = self.state.clone();
        h_flex()
            .gap(px(6.))
            .when(developer, |row| {
                row.child(
                    Button::new("developer")
                        .icon(IconName::Code)
                        .selected(in_dev)
                        .tooltip(t!("titlebar.developer"))
                        .on_click(move |_, _, cx| {
                            let route = if in_dev {
                                dev_handle.read(cx).home()
                            } else {
                                Route::Developer
                            };
                            Self::navigate(&dev_handle, route, cx);
                        }),
                )
            })
            .into_any_element()
    }

    fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let state = self.state.read(cx);
        let route = state.route.clone();
        let entries: Vec<(String, String, bool)> = state
            .instances
            .iter()
            .map(|(id, i)| (id.clone(), i.name.clone(), state.packs.contains_key(id)))
            .collect();
        let selected = entries
            .iter()
            .position(|(id, ..)| route == Route::Instance(id.clone()));
        let mut rows: Vec<AnyElement> = entries
            .into_iter()
            .map(|(id, name, has_pack)| {
                let target = Route::Instance(id.clone());
                let active = route == target;
                let handle = self.state.clone();
                nav_row(
                    SharedString::from(format!("instance-{id}")),
                    active,
                    window,
                    cx,
                )
                .child(div().size(px(8.)).flex_none().rounded(px(4.)).map(|d| {
                    if has_pack {
                        d.bg(c.ok)
                    } else {
                        d.border_1().border_color(c.muted)
                    }
                }))
                .child(div().truncate().child(name))
                .on_click(move |_, _, cx| Self::navigate(&handle, target.clone(), cx))
                .into_any_element()
            })
            .collect();
        rows.push(
            nav_row("new-instance", false, window, cx)
                .text_color(c.muted)
                .child(format!("+ {}", t!("sidebar.new_instance")))
                .on_click(|_, window, cx| super::dialogs::open_new_instance(window, cx))
                .into_any_element(),
        );
        let list = ui::motion::highlighted(
            "sidebar",
            v_flex().gap(px(2.)),
            rows,
            selected,
            |d| d.rounded(px(6.)).bg(c.sel),
            window,
            cx,
        );
        let settings_handle = self.state.clone();
        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .bg(c.panel)
            .border_r_1()
            .border_color(c.border)
            .px(px(8.))
            .py(px(12.))
            .gap(px(2.))
            .child(caption(t!("sidebar.instances"), cx))
            .child(
                div()
                    .id("instances")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                nav_row("open-settings", false, window, cx)
                    .child(icon(IconName::Settings, c.text2))
                    .child(t!("sidebar.settings").to_string())
                    .on_click(move |_, _, cx| {
                        Self::navigate(&settings_handle, Route::Settings, cx)
                    }),
            )
    }

    fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.state.read(cx).route.clone() {
            Route::Instance(_) => match &self.instance {
                Some(view) => view.clone().cached(full()).into_any_element(),
                None => div().into_any_element(),
            },
            Route::Settings => self.settings.clone().cached(full()).into_any_element(),
            Route::Developer => match &self.dev {
                Some(view) => view.clone().cached(full()).into_any_element(),
                None => div().into_any_element(),
            },
            Route::Empty => placeholder(
                IconName::Package,
                t!("library.empty_title").into(),
                t!("library.empty_hint").into(),
                cx,
            )
            .child(
                Button::new("empty-new")
                    .primary()
                    .size(ui::ButtonSize::Md)
                    .icon(IconName::Plus)
                    .label(t!("sidebar.new_instance"))
                    .mt(px(8.))
                    .on_click(|_, window, cx| super::dialogs::open_new_instance(window, cx)),
            )
            .into_any_element(),
        }
    }
}

/// Screens are drawn again only when they change, not with every frame of a dialog over them.
fn full() -> gpui_kit::StyleRefinement {
    gpui_kit::StyleRefinement::default().size_full()
}

/// A 30 px list row of the left column; the selected one sits on a [`ui::motion::highlighted`] list.
pub fn nav_row(
    id: impl Into<gpui_kit::ElementId>,
    active: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let c = cx.theme().colors;
    let id: gpui_kit::ElementId = id.into();
    let key = format!("nav:{id}");
    let hover = ui::motion::hover(SharedString::from(format!("{key}:hover")), window, cx);
    let bg = ui::motion::animate(
        SharedString::from(format!("{key}:bg")),
        if hover.on && !active {
            c.row
        } else {
            c.row.opacity(0.)
        },
        window,
        cx,
    );
    hover
        .track(gpui_kit::base::Button::new(id))
        .selected(active)
        .justify_start()
        .h(px(30.))
        .px(px(8.))
        .gap(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        .bg(bg)
        .map(|b| {
            if active {
                b.text_color(c.text).font_weight(FontWeight::SEMIBOLD)
            } else {
                b.text_color(c.text2)
            }
        })
}

/// A centered empty state with an icon, a title and a hint.
pub fn placeholder(
    name: IconName,
    title: SharedString,
    hint: SharedString,
    cx: &App,
) -> gpui_kit::Div {
    let c = cx.theme().colors;
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .p(px(24.))
        .child(
            div()
                .size(px(44.))
                .mb(px(6.))
                .rounded(px(9.))
                .bg(c.sel)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(name, c.muted).size(px(20.))),
        )
        .child(
            div()
                .font_weight(W_SEMIBOLD)
                .text_size(px(15.))
                .child(title),
        )
        .child(
            div()
                .max_w(px(360.))
                .text_center()
                .text_color(c.muted)
                .line_height(relative(1.5))
                .child(hint),
        )
}

impl Render for RivenApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::theme::tick(window, cx);
        let c = cx.theme().colors;
        let st = self.state.read(cx);
        let route = st.route.clone();
        let modal = st.modal.clone();
        let bar = match &route {
            Route::Instance(id) => Some(launch_bar::render(&self.state, id, window, cx)),
            _ => None,
        };
        let actions = self.render_actions(cx);
        let title_bar = chrome::title_bar(actions, window, cx);
        let route_key = SharedString::from(format!("route:{route:?}"));
        let body = if matches!(route, Route::Settings | Route::Developer) {
            ui::motion::enter(
                route_key,
                div().size_full().child(self.render_content(cx)),
                8.,
                window,
                cx,
            )
        } else {
            h_flex()
                .size_full()
                .items_start()
                .child(self.render_sidebar(window, cx))
                .child(ui::motion::enter(
                    route_key,
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(self.render_content(cx)),
                    8.,
                    window,
                    cx,
                ))
                .into_any_element()
        };
        if self.state.read(cx).error.is_some() {
            let state = self.state.clone();
            cx.defer(move |cx| {
                state.update(cx, |s, cx| {
                    if let Some(e) = s.error.take() {
                        s.toast(super::toast::ToastKind::Error, e, cx);
                    }
                })
            });
        }
        let toast_bottom = if bar.is_some() { 88. } else { 16. };
        let toasts = super::toast::render(&self.state, toast_bottom, window, cx);
        let state = self.state.clone();
        let edges = chrome::resize_edges(window);
        v_flex()
            .relative()
            .size_full()
            .bg(c.bg)
            .ui_text(cx)
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(body))
            .children(bar)
            .children(edges)
            .when_some(modal, |root, m| {
                // The dialog is deferred above this layer; without it hover and scroll reach the screen.
                root.child(div().absolute().inset_0().occlude()).child(ui::modal(
                    &m,
                    move |_, cx| state.update(cx, |s, cx| s.close_modal(cx)),
                    window,
                    cx,
                ))
            })
            .child(toasts)
    }
}
