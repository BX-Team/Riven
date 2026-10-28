use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px, relative,
};
use riven_build::author;
use riven_format::{Entry, Group};
use rust_i18n::t;

use super::{DevView, dialogs};
use moonglass::theme::ActiveTheme as _;
use moonglass::{Button, ButtonSize, IconName, Switch, h_flex, v_flex};

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
    /// The pack tab: what the project is and its optional groups.
    pub(super) fn render_overview(&self, cx: &mut Context<Self>) -> AnyElement {
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
        let fact = |label: String, value: String| {
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(c.muted)
                        .child(label.to_uppercase()),
                )
                .child(div().font_family(mono.clone()).child(value))
        };
        let facts = h_flex()
            .flex_none()
            .gap(px(28.))
            .px(px(18.))
            .py(px(16.))
            .border_b_1()
            .border_color(c.row)
            .child(
                v_flex()
                    .gap(px(2.))
                    .mr(px(12.))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(20.))
                            .child(project.name.clone()),
                    )
                    .child(div().text_color(c.muted).child(project.id.clone())),
            )
            .child(fact(
                t!("dev.pack_version").to_string(),
                project.version.clone(),
            ))
            .child(fact("Minecraft".into(), project.minecraft.clone()))
            .child(fact(
                t!("dev.loader").to_string(),
                format!(
                    "{} {}",
                    crate::gui::launch_bar::loader_display(project.loader.kind),
                    project.loader.version
                ),
            ))
            .child(fact("Java".into(), project.java.major.to_string()))
            .child(fact(
                t!("dev.content").to_string(),
                project.content.len().to_string(),
            ));
        v_flex()
            .size_full()
            .child(facts)
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
}
