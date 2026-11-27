use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div, px, relative,
};
use gpui_base::input::{InputEvent, InputState};
use riven_format::LoaderKind;
use riven_sources::Cache;
use rust_i18n::t;

use super::runtime;
use super::state::AppState;
use moonglass::theme::ActiveTheme as _;
use moonglass::{Button, ButtonSize, TextField, h_flex, v_flex};

pub(super) fn loader_label(kind: Option<LoaderKind>) -> SharedString {
    match kind {
        None => t!("instance.vanilla").into(),
        Some(kind) => super::launch_bar::loader_display(kind).into(),
    }
}

pub(super) fn game_meta() -> riven_sources::GameMeta {
    let meta = riven_sources::GameMeta::new(riven_sources::client());
    match riven_sync::data_dir() {
        Some(dir) => meta.with_cache(Cache::new(
            dir.join("cache").join("api"),
            Duration::from_secs(3600),
        )),
        None => meta,
    }
}

pub(super) fn field(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
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

pub(super) fn leave(back: Option<&Back>, cx: &mut App) {
    match back {
        Some(back) => back(cx),
        None => AppState::global(cx).update(cx, |s, cx| s.close_modal(cx)),
    }
}

/// A dialog's title and description, its body, then Cancel and the main action.
pub(super) fn dialog_frame(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    body: impl IntoElement,
    action: Button,
    back: Option<Back>,
    cx: &App,
) -> AnyElement {
    let cancel = Button::new("cancel")
        .outline()
        .size(ButtonSize::Md)
        .label(t!("common.cancel"))
        .on_click(move |_, _, cx| leave(back.as_ref(), cx));
    dialog_shell(title, description, body, vec![cancel, action], cx)
}

/// A dialog's title and description, its body, then `buttons` on the right of the footer.
pub(super) fn dialog_shell(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    body: impl IntoElement,
    buttons: Vec<Button>,
    cx: &App,
) -> AnyElement {
    shell(title.into(), None, description.into(), body, buttons, cx)
}

/// [`dialog_shell`] with a "?" by the title that opens `docs` of the documentation.
pub(super) fn dialog_shell_help(
    title: impl Into<SharedString>,
    docs: &'static str,
    description: impl Into<SharedString>,
    body: impl IntoElement,
    buttons: Vec<Button>,
    cx: &App,
) -> AnyElement {
    shell(
        title.into(),
        Some(docs),
        description.into(),
        body,
        buttons,
        cx,
    )
}

fn shell(
    title: SharedString,
    docs: Option<&'static str>,
    description: SharedString,
    body: impl IntoElement,
    buttons: Vec<Button>,
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
                    h_flex()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::BOLD)
                                .child(title),
                        )
                        .children(docs.map(|page| super::help::button("dialog-help", page, cx))),
                )
                .child(
                    div()
                        .text_color(c.muted)
                        .line_height(relative(1.5))
                        .child(description),
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
                .children(buttons),
        )
        .into_any_element()
}

pub(super) fn open(view: gpui::AnyView, width: f32, cx: &mut App) {
    AppState::global(cx).update(cx, |s, cx| s.open_modal(view, px(width), cx));
}

/// The form behind "Add offline account": just a player name.
pub struct AddOffline {
    name: Entity<InputState>,
    error: Option<SharedString>,
    /// Claiming the nick on BX ID before the account is added.
    busy: bool,
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
            busy: false,
            back,
            _name,
        }
    }

    /// Signed in to BX ID, the nick is claimed first: one another BX ID holds cannot be added.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().trim().to_string();
        let Ok(account) = riven_launch::accounts::offline(&name) else {
            self.error = Some(t!("accounts.bad_name").into());
            cx.notify();
            return;
        };
        let play = super::play::PlayState::global(cx).read(cx);
        if play.me().is_none() || play.owns(&account.name) {
            return self.add(account, cx);
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let nick = account.name.clone();
        let claimed = runtime::spawn(async move { riven_play::skins::claim(&nick).await });
        cx.spawn(async move |this, cx| {
            let Ok(claimed) = claimed.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match claimed {
                    Err(e @ riven_play::PlayError::Taken(_)) => {
                        this.error = Some(super::problem::play_error(&e).into());
                        cx.notify();
                    }
                    Ok(_) => this.add(account, cx),
                    // The limit, or BX Team out of reach: the account plays, only without a skin.
                    Err(e) => {
                        moonglass::toast::show(
                            moonglass::toast::ToastKind::Error,
                            super::problem::play_error(&e),
                            cx,
                        );
                        this.add(account, cx);
                    }
                }
            });
        })
        .detach();
    }

    fn add(&mut self, account: riven_format::Account, cx: &mut Context<Self>) {
        AppState::global(cx).update(cx, |s, cx| s.add_account(account, cx));
        super::play::PlayState::global(cx).update(cx, |s, cx| s.sync_nicks(cx));
        leave(self.back.as_ref(), cx);
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
                this.child(super::problem::inline(error, cx))
            });
        dialog_frame(
            t!("accounts.add_offline"),
            t!("accounts.offline_hint"),
            body,
            Button::new("add")
                .primary()
                .size(ButtonSize::Md)
                .label(t!("accounts.add"))
                .loading(self.busy)
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

type OnConfirm = Rc<dyn Fn(&mut Window, &mut App)>;

/// "Are you sure?" before something that cannot be undone.
pub struct Confirm {
    title: SharedString,
    message: SharedString,
    action: SharedString,
    on_confirm: OnConfirm,
}

impl Render for Confirm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let on_confirm = self.on_confirm.clone();
        dialog_frame(
            self.title.clone(),
            self.message.clone(),
            div(),
            Button::new("confirm")
                .danger()
                .size(ButtonSize::Md)
                .label(self.action.clone())
                .on_click(move |_, window, cx| {
                    AppState::global(cx).update(cx, |s, cx| s.close_modal(cx));
                    on_confirm(window, cx);
                }),
            None,
            cx,
        )
    }
}

/// Asks before a destructive action and runs `on_confirm` only when the user agrees.
pub fn confirm(
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
    action: impl Into<SharedString>,
    on_confirm: impl Fn(&mut Window, &mut App) + 'static,
    cx: &mut App,
) {
    let view = cx.new(|_| Confirm {
        title: title.into(),
        message: message.into(),
        action: action.into(),
        on_confirm: Rc::new(on_confirm),
    });
    open(view.into(), 420., cx);
}

/// An offline account takes its nick off BX ID with it; the nick stays this player's for a few days.
pub fn confirm_remove_account(id: String, name: String, cx: &mut App) {
    let offline = AppState::global(cx)
        .read(cx)
        .accounts
        .accounts
        .iter()
        .any(|a| a.id == id && a.kind == riven_format::AccountKind::Offline);
    let nick = offline && super::play::PlayState::global(cx).read(cx).owns(&name);
    let body = if nick {
        t!("confirm.remove_offline_body", name = name.clone())
    } else {
        t!("confirm.remove_account_body")
    };
    confirm(
        t!("confirm.remove_account_title", name = name.clone()),
        body,
        t!("accounts.remove"),
        move |_, cx| {
            let id = id.clone();
            AppState::global(cx).update(cx, |s, cx| s.remove_account(&id, cx));
            if nick {
                let name = name.clone();
                super::play::PlayState::global(cx).update(cx, |s, cx| s.release_nick(name, cx));
            }
        },
        cx,
    );
}

/// "Sign in with Microsoft": shows the device code, waits for the player to enter it.
pub struct AddMicrosoft {
    code: Option<riven_launch::accounts::DeviceCode>,
    error: Option<(SharedString, SharedString)>,
    back: Option<Back>,
    _task: Option<gpui::Task<()>>,
}

impl AddMicrosoft {
    fn new(back: Option<Back>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            code: None,
            error: None,
            back,
            _task: None,
        };
        this.start(cx);
        this
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.code = None;
        self.error = None;
        let (code_tx, code_rx) = tokio::sync::oneshot::channel();
        let code_tx = std::sync::Mutex::new(Some(code_tx));
        let signed_in = runtime::spawn(async move {
            riven_launch::accounts::sign_in(move |code| {
                if let Some(tx) = code_tx.lock().ok().and_then(|mut t| t.take()) {
                    let _ = tx.send(code);
                }
            })
            .await
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            if let Ok(code) = code_rx.await {
                let _ = this.update(cx, |this, cx| {
                    let view = cx.entity_id();
                    moonglass::copy::copy("ms-copy".into(), code.code.clone(), view, cx);
                    cx.open_url(&code.url);
                    this.code = Some(code);
                    cx.notify();
                });
            }
            let result = signed_in.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(account)) => {
                    AppState::global(cx).update(cx, |s, cx| s.add_account(account, cx));
                    leave(this.back.as_ref(), cx);
                }
                Ok(Err(e)) => {
                    tracing::warn!("{e}");
                    this.error = Some(ms_failure(&e));
                    cx.notify();
                }
                Err(_) => {}
            });
        }));
        cx.notify();
    }
}

/// A failed Microsoft sign-in in the player's language; anything unforeseen keeps its own text.
fn ms_failure(e: &riven_launch::LaunchError) -> (SharedString, SharedString) {
    use riven_launch::LaunchError;
    use riven_launch::accounts::Refusal;
    let (title, hint) = match e {
        LaunchError::SignInRefused(Refusal::Declined) => (
            t!("accounts.ms_failed.declined"),
            t!("accounts.ms_failed.declined_hint"),
        ),
        LaunchError::SignInRefused(Refusal::Expired) => (
            t!("accounts.ms_failed.expired"),
            t!("accounts.ms_failed.expired_hint"),
        ),
        LaunchError::SignInRefused(Refusal::NoXbox) => (
            t!("accounts.ms_failed.no_xbox"),
            t!("accounts.ms_failed.no_xbox_hint"),
        ),
        LaunchError::SignInRefused(Refusal::Region) => (
            t!("accounts.ms_failed.region"),
            t!("accounts.ms_failed.region_hint"),
        ),
        LaunchError::SignInRefused(Refusal::Adult) => (
            t!("accounts.ms_failed.adult"),
            t!("accounts.ms_failed.adult_hint"),
        ),
        LaunchError::SignInRefused(Refusal::Child) => (
            t!("accounts.ms_failed.child"),
            t!("accounts.ms_failed.child_hint"),
        ),
        LaunchError::SignInRefused(Refusal::NotApproved) => (
            t!("accounts.ms_failed.not_approved"),
            t!("accounts.ms_failed.not_approved_hint"),
        ),
        LaunchError::SignInRefused(Refusal::NoMinecraft) => (
            t!("accounts.ms_failed.no_minecraft"),
            t!("accounts.ms_failed.no_minecraft_hint"),
        ),
        LaunchError::SignInOffline(_) => (
            t!("accounts.ms_failed.offline"),
            t!("accounts.ms_failed.offline_hint"),
        ),
        e => (t!("accounts.ms_failed.other"), e.to_string().into()),
    };
    (title.into(), hint.into())
}

/// A sign-in code in large type, with a button that copies it.
pub(super) fn device_code<V: 'static>(
    id: &'static str,
    code: String,
    cx: &Context<V>,
) -> impl IntoElement {
    let c = cx.theme().colors;
    let copied = moonglass::copy::copied(id, cx);
    let text = code.clone();
    h_flex()
        .relative()
        .justify_center()
        .py(px(14.))
        .rounded(px(10.))
        .bg(c.bg)
        .border_1()
        .border_color(c.border)
        .font_family(cx.theme().mono.clone())
        .text_size(px(26.))
        .font_weight(FontWeight::BOLD)
        .child(code)
        .child(
            div().absolute().right(px(10.)).child(
                moonglass::copy_button(id, move |_| text.clone(), cx)
                    .ghost()
                    .size(ButtonSize::Sm)
                    .tooltip(if copied {
                        t!("accounts.ms_copied")
                    } else {
                        t!("accounts.ms_copy")
                    }),
            ),
        )
}

impl Render for AddMicrosoft {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let body = match (&self.error, &self.code) {
            (Some(error), _) => super::problem::card(error, cx),
            (None, None) => h_flex()
                .gap(px(10.))
                .text_color(c.muted)
                .child(moonglass::motion::spinner(
                    moonglass::icon(moonglass::IconName::Loader, c.muted),
                    cx,
                ))
                .child(t!("accounts.ms_requesting").to_string())
                .into_any_element(),
            (None, Some(code)) => {
                let url = code.url.clone();
                v_flex()
                    .gap(px(12.))
                    .child(device_code("ms-copy", code.code.clone(), cx))
                    .child(
                        Button::new("ms-open")
                            .outline()
                            .size(ButtonSize::Md)
                            .label(t!("accounts.ms_open"))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .text_color(c.muted)
                            .child(moonglass::motion::spinner(
                                moonglass::icon(moonglass::IconName::Loader, c.muted),
                                cx,
                            ))
                            .child(t!("accounts.ms_waiting").to_string()),
                    )
                    .into_any_element()
            }
        };
        let retry = Button::new("ms-retry")
            .primary()
            .size(ButtonSize::Md)
            .label(t!("accounts.ms_retry"))
            .disabled(self.error.is_none())
            .on_click(cx.listener(|this, _, _, cx| this.start(cx)));
        // "Already copied" only holds while a code is on screen.
        let description = match (&self.error, &self.code) {
            (None, Some(_)) => t!("accounts.ms_hint"),
            _ => t!("accounts.ms_about"),
        };
        dialog_frame(
            t!("accounts.add_microsoft"),
            description,
            body,
            retry,
            self.back.clone(),
            cx,
        )
    }
}

pub fn open_add_microsoft(cx: &mut App) {
    open_add_microsoft_then(None, cx);
}

pub fn open_add_microsoft_then(back: Option<Back>, cx: &mut App) {
    let form = cx.new(|cx| AddMicrosoft::new(back, cx));
    open(form.into(), 420., cx);
}

/// The shadow under popovers: a wide soft one on light themes; a short faint one on dark themes,
/// where a wide one reads as a dark, banded halo.
pub fn popup_shadow(cx: &gpui::App) -> Vec<gpui::BoxShadow> {
    use moonglass::theme::ActiveTheme as _;
    if cx.theme().dark {
        vec![gpui_base::box_shadow(
            0.,
            4.,
            14.,
            0.,
            gpui::hsla(0., 0., 0., 0.22),
        )]
    } else {
        vec![gpui_base::box_shadow(
            0.,
            20.,
            50.,
            0.,
            gpui::hsla(0., 0., 0., 0.5),
        )]
    }
}
