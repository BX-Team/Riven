use std::collections::HashSet;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px, relative,
};
use riven_build::author;
use riven_format::{Entry, Group, Reason};
use rust_i18n::t;

use super::{Check, DevView, dialogs};
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{Button, ButtonSize, IconName, Switch, caption, h_flex, v_flex};

const DETAILS_WIDTH: f32 = 360.;

pub(super) fn section_head(
    title: SharedString,
    hint: SharedString,
    cx: &Context<DevView>,
) -> gpui_kit::Div {
    let c = cx.theme().colors;
    h_flex()
        .flex_none()
        .gap(px(12.))
        .px(px(18.))
        .py(px(12.))
        .border_b_1()
        .border_color(c.row)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::BOLD).child(title))
                .child(div().text_size(px(12.)).text_color(c.muted).child(hint)),
        )
}

impl DevView {
    pub(super) fn render_groups(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let Some(ws) = &self.project else {
            return div().into_any_element();
        };
        let project = &ws.project;
        let view = cx.entity().downgrade();
        let add = view.clone();
        let cards: Vec<AnyElement> = project
            .groups
            .iter()
            .enumerate()
            .map(|(i, group)| {
                let members: Vec<&Entry> = project
                    .content
                    .iter()
                    .filter(|e| e.group.as_deref() == Some(group.id.as_str()))
                    .collect();
                let names = members
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let toggled = group.clone();
                let edited = group.clone();
                let removed = group.clone();
                let (view_toggle, view_edit, view_remove) =
                    (view.clone(), view.clone(), view.clone());
                v_flex()
                    .gap(px(8.))
                    .p(px(14.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.panel)
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        h_flex()
                                            .gap(px(8.))
                                            .child(
                                                div()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_size(px(14.))
                                                    .child(group.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .font_family(mono.clone())
                                                    .text_size(px(12.))
                                                    .text_color(c.muted)
                                                    .child(group.id.clone()),
                                            ),
                                    )
                                    .when_some(group.description.clone(), |col, d| {
                                        col.child(div().mt(px(2.)).text_color(c.text2).child(d))
                                    }),
                            )
                            .child(
                                Switch::new(("group-default", i), group.default)
                                    .label(t!("dev.group_default").to_string())
                                    .on_change(move |on, _, cx| {
                                        let group = Group {
                                            default: on,
                                            ..toggled.clone()
                                        };
                                        let _ = view_toggle.update(cx, |this, cx| {
                                            this.edit(|p| author::update_group(p, group), cx)
                                        });
                                    }),
                            )
                            .child(
                                Button::new(("group-edit", i))
                                    .ghost()
                                    .size(ButtonSize::Xs)
                                    .icon(IconName::Settings)
                                    .tooltip(t!("dev.group_edit"))
                                    .on_click(move |_, window, cx| {
                                        dialogs::open_group(
                                            view_edit.clone(),
                                            Some(edited.clone()),
                                            window,
                                            cx,
                                        )
                                    }),
                            )
                            .child(
                                Button::new(("group-remove", i))
                                    .ghost()
                                    .size(ButtonSize::Xs)
                                    .icon(IconName::Trash)
                                    .tooltip(t!("dev.remove"))
                                    .on_click(move |_, _, cx| {
                                        let view = view_remove.clone();
                                        let id = removed.id.clone();
                                        crate::gui::dialogs::confirm(
                                            t!("dev.group_remove_title", name = removed.name),
                                            t!("dev.group_remove_body"),
                                            t!("dev.remove"),
                                            move |_, cx| {
                                                let id = id.clone();
                                                let _ = view.update(cx, |this, cx| {
                                                    this.edit(
                                                        |p| author::remove_group(p, &id).map(drop),
                                                        cx,
                                                    )
                                                });
                                            },
                                            cx,
                                        );
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .line_height(relative(1.5))
                            .child(if members.is_empty() {
                                t!("dev.group_empty").to_string()
                            } else {
                                t!("dev.group_members", n = members.len(), names = names)
                                    .to_string()
                            }),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = cards.is_empty();
        v_flex()
            .size_full()
            .child(
                section_head(t!("dev.groups").into(), t!("dev.groups_hint").into(), cx).child(
                    Button::new("group-add")
                        .primary()
                        .icon(IconName::Plus)
                        .label(t!("dev.group_add"))
                        .on_click(move |_, window, cx| {
                            dialogs::open_group(add.clone(), None, window, cx)
                        }),
                ),
            )
            .child(
                v_flex()
                    .id("dev-groups")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(18.))
                    .gap(px(10.))
                    .max_w(px(820.))
                    .when(empty, |col| {
                        col.child(
                            div()
                                .text_color(c.muted)
                                .child(t!("dev.groups_none").to_string()),
                        )
                    })
                    .children(cards),
            )
            .into_any_element()
    }

    pub(super) fn render_dependencies(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let Some(ws) = &self.project else {
            return div().into_any_element();
        };
        let project = &ws.project;
        let troubled: HashSet<String> = match &self.check {
            Check::Done(report) => report
                .problems
                .iter()
                .map(|p| p.entry().to_owned())
                .collect(),
            _ => HashSet::new(),
        };
        let orphans: HashSet<String> = riven_resolve::orphans(&project.content)
            .into_iter()
            .collect();
        let mut roots: Vec<&Entry> = project
            .content
            .iter()
            .filter(|e| e.reason == Reason::Explicit)
            .collect();
        roots.sort_by_cached_key(|e| e.name.to_lowercase());
        let mut lone: Vec<&Entry> = project
            .content
            .iter()
            .filter(|e| orphans.contains(&e.id))
            .collect();
        lone.sort_by_cached_key(|e| e.name.to_lowercase());

        let row = |n: usize, entry: &Entry, depth: usize, cx: &mut Context<Self>| {
            let on = self.explained.as_deref() == Some(entry.id.as_str());
            let id = entry.id.clone();
            h_flex()
                .id(("dep-row", n))
                .h(px(28.))
                .pl(px(18. + depth as f32 * 18.))
                .pr(px(18.))
                .gap(px(8.))
                .cursor_pointer()
                .map(|r| {
                    if on {
                        r.bg(c.sel)
                    } else {
                        r.hover(|s| s.bg(c.row))
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.explained = Some(id.clone());
                    cx.notify();
                }))
                .when(depth > 0, |r| r.child(div().text_color(c.muted).child("└")))
                .child(
                    div()
                        .truncate()
                        .when(depth == 0, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .when(depth > 0, |d| d.text_color(c.text2))
                        .child(entry.name.clone()),
                )
                .when(troubled.contains(&entry.id), |r| {
                    r.child(div().text_color(crate::gui::theme::danger()).child("●"))
                })
                .into_any_element()
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        for root in &roots {
            rows.push(row(rows.len(), root, 0, cx));
            let mut children: Vec<&Entry> = root
                .requires
                .iter()
                .filter_map(|r| project.entry(r))
                .collect();
            children.sort_by_cached_key(|e| e.name.to_lowercase());
            for child in children {
                rows.push(row(rows.len(), child, 1, cx));
            }
        }
        if !lone.is_empty() {
            rows.push(
                div()
                    .px(px(10.))
                    .pt(px(12.))
                    .child(caption(t!("dev.orphans"), cx))
                    .into_any_element(),
            );
            for entry in lone {
                rows.push(row(rows.len(), entry, 0, cx));
            }
        }

        let details = self
            .explained
            .as_ref()
            .and_then(|id| project.entry(id))
            .map(|entry| self.render_why(entry, cx));
        h_flex()
            .size_full()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(section_head(
                        t!("dev.dependencies").into(),
                        t!("dev.dependencies_hint").into(),
                        cx,
                    ))
                    .child(
                        v_flex()
                            .id("dev-deps")
                            .flex_1()
                            .min_h_0()
                            .py(px(6.))
                            .overflow_y_scroll()
                            .children(rows),
                    ),
            )
            .child(
                v_flex()
                    .id("dev-why")
                    .w(px(DETAILS_WIDTH))
                    .flex_none()
                    .h_full()
                    .overflow_y_scroll()
                    .border_l_1()
                    .border_color(c.border)
                    .bg(c.panel)
                    .p(px(16.))
                    .gap(px(12.))
                    .font_family(mono.clone())
                    .text_size(px(12.))
                    .map(|col| match details {
                        Some(d) => col.child(d),
                        None => col.child(
                            div()
                                .text_color(c.muted)
                                .child(t!("dev.why_pick").to_string()),
                        ),
                    }),
            )
            .into_any_element()
    }

    /// Why an entry is in the pack: the chains that pull it in and what it needs.
    fn render_why(&self, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let ui_font = cx.theme().font.clone();
        let Some(ws) = &self.project else {
            return div().into_any_element();
        };
        let project = &ws.project;
        let name_of = |id: &str| {
            project
                .entry(id)
                .map_or_else(|| id.to_owned(), |e| e.name.clone())
        };
        let paths = author::why_paths(project, &entry.id).unwrap_or_default();
        let needed_by: Vec<String> = author::required_by(project, &entry.id)
            .into_iter()
            .map(|e| e.name.clone())
            .collect();
        let needs: Vec<String> = entry.requires.iter().map(|r| name_of(r)).collect();
        let problems: Vec<String> = match &self.check {
            Check::Done(report) => report
                .problems
                .iter()
                .filter(|p| p.entry() == entry.id)
                .map(ToString::to_string)
                .collect(),
            _ => Vec::new(),
        };
        let orphan = entry.reason == Reason::Dependency && paths.is_empty();
        let block = |title: String, lines: Vec<String>, empty: String| {
            v_flex()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(c.muted)
                        .child(title.to_uppercase()),
                )
                .map(|col| {
                    if lines.is_empty() {
                        col.child(div().text_color(c.muted).child(empty))
                    } else {
                        col.children(
                            lines
                                .into_iter()
                                .map(|l| div().text_color(c.text2).child(l)),
                        )
                    }
                })
        };
        let id = entry.id.clone();
        v_flex()
            .gap(px(14.))
            .child(
                v_flex()
                    .font_family(ui_font)
                    .text_size(px(13.))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(15.))
                            .child(entry.name.clone()),
                    )
                    .child(
                        div()
                            .text_color(c.muted)
                            .child(if entry.reason == Reason::Explicit {
                                t!("dev.why_explicit").to_string()
                            } else {
                                t!("dev.why_dependency").to_string()
                            }),
                    ),
            )
            .when(!problems.is_empty(), |col| {
                col.child(
                    v_flex().gap(px(4.)).children(
                        problems
                            .into_iter()
                            .map(|p| div().text_color(crate::gui::theme::danger()).child(p)),
                    ),
                )
            })
            .when(entry.reason == Reason::Dependency, |col| {
                col.child(block(
                    t!("dev.why_paths").to_string(),
                    paths
                        .iter()
                        .map(|p| {
                            p.iter()
                                .map(|id| name_of(id))
                                .collect::<Vec<_>>()
                                .join(" → ")
                        })
                        .collect(),
                    t!("dev.why_orphan").to_string(),
                ))
            })
            .child(block(
                t!("dev.why_needed_by").to_string(),
                needed_by,
                "—".into(),
            ))
            .child(block(t!("dev.why_needs").to_string(), needs, "—".into()))
            .when(orphan, |col| {
                col.child(
                    h_flex().child(
                        Button::new("why-remove")
                            .icon(IconName::Trash)
                            .label(t!("dev.remove"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_entry(&id, window, cx)
                            })),
                    ),
                )
            })
            .into_any_element()
    }
}
