use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, img, px, relative,
};
use riven_build::author;
use riven_format::{Entry, Kind, LoaderKind, Project, Source};
use riven_sources::{Channel, ProjectInfo, Source as _, Version};
use rust_i18n::t;

use super::DevView;
use super::content::side_label;
use moonglass::theme::ActiveTheme as _;
use moonglass::{Button, IconName, Switch, h_flex, icon, motion, tile, v_flex};

/// Versions an entry page lists at most.
const VERSIONS: usize = 200;

/// What Modrinth tells about an entry, for its page and the inspector.
pub(super) enum Page {
    Loading,
    Ready(Box<Facts>),
    Failed(SharedString),
}

pub(super) struct Facts {
    pub(super) info: ProjectInfo,
    /// Newest first.
    pub(super) versions: Vec<Version>,
    pub(super) icon: Option<PathBuf>,
}

pub(super) fn modrinth_project(entry: &Entry) -> Option<&str> {
    match &entry.source {
        Source::Modrinth { project, .. } => Some(project),
        _ => None,
    }
}

pub(super) fn installed_version(entry: &Entry) -> Option<&str> {
    match &entry.source {
        Source::Modrinth { version, .. } => Some(version),
        _ => None,
    }
}

fn loader_names(kind: LoaderKind) -> &'static [&'static str] {
    match kind {
        LoaderKind::Fabric => &["fabric"],
        LoaderKind::Quilt => &["quilt", "fabric"],
        LoaderKind::Forge => &["forge"],
        LoaderKind::NeoForge => &["neoforge"],
    }
}

/// The versions that fit the pack's game, and its loader for mods.
pub(super) fn fitting<'a>(
    versions: &'a [Version],
    entry: &Entry,
    project: &Project,
    betas: bool,
) -> Vec<&'a Version> {
    let installed = installed_version(entry);
    versions
        .iter()
        .filter(|v| {
            Some(v.id.as_str()) == installed
                || ((betas || v.channel == Channel::Release)
                    && v.game_versions.contains(&project.minecraft)
                    && (entry.kind != Kind::Mod
                        || loader_names(project.loader.kind)
                            .iter()
                            .any(|l| v.loaders.iter().any(|x| x == l))))
        })
        .take(VERSIONS)
        .collect()
}

fn channel_label(channel: Channel) -> &'static str {
    match channel {
        Channel::Release => "release",
        Channel::Beta => "beta",
        Channel::Alpha => "alpha",
    }
}

pub(super) fn published(version: &Version) -> String {
    riven_launch::parse_rfc3339(&version.published)
        .map(|secs| crate::gui::time::ago(UNIX_EPOCH + Duration::from_secs(secs)))
        .unwrap_or_default()
}

/// The entry's icon, or its initials on a tile.
pub(super) fn entry_icon(
    entry: &Entry,
    page: Option<&Page>,
    size: f32,
    cx: &gpui_kit::App,
) -> AnyElement {
    match page {
        Some(Page::Ready(facts)) if facts.icon.is_some() => {
            img(facts.icon.clone().expect("checked"))
                .size(px(size))
                .rounded(px(size / 5.))
                .flex_none()
                .into_any_element()
        }
        _ => {
            let initials: String = entry
                .name
                .split_whitespace()
                .filter_map(|w| w.chars().next())
                .take(2)
                .collect::<String>()
                .to_uppercase();
            tile(initials.into(), size, size / 5., cx)
                .text_size(px(size / 2.6))
                .into_any_element()
        }
    }
}

impl DevView {
    /// Reads an entry's Modrinth project and versions once, in the background.
    pub(super) fn load_page(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.pages.contains_key(id) {
            return;
        }
        let Some(ws) = &self.project else {
            return;
        };
        let Some(project) = ws
            .project
            .entry(id)
            .and_then(modrinth_project)
            .map(str::to_owned)
        else {
            return;
        };
        self.pages.insert(id.to_owned(), Page::Loading);
        let modrinth = ws.modrinth();
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let found = crate::gui::runtime::spawn(async move {
                let info = modrinth.project(&project).await?;
                let versions = modrinth.project_versions(&project).await?;
                let icon = match (&info.icon_url, crate::gui::mods::cache_dir()) {
                    (Some(url), Some(dir)) => {
                        crate::gui::mods::fetch_icon(&dir.join("icons"), &info.id, url).await
                    }
                    _ => None,
                };
                Ok::<_, riven_sources::Error>(Facts {
                    info,
                    versions,
                    icon,
                })
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                let page = match found {
                    Ok(Ok(facts)) => Page::Ready(Box::new(facts)),
                    Ok(Err(e)) => Page::Failed(e.to_string().into()),
                    Err(_) => Page::Failed(SharedString::default()),
                };
                this.pages.insert(id, page);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_entry_page(&self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let Some(ws) = &self.project else {
            return div().into_any_element();
        };
        let Some(entry) = ws.project.entry(id) else {
            return div()
                .p(px(18.))
                .text_color(c.muted)
                .child(t!("dev.entry_gone").to_string())
                .into_any_element();
        };
        let page = self.pages.get(id);
        let facts = match page {
            Some(Page::Ready(facts)) => Some(facts.as_ref()),
            _ => None,
        };
        let disabled = entry.disabled;
        let (toggle_id, update_id, remove_id) =
            (entry.id.clone(), entry.id.clone(), entry.id.clone());
        let actions = h_flex()
            .flex_none()
            .h(px(36.))
            .pl(px(14.))
            .border_b_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(entry.name.clone()),
            )
            .child(
                Button::new("entry-toggle")
                    .ghost()
                    .h_full()
                    .rounded(px(0.))
                    .border_l_1()
                    .border_color(c.border)
                    .label(if disabled {
                        t!("dev.enable")
                    } else {
                        t!("dev.disable")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = toggle_id.clone();
                        this.edit(|p| author::set_disabled(p, &id, !disabled), cx)
                    })),
            )
            .child(
                Button::new("entry-update")
                    .ghost()
                    .h_full()
                    .rounded(px(0.))
                    .border_l_1()
                    .border_color(c.border)
                    .label(t!("dev.update_one"))
                    .disabled(self.busy.is_some())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.update_entry(update_id.clone(), window, cx)
                    })),
            )
            .child(
                Button::new("entry-remove")
                    .ghost()
                    .h_full()
                    .rounded(px(0.))
                    .border_l_1()
                    .border_color(c.border)
                    .text_color(moonglass::theme::danger())
                    .label(t!("dev.remove"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.remove_entry(&remove_id, window, cx)
                    })),
            );
        let byline = [
            side_label(entry.side).to_string(),
            author::source_name(entry.source.kind()).to_owned(),
        ]
        .join(" · ");
        let link = facts.map(|f| format!("https://modrinth.com/project/{}", f.info.slug));
        let header = h_flex()
            .gap(px(14.))
            .items_start()
            .child(entry_icon(entry, page, 56., cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(20.))
                            .child(entry.name.clone()),
                    )
                    .child(div().text_color(c.muted).child(byline)),
            )
            .when_some(link, |row, link| {
                row.child(
                    Button::new("entry-modrinth")
                        .outline()
                        .icon(IconName::ExternalLink)
                        .label(t!("dev.modrinth_page"))
                        .on_click(move |_, _, cx| cx.open_url(&link)),
                )
            });
        let description = facts
            .map(|f| f.info.description.clone())
            .filter(|d| !d.is_empty());
        let versions: AnyElement = match page {
            None => div()
                .px(px(18.))
                .py(px(12.))
                .text_color(c.muted)
                .child(t!("dev.versions_modrinth_only").to_string())
                .into_any_element(),
            Some(Page::Loading) => h_flex()
                .px(px(18.))
                .py(px(12.))
                .gap(px(8.))
                .text_color(c.muted)
                .child(motion::spinner(
                    "entry-versions",
                    icon(IconName::Loader, c.muted).size(px(13.)),
                    cx,
                ))
                .child(t!("dev.versions_loading").to_string())
                .into_any_element(),
            Some(Page::Failed(e)) => div()
                .px(px(18.))
                .py(px(12.))
                .text_color(c.warn)
                .child(e.clone())
                .into_any_element(),
            Some(Page::Ready(facts)) => self.render_versions(entry, facts, cx),
        };
        v_flex()
            .size_full()
            .child(actions)
            .child(
                v_flex()
                    .id("entry-page")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .gap(px(12.))
                            .px(px(18.))
                            .py(px(18.))
                            .child(header)
                            .when_some(description, |col, d| {
                                col.child(
                                    div()
                                        .max_w(px(760.))
                                        .text_color(c.text2)
                                        .line_height(relative(1.5))
                                        .child(d),
                                )
                            })
                            .child(
                                h_flex()
                                    .gap(px(8.))
                                    .font_family(mono)
                                    .text_size(px(12.))
                                    .text_color(c.muted)
                                    .child(entry.file.path.as_str().to_owned()),
                            ),
                    )
                    .child(versions),
            )
            .into_any_element()
    }

    fn render_versions(&self, entry: &Entry, facts: &Facts, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let Some(ws) = &self.project else {
            return div().into_any_element();
        };
        let installed = installed_version(entry).map(str::to_owned);
        let list = fitting(&facts.versions, entry, &ws.project, self.betas);
        let rows: Vec<AnyElement> = list
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let current = installed.as_deref() == Some(v.id.as_str());
                let (id, version) = (entry.id.clone(), v.id.clone());
                h_flex()
                    .id(("version", i))
                    .h(px(30.))
                    .px(px(18.))
                    .gap(px(12.))
                    .border_t_1()
                    .border_color(c.row)
                    .when(current, |r| r.bg(c.sel))
                    .when(!current, |r| {
                        r.cursor_pointer()
                            .hover(|s| s.bg(c.row))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.switch_version(id.clone(), version.clone(), window, cx)
                            }))
                    })
                    .child(
                        div()
                            .w(px(300.))
                            .flex_none()
                            .truncate()
                            .font_family(mono.clone())
                            .text_size(px(12.))
                            .child(v.number.clone()),
                    )
                    .child(
                        div()
                            .w(px(90.))
                            .flex_none()
                            .text_color(if v.channel == Channel::Release {
                                c.text2
                            } else {
                                c.warn
                            })
                            .child(channel_label(v.channel)),
                    )
                    .child(
                        div()
                            .w(px(140.))
                            .flex_none()
                            .text_color(c.text2)
                            .child(published(v)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(c.accent)
                            .font_weight(FontWeight::SEMIBOLD)
                            .when(current, |d| d.child(t!("dev.in_pack").to_string())),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        v_flex()
            .child(
                h_flex()
                    .h(px(36.))
                    .px(px(18.))
                    .gap(px(10.))
                    .bg(c.panel)
                    .border_t_1()
                    .border_color(c.border)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("dev.versions").to_string()),
                    )
                    .child({
                        let view = cx.entity().downgrade();
                        Switch::new("entry-betas", self.betas)
                            .label(t!("dev.betas").to_string())
                            .on_change(move |on, _, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.betas = on;
                                    cx.notify();
                                });
                            })
                    }),
            )
            .child(
                h_flex()
                    .h(px(26.))
                    .px(px(18.))
                    .gap(px(12.))
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .child(div().w(px(300.)).child(t!("dev.version").to_uppercase()))
                    .child(div().w(px(90.)).child(t!("dev.channel").to_uppercase()))
                    .child(
                        div()
                            .w(px(140.))
                            .child(t!("dev.published_col").to_uppercase()),
                    ),
            )
            .when(empty, |col| {
                col.child(
                    div()
                        .px(px(18.))
                        .py(px(10.))
                        .text_color(c.muted)
                        .child(t!("dev.versions_none").to_string()),
                )
            })
            .children(rows)
            .child(
                div()
                    .px(px(18.))
                    .py(px(10.))
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .child(t!("dev.versions_hint").to_string()),
            )
            .into_any_element()
    }
}
