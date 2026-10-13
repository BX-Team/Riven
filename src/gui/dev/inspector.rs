use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use riven_build::author;
use riven_format::{Entry, PackPath, Reason, Side, UpdatePolicy};
use rust_i18n::t;

use super::content::side_label;
use super::entry::{Page, entry_icon, fitting, installed_version};
use super::{Check, DevView, Edge, Tab, View};
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{Button, Dropdown, IconName, MenuItem, Switch, h_flex, icon, v_flex};

pub(super) const INSPECTOR_WIDTH: f32 = 300.;
const LABEL_WIDTH: f32 = 112.;

/// Override files that look like an entry's configs: their name holds the entry's id or name.
fn configs<'a>(entry: &Entry, files: &'a [PackPath]) -> Vec<&'a PackPath> {
    let squash = |s: &str| {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_lowercase()
    };
    let keys: Vec<String> = [squash(&entry.id), squash(&entry.name)]
        .into_iter()
        .filter(|k| k.len() >= 4)
        .collect();
    files
        .iter()
        .filter(|p| {
            let path = squash(p.as_str().trim_start_matches("overrides/"));
            keys.iter().any(|k| path.contains(k.as_str()))
        })
        .take(12)
        .collect()
}

impl DevView {
    fn inspector_section(&self, title: SharedString, cx: &Context<Self>) -> gpui_kit::Div {
        let c = cx.theme().colors;
        div()
            .flex_none()
            .h(px(30.))
            .px(px(14.))
            .flex()
            .items_center()
            .bg(c.row)
            .border_t_1()
            .border_color(c.border)
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(12.))
            .child(title)
    }

    fn inspector_row(
        &self,
        label: SharedString,
        value: impl IntoElement,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        h_flex()
            .min_h(px(36.))
            .py(px(3.))
            .px(px(14.))
            .gap(px(8.))
            .child(
                div()
                    .w(px(LABEL_WIDTH))
                    .flex_none()
                    .text_color(c.muted)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(value))
            .into_any_element()
    }

    pub(super) fn render_inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let entry = self
            .selected
            .as_ref()
            .and_then(|id| self.project.as_ref()?.project.entry(id))
            .cloned();
        let body = match entry {
            Some(entry) => self.render_entry_inspector(&entry, cx),
            None => self.render_pack_inspector(cx),
        };
        v_flex()
            .relative()
            .flex_shrink(1.)
            .min_w(px(240.))
            .w(px(self.width(Edge::Inspector, cx)))
            .h_full()
            .border_l_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(body)
            .child(self.edge_handle(Edge::Inspector, cx).left(px(-3.)))
            .into_any_element()
    }

    /// The pack's facts while no entry is picked.
    fn render_pack_inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let Some(ws) = &self.project else {
            return div().into_any_element();
        };
        let p = &ws.project;
        let count = |view: View| self.view_count(&view).to_string();
        let value = |text: String| div().truncate().text_color(c.text2).child(text);
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(14.))
                    .child(icon(IconName::Package, c.text2).size(px(22.)))
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(p.name.clone()),
                            )
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .text_color(c.muted)
                                    .child(format!("{} · {}", p.id, p.version)),
                            ),
                    ),
            )
            .child(self.inspector_section(t!("dev.details").into(), cx))
            .child(self.inspector_row("Minecraft".into(), value(p.minecraft.clone()), cx))
            .child(self.inspector_row(
                t!("dev.loader").into(),
                value(format!(
                    "{} {}",
                    crate::gui::launch_bar::loader_display(p.loader.kind),
                    p.loader.version
                )),
                cx,
            ))
            .child(self.inspector_row("Java".into(), value(p.java.major.to_string()), cx))
            .child(self.inspector_section(t!("dev.content").into(), cx))
            .child(self.inspector_row(
                t!("dev.kind_mods").into(),
                value(count(View::Kind(riven_format::Kind::Mod))),
                cx,
            ))
            .child(self.inspector_row(
                t!("dev.view_dependencies").into(),
                value(count(View::Dependencies)),
                cx,
            ))
            .child(self.inspector_row(
                t!("dev.groups").into(),
                value(p.groups.len().to_string()),
                cx,
            ))
            .child(
                div()
                    .px(px(14.))
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .child(t!("dev.inspector_hint").to_string()),
            )
            .into_any_element()
    }

    fn render_entry_inspector(&self, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let Some(ws) = &self.project else {
            return div().into_any_element();
        };
        let project = &ws.project;
        let page = self.pages.get(&entry.id);
        let view = cx.entity().downgrade();
        let id = entry.id.clone();
        let source = author::source_name(entry.source.kind());
        let version_label = match (page, installed_version(entry)) {
            (Some(Page::Ready(facts)), Some(v)) => facts
                .versions
                .iter()
                .find(|x| x.id == v)
                .map(|x| x.number.clone()),
            _ => None,
        }
        .unwrap_or_else(|| entry.file.path.file_name().to_owned());
        let needed_by: Vec<String> = author::required_by(project, &entry.id)
            .into_iter()
            .map(|e| e.name.clone())
            .collect();
        let problems: Vec<String> = match &self.check {
            Check::Done(report) => report
                .problems
                .iter()
                .filter(|p| p.entry() == entry.id)
                .map(ToString::to_string)
                .collect(),
            _ => Vec::new(),
        };
        let text = |s: String| div().truncate().text_color(c.text2).child(s);

        let version: AnyElement = match page {
            Some(Page::Ready(facts)) => {
                let items: Vec<MenuItem> = fitting(&facts.versions, entry, project, true)
                    .into_iter()
                    .map(|v| MenuItem::new(v.id.clone(), v.number.clone()))
                    .collect();
                let (view, id) = (view.clone(), id.clone());
                let current = installed_version(entry).map(|v| SharedString::from(v.to_owned()));
                Dropdown::new(
                    "inspector-version",
                    items,
                    current.clone(),
                    move |picked, window, cx| {
                        if Some(&picked) == current.as_ref() {
                            return;
                        }
                        let id = id.clone();
                        let _ = view.update(cx, |this, cx| {
                            this.switch_version(id, picked.to_string(), window, cx)
                        });
                    },
                )
                .width(px(176.))
                .into_any_element()
            }
            Some(Page::Loading) => text(t!("dev.versions_loading").to_string()).into_any_element(),
            _ => text(version_label.clone()).into_any_element(),
        };
        let enabled = {
            let (view, id) = (view.clone(), id.clone());
            Switch::new("inspector-enabled", !entry.disabled).on_change(move |on, _, cx| {
                let id = id.clone();
                let _ = view.update(cx, |this, cx| {
                    this.edit(|p| author::set_disabled(p, &id, !on), cx)
                });
            })
        };
        let pinned = {
            let (view, id) = (view.clone(), id.clone());
            Switch::new("inspector-pin", entry.update == UpdatePolicy::Pinned).on_change(
                move |on, _, cx| {
                    let id = id.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.edit(|p| author::set_pinned(p, &id, on), cx)
                    });
                },
            )
        };
        let side = {
            let (view, id) = (view.clone(), id.clone());
            let items = [Side::Both, Side::Client, Side::Server]
                .into_iter()
                .map(|s| MenuItem::new(format!("{s:?}"), side_label(s)))
                .collect();
            Dropdown::new(
                "inspector-side",
                items,
                Some(format!("{:?}", entry.side).into()),
                move |picked, _, cx| {
                    let side = match picked.as_ref() {
                        "Client" => Side::Client,
                        "Server" => Side::Server,
                        _ => Side::Both,
                    };
                    let id = id.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.edit(|p| author::set_side(p, &id, side), cx)
                    });
                },
            )
            .width(px(176.))
        };
        let group = (!project.groups.is_empty()).then(|| {
            let (view, id) = (view.clone(), id.clone());
            let items = std::iter::once(MenuItem::new("", t!("dev.no_group").to_string()))
                .chain(
                    project
                        .groups
                        .iter()
                        .map(|g| MenuItem::new(g.id.clone(), g.name.clone())),
                )
                .collect();
            Dropdown::new(
                "inspector-group",
                items,
                Some(entry.group.clone().unwrap_or_default().into()),
                move |picked, _, cx| {
                    let group = (!picked.is_empty()).then(|| picked.to_string());
                    let id = id.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.edit(|p| author::set_group(p, &id, group), cx)
                    });
                },
            )
            .width(px(176.))
        });
        let config_rows: Vec<AnyElement> = configs(entry, &self.override_files)
            .into_iter()
            .enumerate()
            .map(|(i, path)| {
                let open = path.clone();
                h_flex()
                    .id(("inspector-config", i))
                    .h(px(28.))
                    .px(px(14.))
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c.row))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.show(Tab::File(open.clone()), cx)),
                    )
                    .child(icon(IconName::File, c.muted).size(px(13.)))
                    .child(
                        div()
                            .truncate()
                            .font_family(mono.clone())
                            .text_size(px(12.))
                            .child(
                                path.as_str()
                                    .split_once('/')
                                    .and_then(|(_, rest)| rest.split_once('/'))
                                    .map_or(path.as_str(), |(_, rest)| rest)
                                    .to_owned(),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        let paths = if entry.reason == Reason::Dependency {
            author::why_paths(project, &entry.id).unwrap_or_default()
        } else {
            Vec::new()
        };
        let name_of = |id: &str| {
            project
                .entry(id)
                .map_or_else(|| id.to_owned(), |e| e.name.clone())
        };
        let open_id = entry.id.clone();
        let remove_id = entry.id.clone();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(14.))
                    .child(entry_icon(entry, page, 36., cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .id("inspector-title")
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(c.accent))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.show(Tab::Entry(open_id.clone()), cx)
                                    }))
                                    .child(entry.name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .text_color(c.muted)
                                    .child(format!("{version_label} · {source}")),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("inspector-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(!problems.is_empty(), |col| {
                        col.child(
                            v_flex().px(px(14.)).pb(px(10.)).gap(px(4.)).children(
                                problems.into_iter().map(|p| {
                                    div().text_color(crate::gui::theme::danger()).child(p)
                                }),
                            ),
                        )
                    })
                    .child(self.inspector_section(t!("dev.details").into(), cx))
                    .child(self.inspector_row(
                        t!("dev.side").into(),
                        text(side_label(entry.side).to_string()),
                        cx,
                    ))
                    .child(self.inspector_row(t!("dev.source").into(), text(source.to_owned()), cx))
                    .child(
                        self.inspector_row(
                            t!("dev.file").into(),
                            div()
                                .truncate()
                                .font_family(mono.clone())
                                .text_size(px(12.))
                                .child(entry.file.path.file_name().to_owned()),
                            cx,
                        ),
                    )
                    .child(self.inspector_row(
                        t!("dev.why_needed_by").into(),
                        text(if needed_by.is_empty() {
                            "—".into()
                        } else {
                            needed_by.join(", ")
                        }),
                        cx,
                    ))
                    .when(!paths.is_empty(), |col| {
                        col.child(self.inspector_row(
                            t!("dev.why_paths").into(),
                            v_flex().children(paths.iter().map(|p| {
                                div().text_color(c.text2).child(
                                    p.iter()
                                        .map(|id| name_of(id))
                                        .collect::<Vec<_>>()
                                        .join(" → "),
                                )
                            })),
                            cx,
                        ))
                    })
                    .child(self.inspector_section(t!("dev.settings").into(), cx))
                    .child(self.inspector_row(t!("dev.version").into(), version, cx))
                    .child(self.inspector_row(t!("dev.enabled").into(), enabled, cx))
                    .child(self.inspector_row(t!("dev.pin_version").into(), pinned, cx))
                    .child(self.inspector_row(t!("dev.side").into(), side, cx))
                    .when_some(group, |col, group| {
                        col.child(self.inspector_row(t!("dev.group").into(), group, cx))
                    })
                    .when(!config_rows.is_empty(), |col| {
                        col.child(self.inspector_section(t!("dev.configs").into(), cx))
                            .children(config_rows)
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px(px(14.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(c.border)
                    .child(
                        Button::new("inspector-remove")
                            .outline()
                            .icon(IconName::Trash)
                            .text_color(crate::gui::theme::danger())
                            .label(t!("dev.remove"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_entry(&remove_id, window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("inspector-close")
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip(t!("common.close"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}
