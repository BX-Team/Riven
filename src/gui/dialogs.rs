use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div, px, relative,
};
use riven_format::LoaderKind;
use riven_sources::Cache;
use rust_i18n::t;

use super::runtime;
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{Button, ButtonSize, TextField, h_flex, v_flex};

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

/// A dialog's title and description, its body, then Cancel and the main action.
fn dialog_frame(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    body: impl IntoElement,
    action: Button,
    cx: &App,
) -> AnyElement {
    let cancel = Button::new("cancel")
        .outline()
        .size(ButtonSize::Md)
        .label(t!("common.cancel"))
        .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx)));
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
                .children(buttons),
        )
        .into_any_element()
}

pub(super) fn open(view: gpui_kit::AnyView, width: f32, cx: &mut App) {
    AppState::global(cx).update(cx, |s, cx| s.open_modal(view, px(width), cx));
}

/// The form behind "Add offline account": just a player name.
pub struct AddOffline {
    name: Entity<InputState>,
    error: Option<SharedString>,
    _name: Subscription,
}

impl AddOffline {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            _name,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        match riven_launch::accounts::offline(&name) {
            Ok(account) => AppState::global(cx).update(cx, |s, cx| {
                s.add_account(account, cx);
                s.close_modal(cx);
            }),
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
            cx,
        )
    }
}

pub fn open_add_offline(window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| AddOffline::new(window, cx));
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

pub fn confirm_remove_account(id: String, name: String, cx: &mut App) {
    confirm(
        t!("confirm.remove_account_title", name = name),
        t!("confirm.remove_account_body"),
        t!("accounts.remove"),
        move |_, cx| {
            let id = id.clone();
            AppState::global(cx).update(cx, |s, cx| s.remove_account(&id, cx));
        },
        cx,
    );
}

/// "Sign in with Microsoft": shows the device code, waits for the player to enter it.
pub struct AddMicrosoft {
    code: Option<riven_launch::accounts::DeviceCode>,
    error: Option<SharedString>,
    copied: bool,
    _task: Option<gpui_kit::Task<()>>,
}

impl AddMicrosoft {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            code: None,
            error: None,
            copied: false,
            _task: None,
        };
        this.start(cx);
        this
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.code = None;
        self.error = None;
        self.copied = false;
        let (code_tx, code_rx) = tokio::sync::oneshot::channel();
        let code_tx = std::sync::Mutex::new(Some(code_tx));
        let signed_in = runtime::pinned(move || async move {
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
                    cx.open_url(&code.url);
                    this.code = Some(code);
                    cx.notify();
                });
            }
            let result = signed_in.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(account)) => AppState::global(cx).update(cx, |s, cx| {
                    s.add_account(account, cx);
                    s.close_modal(cx);
                }),
                Ok(Err(e)) => {
                    this.error = Some(e.to_string().into());
                    cx.notify();
                }
                Err(_) => {}
            });
        }));
        cx.notify();
    }
}

impl Render for AddMicrosoft {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let body = match (&self.error, &self.code) {
            (Some(error), _) => div()
                .text_color(c.warn)
                .line_height(relative(1.5))
                .child(error.clone())
                .into_any_element(),
            (None, None) => h_flex()
                .gap(px(10.))
                .text_color(c.muted)
                .child(super::ui::motion::spinner(
                    "ms-wait",
                    super::ui::icon(super::ui::IconName::Loader, c.muted),
                    cx,
                ))
                .child(t!("accounts.ms_requesting").to_string())
                .into_any_element(),
            (None, Some(code)) => {
                let text = code.code.clone();
                let url = code.url.clone();
                v_flex()
                    .gap(px(12.))
                    .child(
                        h_flex()
                            .justify_center()
                            .py(px(14.))
                            .rounded(px(10.))
                            .bg(c.bg)
                            .border_1()
                            .border_color(c.border)
                            .font_family(mono.clone())
                            .text_size(px(26.))
                            .font_weight(FontWeight::BOLD)
                            .child(code.code.clone()),
                    )
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(
                                Button::new("ms-copy")
                                    .outline()
                                    .size(ButtonSize::Md)
                                    .flex_1()
                                    .label(if self.copied {
                                        t!("accounts.ms_copied")
                                    } else {
                                        t!("accounts.ms_copy")
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                            text.clone(),
                                        ));
                                        this.copied = true;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("ms-open")
                                    .outline()
                                    .size(ButtonSize::Md)
                                    .flex_1()
                                    .label(t!("accounts.ms_open"))
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .text_color(c.muted)
                            .child(super::ui::motion::spinner(
                                "ms-wait",
                                super::ui::icon(super::ui::IconName::Loader, c.muted),
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
        dialog_frame(
            t!("accounts.add_microsoft"),
            t!("accounts.ms_hint"),
            body,
            retry,
            cx,
        )
    }
}

pub fn open_add_microsoft(cx: &mut App) {
    let form = cx.new(AddMicrosoft::new);
    open(form.into(), 420., cx);
}
