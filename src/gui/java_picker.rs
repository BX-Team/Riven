use std::rc::Rc;

use std::path::PathBuf;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Global, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use riven_format::JavaChoice;
use riven_launch::java::{self, JavaInstall};
use rust_i18n::t;

use super::runtime;
use moonglass::theme::ActiveTheme as _;
use moonglass::{
    Button, ContainScroll as _, IconName, TextField, W_SEMIBOLD, h_flex, icon, motion, v_flex,
};

/// The Java runtimes found on this machine, shared by every picker; scanned on first use.
#[derive(Default)]
struct Found {
    installs: Vec<JavaInstall>,
    scanned: bool,
    scanning: bool,
}

impl Global for Found {}

fn scan(cx: &mut App) {
    if cx.default_global::<Found>().scanning {
        return;
    }
    cx.global_mut::<Found>().scanning = true;
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        let installs = runtime::blocking(java::detect).await.unwrap_or_default();
        cx.update(|cx| {
            let found = cx.global_mut::<Found>();
            found.installs = installs;
            found.scanned = true;
            found.scanning = false;
            cx.refresh_windows();
        });
    })
    .detach();
}

type OnChange = Rc<dyn Fn(JavaChoice, &mut App)>;

/// One line of the list: what it says and what picking it chooses.
struct Entry {
    title: String,
    detail: String,
    major: Option<u32>,
    choice: JavaChoice,
}

/// Prism-style Java choice: "automatic", every Java found on the machine, or one picked by hand.
pub struct JavaPicker {
    key: SharedString,
    choice: JavaChoice,
    /// The major the game needs, to mark the runtimes that fit.
    required: Option<u32>,
    error: Option<SharedString>,
    on_change: OnChange,
    /// Where a path can be typed instead of browsed; made on first render, which has a window.
    path: Option<(Entity<InputState>, Subscription)>,
    scroll: ScrollHandle,
}

impl JavaPicker {
    pub fn new(
        key: impl Into<SharedString>,
        choice: JavaChoice,
        required: Option<u32>,
        on_change: impl Fn(JavaChoice, &mut App) + 'static,
        cx: &mut App,
    ) -> Entity<Self> {
        if !cx.default_global::<Found>().scanned {
            scan(cx);
        }
        cx.new(|_| Self {
            key: key.into(),
            choice,
            required,
            error: None,
            on_change: Rc::new(on_change),
            path: None,
            scroll: ScrollHandle::new(),
        })
    }

    /// Shows a choice made elsewhere without reporting it.
    pub fn show(&mut self, choice: JavaChoice, cx: &mut Context<Self>) {
        if self.choice != choice {
            self.choice = choice;
            cx.notify();
        }
    }

    fn pick(&mut self, choice: JavaChoice, cx: &mut Context<Self>) {
        self.error = None;
        self.choice = choice.clone();
        (self.on_change)(choice, cx);
        cx.notify();
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("java.browse").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.try_path(path, cx));
        })
        .detach();
    }

    /// Probes a typed or picked path; a Java home or its `bin` folder stands for the executable inside.
    fn try_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let exe = if cfg!(windows) { "java.exe" } else { "java" };
        let path = [path.join("bin").join(exe), path.join(exe)]
            .into_iter()
            .find(|p| path.is_dir() && p.is_file())
            .unwrap_or(path);
        cx.spawn(async move |this, cx| {
            let probe = path.clone();
            let found = runtime::blocking(move || java::probe(&probe))
                .await
                .ok()
                .flatten();
            let _ = this.update(cx, |this, cx| match found {
                Some(install) => {
                    let path = install.path.display().to_string();
                    let list = cx.global_mut::<Found>();
                    if !list.installs.iter().any(|i| i.path == install.path) {
                        list.installs.push(install);
                    }
                    this.pick(JavaChoice::Path { path }, cx);
                }
                None => {
                    this.error =
                        Some(t!("java.not_java", path = path.display().to_string()).into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn typed(&mut self, cx: &mut Context<Self>) {
        let Some((input, _)) = &self.path else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        if !text.is_empty() {
            self.try_path(PathBuf::from(text), cx);
        }
    }

    fn path_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some((input, _)) = &self.path {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("java.path_hint")));
        let sub = cx.subscribe(&input, |this, _, event, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.typed(cx);
            }
        });
        self.path = Some((input.clone(), sub));
        input
    }

    fn row(
        &self,
        ix: usize,
        option: Entry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Entry {
            title,
            detail,
            major,
            choice,
        } = option;
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let selected = self.choice == choice;
        let key = format!("{}:{ix}", self.key);
        let hover = motion::hover(SharedString::from(format!("{key}:hover")), window, cx);
        let bg = motion::animate(
            SharedString::from(format!("{key}:bg")),
            match (selected, hover.on) {
                (true, _) => c.sel,
                (false, true) => c.row,
                _ => c.row.opacity(0.),
            },
            window,
            cx,
        );
        let fits = match (major, self.required) {
            (Some(have), Some(need)) => Some(have == need),
            _ => None,
        };
        hover
            .track(h_flex().id(SharedString::from(key)))
            .gap(px(12.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .bg(bg)
            .on_click(cx.listener(move |this, _, _, cx| this.pick(choice.clone(), cx)))
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(if selected { c.accent } else { c.muted })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |d| {
                        d.child(div().size(px(6.)).rounded(px(3.)).bg(c.accent))
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(W_SEMIBOLD).truncate().child(title))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_family(mono)
                            .text_color(c.muted)
                            .truncate()
                            .child(detail),
                    ),
            )
            .when_some(fits, |row, fits| {
                row.child(
                    div()
                        .flex_none()
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(10.))
                        .text_size(px(11.))
                        .bg(if fits {
                            c.ok.opacity(0.15)
                        } else {
                            c.warn.opacity(0.15)
                        })
                        .text_color(if fits { c.ok } else { c.warn })
                        .child(if fits {
                            t!("java.fits").to_string()
                        } else {
                            t!("java.needs", major = self.required.unwrap_or_default()).into()
                        }),
                )
            })
            .into_any_element()
    }
}

fn describe(install: &JavaInstall) -> String {
    let vendor = install
        .vendor
        .trim_end_matches(" Corporation")
        .trim_end_matches(", Inc.");
    format!("Java {} · {vendor} · {}", install.version, install.arch)
}

impl Render for JavaPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let path = self.path_input(window, cx);
        let (installs, scanning) = {
            let found = cx.default_global::<Found>();
            (found.installs.clone(), found.scanning)
        };
        let mut rows = vec![self.row(
            0,
            Entry {
                title: t!("java.auto").into(),
                detail: t!("java.auto_hint").into(),
                major: self.required,
                choice: JavaChoice::Auto,
            },
            window,
            cx,
        )];
        for (i, install) in installs.iter().enumerate() {
            let choice = JavaChoice::Path {
                path: install.path.display().to_string(),
            };
            rows.push(self.row(
                i + 1,
                Entry {
                    title: describe(install),
                    detail: install.path.display().to_string(),
                    major: Some(install.major),
                    choice,
                },
                window,
                cx,
            ));
        }
        if let JavaChoice::Path { path } = &self.choice
            && !installs.iter().any(|i| i.path.as_os_str() == path.as_str())
        {
            rows.push(self.row(
                installs.len() + 1,
                Entry {
                    title: t!("java.custom").into(),
                    detail: path.clone(),
                    major: None,
                    choice: self.choice.clone(),
                },
                window,
                cx,
            ));
        }
        v_flex()
            .w_full()
            .gap(px(8.))
            .child(
                div()
                    .id(SharedString::from(format!("{}:list", self.key)))
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .contain_scroll(&self.scroll)
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.bg)
                    .p(px(4.))
                    .child(v_flex().gap(px(2.)).children(rows)),
            )
            .when_some(self.error.clone(), |col, e| {
                col.child(div().text_color(c.warn).child(e))
            })
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        TextField::new(&path)
                            .flex_1()
                            .font_family(cx.theme().mono.clone()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{}:use", self.key)))
                            .label(t!("java.use_path"))
                            .on_click(cx.listener(|this, _, _, cx| this.typed(cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        Button::new(SharedString::from(format!("{}:browse", self.key)))
                            .label(t!("java.browse"))
                            .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{}:rescan", self.key)))
                            .disabled(scanning)
                            .label(t!("java.rescan"))
                            .on_click(|_, _, cx| scan(cx)),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .when(scanning, |row| {
                                row.child(motion::spinner(
                                    "java-scan",
                                    icon(IconName::Loader, c.muted).size(px(13.)),
                                    cx,
                                ))
                                .child(t!("java.scanning").to_string())
                            })
                            .when(!scanning, |row| {
                                row.child(t!("java.found", n = installs.len()).to_string())
                            }),
                    ),
            )
    }
}
