use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, uniform_list,
};
use riven_build::author;
use riven_format::{Entry, Reason, Side, SourceKind, UpdatePolicy};
use rust_i18n::t;

use super::{DevView, Updates, dialogs};
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{
    ActionMenu, Button, IconName, MenuEntry, TextField, h_flex, icon, scrollbar, tooltip, v_flex,
};

const ROW_HEIGHT: f32 = 36.;
const FILE_WIDTH: f32 = 300.;
const SOURCE_WIDTH: f32 = 90.;
const SIDE_WIDTH: f32 = 80.;
const GROUP_WIDTH: f32 = 110.;
const ACTIONS_WIDTH: f32 = 40.;

fn side_label(side: Side) -> SharedString {
    match side {
        Side::Client => t!("dev.side_client"),
        Side::Server => t!("dev.side_server"),
        Side::Both => t!("dev.side_both"),
    }
    .into()
}

impl DevView {
    pub(super) fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let theme = cx.theme();
        let head = |label: String, width: Option<f32>| {
            let cell = div().child(label.to_uppercase());
            match width {
                Some(w) => cell.w(px(w)).flex_none(),
                None => cell.flex_1().min_w_0(),
            }
        };
        let (update_label, update_busy, ready) = match &self.updates {
            Updates::Unchecked => (t!("dev.check_updates"), false, false),
            Updates::Checking => (t!("dev.checking_updates"), true, false),
            Updates::Ready(plan) => (t!("dev.update_n", n = plan.update.len()), false, true),
        };
        let view = cx.entity().downgrade();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(c.row)
                    .child(
                        TextField::new(&self.filter)
                            .leading(IconName::Search)
                            .w(px(280.)),
                    )
                    .child(div().flex_1())
                    .child({
                        let button = Button::new("dev-updates")
                            .icon(IconName::Refresh)
                            .label(update_label)
                            .disabled(update_busy || self.busy.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if ready {
                                    this.review_updates(window, cx)
                                } else {
                                    this.check_updates(cx)
                                }
                            }));
                        if ready { button.primary() } else { button }
                    })
                    .child(
                        Button::new("dev-add")
                            .primary()
                            .icon(IconName::Plus)
                            .label(t!("dev.add"))
                            .disabled(self.busy.is_some())
                            .on_click(move |_, window, cx| {
                                dialogs::open_add(view.clone(), window, cx)
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(8.))
                    .font_family(theme.mono.clone())
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .font_weight(FontWeight::MEDIUM)
                    .child(head(t!("mods.name").to_string(), None))
                    .child(head(t!("dev.file").to_string(), Some(FILE_WIDTH)))
                    .child(head(t!("dev.source").to_string(), Some(SOURCE_WIDTH)))
                    .child(head(t!("dev.side").to_string(), Some(SIDE_WIDTH)))
                    .child(head(t!("dev.group").to_string(), Some(GROUP_WIDTH)))
                    .child(div().w(px(ACTIONS_WIDTH)).flex_none()),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "dev-content",
                            self.shown.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .filter_map(|ix| this.render_entry(ix, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .size_full()
                        .track_scroll(&self.scroll),
                    )
                    .child(scrollbar(&self.scroll)),
            )
            .into_any_element()
    }

    fn render_entry(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let c = theme.colors;
        let ws = self.project.as_ref()?;
        let entry = ws.project.content.get(*self.shown.get(ix)?)?;
        let next = match &self.updates {
            Updates::Ready(plan) => plan
                .update
                .iter()
                .find(|(old, _)| old.id == entry.id)
                .map(|(_, new)| new.file.path.file_name().to_owned()),
            _ => None,
        };
        let source = entry.source.kind();
        let source_color = match source {
            SourceKind::Modrinth => c.ok,
            SourceKind::GitHub => c.accent,
            SourceKind::Url => c.warn,
            SourceKind::Local => c.muted,
        };
        let group = entry
            .group
            .as_ref()
            .and_then(|g| ws.project.groups.iter().find(|x| &x.id == g))
            .map(|g| g.name.clone());
        let pinned = entry.update == UpdatePolicy::Pinned;
        let dependency = entry.reason == Reason::Dependency;
        Some(
            h_flex()
                .id(ix)
                .w_full()
                .h(px(ROW_HEIGHT))
                .gap(px(12.))
                .px(px(14.))
                .border_t_1()
                .border_color(c.row)
                .hover(|s| s.bg(c.row))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(entry.name.clone()),
                        )
                        .when(pinned, |h| {
                            h.child(
                                div()
                                    .id(("pinned", ix))
                                    .flex_none()
                                    .child(icon(IconName::Lock, c.muted).size(px(12.)))
                                    .tooltip(tooltip(t!("dev.pinned").into())),
                            )
                        })
                        .when(dependency, |h| {
                            h.child(
                                div()
                                    .flex_none()
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .bg(c.sel)
                                    .text_size(px(11.))
                                    .text_color(c.text2)
                                    .child(t!("dev.dependency").to_string()),
                            )
                        }),
                )
                .child(
                    div()
                        .id(("file", ix))
                        .w(px(FILE_WIDTH))
                        .flex_none()
                        .truncate()
                        .font_family(theme.mono.clone())
                        .text_size(px(12.))
                        .map(|d| match &next {
                            Some(next) => d
                                .text_color(c.accent)
                                .child(format!("↑ {next}"))
                                .tooltip(tooltip(entry.file.path.file_name().to_owned().into())),
                            None => d
                                .text_color(c.text2)
                                .child(entry.file.path.file_name().to_owned()),
                        }),
                )
                .child(
                    div()
                        .w(px(SOURCE_WIDTH))
                        .flex_none()
                        .text_color(source_color)
                        .child(author::source_name(source)),
                )
                .child(
                    div()
                        .w(px(SIDE_WIDTH))
                        .flex_none()
                        .text_color(c.text2)
                        .child(side_label(entry.side)),
                )
                .child(
                    div()
                        .w(px(GROUP_WIDTH))
                        .flex_none()
                        .truncate()
                        .text_color(if group.is_some() { c.text2 } else { c.muted })
                        .child(group.unwrap_or_else(|| "—".into())),
                )
                .child(
                    h_flex()
                        .w(px(ACTIONS_WIDTH))
                        .flex_none()
                        .justify_end()
                        .child(self.entry_menu(ix, entry, cx)),
                )
                .into_any_element(),
        )
    }

    /// Side, group, pinning, update and removal of one entry.
    fn entry_menu(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> ActionMenu {
        let view = cx.entity().downgrade();
        let id = entry.id.clone();
        let mut entries = vec![MenuEntry::Caption(t!("dev.side").into())];
        for side in [Side::Client, Side::Server, Side::Both] {
            let (view, id) = (view.clone(), id.clone());
            entries.push(
                MenuEntry::action(side_label(side), move |_, cx| {
                    let id = id.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.edit(|p| author::set_side(p, &id, side), cx)
                    });
                })
                .checked(entry.side == side),
            );
        }
        let groups = self
            .project
            .as_ref()
            .map(|ws| ws.project.groups.clone())
            .unwrap_or_default();
        if !groups.is_empty() {
            entries.push(MenuEntry::Caption(t!("dev.group").into()));
            let choices = std::iter::once((None, t!("dev.no_group").to_string()))
                .chain(groups.into_iter().map(|g| (Some(g.id), g.name)));
            for (group, label) in choices {
                let (view, id) = (view.clone(), id.clone());
                let on = entry.group == group;
                entries.push(
                    MenuEntry::action(label, move |_, cx| {
                        let (id, group) = (id.clone(), group.clone());
                        let _ = view.update(cx, |this, cx| {
                            this.edit(|p| author::set_group(p, &id, group), cx)
                        });
                    })
                    .checked(on),
                );
            }
        }
        entries.push(MenuEntry::Separator);
        let pinned = entry.update == UpdatePolicy::Pinned;
        {
            let (view, id) = (view.clone(), id.clone());
            entries.push(
                MenuEntry::action(t!("dev.pin"), move |_, cx| {
                    let id = id.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.edit(|p| author::set_pinned(p, &id, !pinned), cx)
                    });
                })
                .icon(IconName::Lock)
                .checked(pinned),
            );
        }
        {
            let (view, id) = (view.clone(), id.clone());
            entries.push(
                MenuEntry::action(t!("dev.update_one"), move |window, cx| {
                    let id = id.clone();
                    let _ = view.update(cx, |this, cx| this.update_entry(id, window, cx));
                })
                .icon(IconName::Refresh)
                .disabled(pinned || entry.source.kind() == SourceKind::Url),
            );
        }
        {
            let (view, id) = (view.clone(), id.clone());
            entries.push(
                MenuEntry::action(t!("dev.remove"), move |window, cx| {
                    let id = id.clone();
                    let _ = view.update(cx, |this, cx| this.remove_entry(&id, window, cx));
                })
                .icon(IconName::Trash)
                .danger(),
            );
        }
        let trigger = Button::new(("entry-menu", ix))
            .ghost()
            .size(crate::gui::ui::ButtonSize::Xs)
            .icon(IconName::More);
        ActionMenu::new(("entry-actions", ix), trigger, entries)
    }

    fn check_updates(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.project.clone() else {
            return;
        };
        self.updates = Updates::Checking;
        self.error = None;
        self.notice = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result =
                crate::gui::runtime::spawn(
                    async move { author::plan_updates(&ws, &[], None).await },
                )
                .await
                .unwrap_or_else(|_| Err(author::AuthorError::NoDataDir));
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(plan) if plan.is_empty() => {
                        this.updates = Updates::Unchecked;
                        this.notice = Some(t!("dev.up_to_date").into());
                    }
                    Ok(plan) => this.updates = Updates::Ready(plan),
                    Err(e) => {
                        this.updates = Updates::Unchecked;
                        this.error = Some(e.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn review_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Updates::Ready(plan) = &self.updates else {
            return;
        };
        let plan = plan.clone();
        let view = cx.entity().downgrade();
        dialogs::open_preview(t!("dev.updates_title").into(), plan, view, window, cx);
    }

    fn update_entry(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.plan(
            t!("dev.updates_title").into(),
            t!("dev.checking_updates").into(),
            move |ws| Box::pin(async move { author::plan_updates(&ws, &[id], None).await }),
            window,
            cx,
        );
    }

    /// Removes an entry at once when nothing else goes with it, otherwise shows what will.
    pub(super) fn remove_entry(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = &self.project else {
            return;
        };
        match author::plan_remove(ws, id, false) {
            Ok(plan) if plan.remove.len() == 1 && plan.notes.is_empty() => {
                self.apply_plan(&plan, cx)
            }
            Ok(plan) => {
                let view = cx.entity().downgrade();
                dialogs::open_preview(t!("dev.remove_title").into(), plan, view, window, cx);
            }
            Err(e) => {
                self.error = Some(e.to_string().into());
                cx.notify();
            }
        }
    }
}
