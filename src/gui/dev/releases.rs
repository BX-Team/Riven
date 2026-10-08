use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, relative,
};
use riven_build::author;
use riven_build::ship::{self, DEFAULT_BRANCH, DEFAULT_REMOTE, DIST, ExportKind, FileChange};
use riven_format::Channel;
use rust_i18n::t;

use super::DevView;
use super::sections::section_head;
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{
    ActionMenu, Button, ButtonSize, Dropdown, IconName, MenuEntry, MenuItem, Switch, TextField,
    caption, h_flex, v_flex,
};

const BASE_CHANNELS: [&str; 2] = ["stable", "beta"];
const SHOWN_CHANGES: usize = 40;
const ICON_FILE: &str = "icon.png";

/// What the next build changes against the newest built release.
pub(super) enum Changes {
    Stale,
    Loading,
    Ready {
        base: Option<String>,
        files: usize,
        changes: Vec<FileChange>,
    },
    Failed(SharedString),
}

/// The Releases section's state: `dist/`, the signing key and the form.
pub(super) struct Releases {
    channel: SharedString,
    sign: bool,
    push: bool,
    key: Option<String>,
    built: Vec<String>,
    channels: BTreeMap<String, Channel>,
    pub(super) changes: Changes,
    link: Option<String>,
    exported: Option<PathBuf>,
    /// The pack icon as read from the project, drawn from memory so a new one shows at once.
    icon: Option<std::sync::Arc<gpui_kit::Image>>,
    version: Entity<InputState>,
    branch: Entity<InputState>,
    remote: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl Releases {
    pub(super) fn new(window: &mut Window, cx: &mut Context<DevView>) -> Self {
        let version = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("dev.version_hint").to_string()));
        let branch = cx.new(|cx| InputState::new(window, cx).default_value(DEFAULT_BRANCH));
        let remote = cx.new(|cx| InputState::new(window, cx).default_value(DEFAULT_REMOTE));
        let subs = vec![
            cx.subscribe(&version, |this, input, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let to = input.read(cx).value().trim().to_string();
                    if !to.is_empty() {
                        this.bump(&to, cx);
                    }
                }
            }),
        ];
        Self {
            channel: BASE_CHANNELS[0].into(),
            sign: true,
            push: true,
            key: None,
            built: Vec::new(),
            channels: BTreeMap::new(),
            changes: Changes::Stale,
            link: None,
            exported: None,
            icon: None,
            version,
            branch,
            remote,
            _subs: subs,
        }
    }
}

fn change_line(change: &FileChange) -> (&'static str, &str) {
    match change {
        FileChange::Added(p) => ("+", p),
        FileChange::Changed(p) => ("~", p),
        FileChange::Removed(p) => ("-", p),
    }
}

impl DevView {
    fn dist(&self) -> Option<PathBuf> {
        self.project.as_ref().map(|ws| ws.dir.join(DIST))
    }

    /// Reads `dist/` and the signing key again.
    pub(super) fn refresh_dist(&mut self) {
        let Some(ws) = &self.project else {
            return;
        };
        let dist = ws.dir.join(DIST);
        let r = &mut self.releases;
        r.built = ship::releases(&dist);
        r.channels = ship::channels(&dist);
        r.key = ship::load_key(&ws.project.id)
            .ok()
            .flatten()
            .map(|k| k.public().to_string());
        r.sign = r.key.is_some();
        r.changes = Changes::Stale;
        r.icon = ws
            .project
            .icon
            .as_ref()
            .and_then(|p| std::fs::read(ws.dir.join(p.as_str())).ok())
            .map(|bytes| {
                std::sync::Arc::new(gpui_kit::Image::from_bytes(
                    gpui_kit::ImageFormat::Png,
                    bytes,
                ))
            });
    }

    /// Makes a picked image the pack icon: `icon.png` next to `riven.json`.
    fn pick_pack_icon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.project.as_ref().map(|ws| ws.dir.clone()) else {
            return;
        };
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let target = dir.join(ICON_FILE);
            let written = crate::gui::runtime::blocking(move || {
                let png = std::fs::read(&path)
                    .ok()
                    .and_then(|b| crate::gui::mods::pack_icon(&b))
                    .ok_or_else(|| t!("new_instance.bad_icon").to_string())?;
                std::fs::write(&target, png).map_err(|e| format!("{}: {e}", target.display()))
            })
            .await
            .unwrap_or_else(|_| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| match written {
                Ok(()) => {
                    this.edit(
                        |p| {
                            p.icon = Some(
                                riven_format::PackPath::new(ICON_FILE)
                                    .expect("static path is valid"),
                            );
                            Ok(())
                        },
                        cx,
                    );
                    this.refresh_dist();
                }
                Err(e) => {
                    this.error = Some(e.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Works out what the next build changes, once per project state.
    pub(super) fn ensure_changes(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.releases.changes, Changes::Stale) {
            return;
        }
        let Some(ws) = self.project.clone() else {
            return;
        };
        let base = self.releases.built.first().cloned();
        self.releases.changes = Changes::Loading;
        let dist = ws.dir.join(DIST);
        cx.spawn(async move |this, cx| {
            let result = crate::gui::runtime::blocking(move || {
                let built = riven_build::release::build_release(&ws.project, &ws.dir)
                    .map_err(|e| e.to_string())?;
                let new = built.release;
                let old = base.as_deref().and_then(|v| ship::read_release(&dist, v));
                let changes = old
                    .as_ref()
                    .map(|old| ship::changes(old, &new))
                    .unwrap_or_default();
                Ok::<_, String>((base, new.files.len(), changes))
            })
            .await
            .unwrap_or_else(|_| Err(String::new()));
            let _ = this.update(cx, |this, cx| {
                if !matches!(this.releases.changes, Changes::Loading) {
                    return;
                }
                this.releases.changes = match result {
                    Ok((base, files, changes)) => Changes::Ready {
                        base,
                        files,
                        changes,
                    },
                    Err(e) => Changes::Failed(e.into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn bump(&mut self, to: &str, cx: &mut Context<Self>) {
        let to = to.to_owned();
        self.edit(
            move |p| {
                p.version = author::bump(&p.version, &to)?;
                Ok(())
            },
            cx,
        );
    }

    fn keygen(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = &self.project else {
            return;
        };
        match ship::keygen(&ws.project.id) {
            Ok((_, path, _)) => {
                self.notice = Some(t!("dev.key_created", path = path.display()).into());
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
        self.refresh_dist();
        cx.notify();
    }

    fn build_release(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(dist)) = (self.project.clone(), self.dist()) else {
            return;
        };
        let channel = self.releases.channel.to_string();
        let unsigned = !self.releases.sign;
        let version = ws.project.version.clone();
        self.job(
            t!("dev.building").into(),
            async move {
                let key = ship::signing_key(&ws.project.id, unsigned)?;
                ship::build(&ws, &dist, &channel, key.as_ref())?;
                Ok::<_, ship::ShipError>(channel)
            },
            move |this, channel, cx| {
                this.notice = Some(t!("dev.built", version = version, channel = channel).into());
                this.refresh_dist();
                cx.notify();
            },
            cx,
        );
    }

    fn publish(&mut self, channel: String, version: String, cx: &mut Context<Self>) {
        let (Some(ws), Some(dist)) = (self.project.as_ref(), self.dist()) else {
            return;
        };
        let key = match ship::load_key(&ws.project.id) {
            Ok(key) => key,
            Err(e) => {
                self.error = Some(e.to_string().into());
                cx.notify();
                return;
            }
        };
        match riven_build::release::publish(&dist, &channel, &version, key.as_ref()) {
            Ok(_) => {
                self.notice =
                    Some(t!("dev.published", channel = channel, version = version).into());
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
        self.refresh_dist();
        cx.notify();
    }

    fn deploy(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(dist)) = (self.project.clone(), self.dist()) else {
            return;
        };
        let branch = self.releases.branch.read(cx).value().trim().to_string();
        let remote = self.releases.remote.read(cx).value().trim().to_string();
        let push = self.releases.push;
        let (b, r) = (branch.clone(), remote.clone());
        self.job(
            t!("dev.deploying").into(),
            async move { ship::deploy(&ws, &dist, &b, &r, push) },
            move |this, shipped, cx| {
                let d = &shipped.deployed;
                let mut notice = match (&d.commit, d.pushed) {
                    (Some(_), true) => t!("dev.deployed_pushed", branch = branch, remote = remote),
                    (Some(_), false) => t!("dev.deployed", branch = branch),
                    (None, true) => t!("dev.deploy_pushed", branch = branch, remote = remote),
                    (None, false) => t!("dev.deploy_same", branch = branch),
                }
                .to_string();
                if d.created {
                    notice.push_str(" · ");
                    notice.push_str(&t!("dev.enable_pages", branch = branch));
                }
                this.notice = Some(notice.into());
                this.releases.link = shipped.link;
                this.refresh_dist();
                cx.notify();
            },
            cx,
        );
    }

    fn export(&mut self, kind: ExportKind, cx: &mut Context<Self>) {
        let Some(ws) = self.project.clone() else {
            return;
        };
        self.job(
            t!("dev.exporting").into(),
            async move { ship::export(&ws, kind, None, None).await },
            |this, report, cx| {
                let skipped = report.exported.skipped.len() + report.failures.len();
                let name = report
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                this.notice = Some(if skipped == 0 {
                    t!("dev.exported", name = name).into()
                } else {
                    t!("dev.exported_skipped", name = name, n = skipped).into()
                });
                if let Some(first) = report.failures.first().cloned().or_else(|| {
                    report
                        .exported
                        .skipped
                        .first()
                        .map(|(id, why)| format!("{id}: {why}"))
                }) {
                    this.error = Some(first.into());
                }
                this.releases.exported = Some(report.path);
                cx.notify();
            },
            cx,
        );
    }

    fn card(&self, title: SharedString, cx: &Context<Self>) -> gpui_kit::Div {
        let c = cx.theme().colors;
        v_flex()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(14.))
                    .child(title),
            )
    }

    fn render_version_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let Some(ws) = &self.project else {
            return div().into_any_element();
        };
        let current = ws.project.version.clone();
        let bump = |part: &'static str, label: SharedString, cx: &mut Context<Self>| {
            let next = author::bump(&current, part).ok();
            Button::new(SharedString::from(format!("bump-{part}")))
                .label(match &next {
                    Some(v) => SharedString::from(format!("{label} → {v}")),
                    None => label,
                })
                .disabled(next.is_none() || self.busy.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.bump(part, cx)))
        };
        self.card(t!("dev.version").into(), cx)
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(
                        div()
                            .font_family(mono)
                            .text_size(px(20.))
                            .font_weight(FontWeight::BOLD)
                            .child(current.clone()),
                    )
                    .when(self.releases.built.contains(&current), |row| {
                        row.child(
                            div()
                                .text_size(px(12.))
                                .text_color(c.warn)
                                .child(t!("dev.version_built").to_string()),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(match &self.releases.icon {
                        Some(image) => gpui_kit::img(image.clone())
                            .size(px(40.))
                            .rounded(px(8.))
                            .into_any_element(),
                        None => crate::gui::launch_bar::instance_tile(
                            &ws.project.name,
                            None,
                            40.,
                            8.,
                            cx,
                        ),
                    })
                    .child(
                        Button::new("pack-icon")
                            .icon(IconName::Image)
                            .label(t!("dev.pack_icon"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_pack_icon(window, cx)),
                            ),
                    )
                    .when(self.releases.icon.is_some(), |row| {
                        row.child(
                            Button::new("pack-icon-clear")
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip(t!("new_instance.clear_icon"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.edit(
                                        |p| {
                                            p.icon = None;
                                            Ok(())
                                        },
                                        cx,
                                    );
                                    this.refresh_dist();
                                })),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .child(t!("dev.pack_icon_hint").to_string()),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .flex_wrap()
                    .child(bump("patch", t!("dev.bump_patch").into(), cx))
                    .child(bump("minor", t!("dev.bump_minor").into(), cx))
                    .child(bump("major", t!("dev.bump_major").into(), cx))
                    .child(
                        div()
                            .w(px(180.))
                            .child(TextField::new(&self.releases.version)),
                    ),
            )
            .into_any_element()
    }

    fn render_build_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let r = &self.releases;
        let view = cx.entity().downgrade();
        let names = channel_names(&r.channels);
        let channels = names
            .iter()
            .map(|n| MenuItem::new(n.clone(), n.clone()))
            .collect();
        let pick = view.clone();
        let key_row = match &r.key {
            Some(key) => {
                let copy = key.clone();
                h_flex()
                    .gap(px(8.))
                    .child(
                        Switch::new("release-sign", r.sign)
                            .label(t!("dev.sign").to_string())
                            .on_change(move |on, _, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.releases.sign = on;
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(mono.clone())
                            .text_size(px(11.))
                            .text_color(c.muted)
                            .child(key.clone()),
                    )
                    .child(
                        Button::new("copy-key")
                            .ghost()
                            .size(ButtonSize::Xs)
                            .icon(IconName::Copy)
                            .tooltip(t!("dev.copy_key"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                            }),
                    )
            }
            None => h_flex()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .text_color(c.warn)
                        .child(t!("dev.no_key").to_string()),
                )
                .child(
                    Button::new("keygen")
                        .icon(IconName::Lock)
                        .label(t!("dev.keygen"))
                        .on_click(cx.listener(|this, _, _, cx| this.keygen(cx))),
                ),
        };
        let version = self
            .project
            .as_ref()
            .map(|ws| ws.project.version.clone())
            .unwrap_or_default();
        let unsigned_blocked = r.sign && r.key.is_none();
        self.card(t!("dev.build").into(), cx)
            .child(key_row)
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(
                        Dropdown::new(
                            "release-channel",
                            channels,
                            Some(r.channel.clone()),
                            move |v, _, cx| {
                                let _ = pick.update(cx, |this, cx| {
                                    this.releases.channel = v;
                                    cx.notify();
                                });
                            },
                        )
                        .width(px(160.)),
                    )
                    .child(
                        Button::new("release-build")
                            .primary()
                            .icon(IconName::Package)
                            .label(t!(
                                "dev.build_to",
                                version = version,
                                channel = r.channel.as_ref()
                            ))
                            .disabled(self.busy.is_some() || unsigned_blocked)
                            .on_click(cx.listener(|this, _, _, cx| this.build_release(cx))),
                    ),
            )
            .child(self.render_changes(cx))
            .into_any_element()
    }

    fn render_changes(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let body: Vec<AnyElement> = match &self.releases.changes {
            Changes::Stale | Changes::Loading => {
                vec![
                    div()
                        .text_color(c.muted)
                        .child(t!("dev.changes_loading").to_string())
                        .into_any_element(),
                ]
            }
            Changes::Failed(e) => {
                vec![div().text_color(c.warn).child(e.clone()).into_any_element()]
            }
            Changes::Ready {
                base: None, files, ..
            } => vec![
                div()
                    .text_color(c.muted)
                    .child(t!("dev.changes_first", n = files).to_string())
                    .into_any_element(),
            ],
            Changes::Ready {
                base: Some(_),
                changes,
                ..
            } if changes.is_empty() => vec![
                div()
                    .text_color(c.muted)
                    .child(t!("dev.changes_none").to_string())
                    .into_any_element(),
            ],
            Changes::Ready { changes, .. } => {
                let mut lines: Vec<AnyElement> = changes
                    .iter()
                    .take(SHOWN_CHANGES)
                    .map(|change| {
                        let (mark, path) = change_line(change);
                        let color = match mark {
                            "+" => c.ok,
                            "~" => c.warn,
                            _ => crate::gui::theme::danger(),
                        };
                        h_flex()
                            .gap(px(8.))
                            .child(div().flex_none().text_color(color).child(mark))
                            .child(div().min_w_0().truncate().child(path.to_owned()))
                            .into_any_element()
                    })
                    .collect();
                if changes.len() > SHOWN_CHANGES {
                    lines.push(
                        div()
                            .text_color(c.muted)
                            .child(
                                t!("dev.changes_more", n = changes.len() - SHOWN_CHANGES)
                                    .to_string(),
                            )
                            .into_any_element(),
                    );
                }
                lines
            }
        };
        let title = match &self.releases.changes {
            Changes::Ready {
                base: Some(base), ..
            } => t!("dev.changes_since", version = base).to_string(),
            _ => t!("dev.changes").to_string(),
        };
        v_flex()
            .gap(px(6.))
            .child(caption(title, cx))
            .child(
                v_flex()
                    .id("release-changes")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .gap(px(3.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.bg)
                    .font_family(mono)
                    .text_size(px(12.))
                    .text_color(c.text2)
                    .children(body),
            )
            .into_any_element()
    }

    fn render_built_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let r = &self.releases;
        let names = channel_names(&r.channels);
        let view = cx.entity().downgrade();
        let rows: Vec<AnyElement> = r
            .built
            .iter()
            .enumerate()
            .map(|(i, version)| {
                let on: Vec<&String> = r
                    .channels
                    .iter()
                    .filter(|(_, ch)| &ch.version == version)
                    .map(|(name, _)| name)
                    .collect();
                let entries = names
                    .iter()
                    .map(|channel| {
                        let current = r.channels.get(channel).map(|ch| ch.version.clone());
                        let here = current.as_deref() == Some(version.as_str());
                        let label = match &current {
                            Some(cur) if !here && older(version, cur) => {
                                t!("dev.rollback_to", channel = channel)
                            }
                            _ => t!("dev.publish_to", channel = channel),
                        };
                        let (view, channel, version) =
                            (view.clone(), channel.clone(), version.clone());
                        MenuEntry::action(label, move |_, cx| {
                            let (channel, version) = (channel.clone(), version.clone());
                            let _ = view.update(cx, |this, cx| this.publish(channel, version, cx));
                        })
                        .checked(here)
                        .disabled(here)
                    })
                    .collect();
                let trigger = Button::new(("release-menu", i))
                    .ghost()
                    .size(ButtonSize::Xs)
                    .icon(IconName::More);
                h_flex()
                    .h(px(32.))
                    .gap(px(10.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(c.row))
                    .child(
                        div()
                            .w(px(120.))
                            .flex_none()
                            .font_family(mono.clone())
                            .child(version.clone()),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .gap(px(6.))
                            .children(on.into_iter().map(|name| {
                                div()
                                    .px(px(7.))
                                    .py(px(1.))
                                    .rounded(px(10.))
                                    .bg(c.sel)
                                    .text_size(px(11.))
                                    .text_color(c.text)
                                    .child(name.clone())
                            })),
                    )
                    .child(
                        ActionMenu::new(("release-actions", i), trigger, entries).width(px(240.)),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        self.card(t!("dev.built_releases").into(), cx)
            .when(empty, |card| {
                card.child(
                    div()
                        .text_color(c.muted)
                        .child(t!("dev.no_releases").to_string()),
                )
            })
            .child(v_flex().gap(px(2.)).children(rows))
            .into_any_element()
    }

    fn render_deploy_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let r = &self.releases;
        let view = cx.entity().downgrade();
        let field = |label: String, input: &Entity<InputState>| {
            v_flex()
                .gap(px(4.))
                .child(div().text_size(px(12.)).text_color(c.muted).child(label))
                .child(div().w(px(160.)).child(TextField::new(input)))
        };
        self.card(t!("dev.deploy").into(), cx)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .line_height(relative(1.5))
                    .child(t!("dev.deploy_hint").to_string()),
            )
            .child(
                h_flex()
                    .items_end()
                    .gap(px(10.))
                    .child(field(t!("dev.branch").to_string(), &r.branch))
                    .child(field(t!("dev.remote").to_string(), &r.remote))
                    .child(
                        div().pb(px(6.)).child(
                            Switch::new("deploy-push", r.push)
                                .label(t!("dev.push").to_string())
                                .on_change(move |on, _, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        this.releases.push = on;
                                        cx.notify();
                                    });
                                }),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("deploy")
                            .primary()
                            .icon(IconName::ArrowUp)
                            .label(t!("dev.deploy_run"))
                            .disabled(self.busy.is_some() || r.built.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.deploy(cx))),
                    ),
            )
            .when_some(r.link.clone(), |card, link| {
                let copy = link.clone();
                card.child(
                    h_flex()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_color(c.muted)
                                .child(t!("dev.install_link").to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(mono)
                                .text_size(px(12.))
                                .child(link),
                        )
                        .child(
                            Button::new("copy-link")
                                .ghost()
                                .size(ButtonSize::Xs)
                                .icon(IconName::Copy)
                                .tooltip(t!("dev.copy"))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                                }),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_export_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let exports = self.project.as_ref().map(|ws| ws.dir.join(ship::EXPORTS));
        let button =
            |id: &'static str, kind: ExportKind, label: SharedString, cx: &mut Context<Self>| {
                Button::new(id)
                    .label(label)
                    .disabled(self.busy.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| this.export(kind, cx)))
            };
        self.card(t!("dev.export").into(), cx)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .line_height(relative(1.5))
                    .child(t!("dev.export_hint").to_string()),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(button(
                        "export-mrpack",
                        ExportKind::Mrpack,
                        "Modrinth (.mrpack)".into(),
                        cx,
                    ))
                    .child(button(
                        "export-prism",
                        ExportKind::Prism,
                        "Prism (.zip)".into(),
                        cx,
                    ))
                    .child(button(
                        "export-riven",
                        ExportKind::Riven,
                        "Riven (.riven)".into(),
                        cx,
                    ))
                    .child(div().flex_1())
                    .when_some(exports.filter(|d| d.is_dir()), |row, dir| {
                        row.child(
                            Button::new("open-exports")
                                .ghost()
                                .icon(IconName::Folder)
                                .label(t!("dev.open_exports"))
                                .on_click(move |_, _, cx| cx.open_with_system(&dir)),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_releases(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .child(section_head(
                t!("dev.releases").into(),
                t!("dev.releases_hint").into(),
                cx,
            ))
            .child(
                v_flex()
                    .id("dev-releases")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .p(px(18.))
                            .gap(px(12.))
                            .max_w(px(820.))
                            .child(self.render_version_card(cx))
                            .child(self.render_build_card(cx))
                            .child(self.render_built_card(cx))
                            .child(self.render_deploy_card(cx))
                            .child(self.render_export_card(cx)),
                    ),
            )
            .into_any_element()
    }
}

/// The usual channels, then any other the pack already has.
fn channel_names(channels: &BTreeMap<String, Channel>) -> Vec<String> {
    let mut names: Vec<String> = BASE_CHANNELS.iter().map(|s| (*s).to_owned()).collect();
    let extra: Vec<String> = channels
        .keys()
        .filter(|k| !names.contains(k))
        .cloned()
        .collect();
    names.extend(extra);
    names
}

/// Whether publishing `version` over `current` moves a channel back.
fn older(version: &str, current: &str) -> bool {
    ship::compare_versions(version, current).is_lt()
}
