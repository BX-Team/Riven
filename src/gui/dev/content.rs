use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, canvas, div, px, uniform_list,
};
use riven_build::author;
use riven_format::{Entry, Reason, Side, SourceKind, UpdatePolicy};
use rust_i18n::t;

use super::{DevView, Edge, Tab, Updates, View, dialogs};
use crate::gui::state::AppState;
use moonglass::theme::ActiveTheme as _;
use moonglass::{
    ActionMenu, Button, IconName, MenuEntry, TextField, h_flex, icon, scrollbar, tooltip, v_flex,
};

const ROW_HEIGHT: f32 = 34.;
const ACTIONS_WIDTH: f32 = 40.;
const NAME: usize = 0;
const FILE: usize = 1;
const SOURCE: usize = 2;
const SIDE: usize = 3;
const GROUP: usize = 4;

const COLUMN_GAP: f32 = 12.;
/// The gaps between the five columns.
pub(super) const COLUMN_GAPS: f32 = COLUMN_GAP * 4.;

/// The list's columns side by side, each as wide as [`DevView::column_widths`] gives.
fn columns(cells: [AnyElement; 5]) -> gpui_kit::Div {
    h_flex()
        .relative()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .gap(px(COLUMN_GAP))
        .children(cells)
}

pub(super) fn side_label(side: Side) -> SharedString {
    match side {
        Side::Client => t!("dev.side_client"),
        Side::Server => t!("dev.side_server"),
        Side::Both => t!("dev.side_both"),
    }
    .into()
}

impl DevView {
    pub(super) fn render_content(&self, view: &View, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let theme = cx.theme();
        let project = self.project.as_ref().map(|ws| &ws.project);
        let widths = self.column_widths(cx);
        let head = |label: String, column: usize, cx: &mut Context<Self>| {
            div()
                .relative()
                .whitespace_nowrap()
                .w(px(widths[column]))
                .flex_none()
                .child(div().truncate().child(label.to_uppercase()))
                .child(self.edge_handle(Edge::Column(column), cx).right(px(-9.)))
                .into_any_element()
        };
        let (update_label, update_busy, ready) = match &self.updates {
            Updates::Unchecked => (t!("dev.check_updates"), false, false),
            Updates::Checking => (t!("dev.checking_updates"), true, false),
            Updates::Ready(plan) => (t!("dev.update_n", n = plan.update.len()), false, true),
        };
        let add = cx.entity().downgrade();
        let empty = self.shown.is_empty();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(40.))
                    .gap(px(10.))
                    .px(px(14.))
                    .border_b_1()
                    .border_color(c.row)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(view.label(project)),
                    )
                    .child(
                        div()
                            .font_family(theme.mono.clone())
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .child(self.shown.len().to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        TextField::new(&self.filter)
                            .leading(IconName::Search)
                            .clearable()
                            .w(px(240.)),
                    )
                    .child({
                        let button = Button::new("dev-updates")
                            .icon(IconName::Refresh)
                            .label(update_label)
                            .disabled(update_busy || self.busy.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if ready {
                                    this.review_updates(window, cx)
                                } else {
                                    this.check_updates(true, cx)
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
                                dialogs::open_add(add.clone(), window, cx)
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(7.))
                    .font_family(theme.mono.clone())
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        columns([
                            head(t!("mods.name").to_string(), NAME, cx),
                            head(t!("dev.file").to_string(), FILE, cx),
                            head(t!("dev.source").to_string(), SOURCE, cx),
                            head(t!("dev.side").to_string(), SIDE, cx),
                            head(t!("dev.group").to_string(), GROUP, cx),
                        ])
                        .child({
                            let width = self.columns_width.clone();
                            canvas(
                                move |bounds, window, _| {
                                    let now = f32::from(bounds.size.width);
                                    if (now - width.get()).abs() >= 1. {
                                        width.set(now);
                                        window.refresh();
                                    }
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0()
                        }),
                    )
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
                    .child(scrollbar(&self.scroll))
                    .when(empty, |area| {
                        area.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(c.muted)
                                .child(t!("dev.view_empty").to_string()),
                        )
                    }),
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
        let selected = self.selected.as_deref() == Some(entry.id.as_str());
        let id = entry.id.clone();
        let menu_id = entry.id.clone();
        let widths = self.column_widths(cx);
        Some(
            h_flex()
                .id(ix)
                .w_full()
                .h(px(ROW_HEIGHT))
                .overflow_hidden()
                .gap(px(12.))
                .px(px(14.))
                .border_t_1()
                .border_color(c.row)
                .cursor_pointer()
                .map(|r| {
                    if selected {
                        r.bg(c.sel)
                    } else {
                        r.hover(|s| s.bg(c.row))
                    }
                })
                .when(entry.disabled, |r| r.opacity(0.55))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.select_entry(menu_id.clone(), cx);
                        let entries = this.entry_entries(&menu_id, cx);
                        AppState::global(cx)
                            .update(cx, |s, cx| s.open_context_menu(event.position, entries, cx));
                    }),
                )
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    if event.click_count() >= 2 {
                        this.show(Tab::Entry(id.clone()), cx);
                    } else {
                        this.select_entry(id.clone(), cx);
                    }
                }))
                .child(columns([
                    h_flex()
                        .w(px(widths[NAME]))
                        .flex_none()
                        .overflow_hidden()
                        .gap(px(6.))
                        .child(
                            div()
                                .min_w(px(60.))
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
                        })
                        .when(entry.disabled, |h| {
                            h.child(
                                div()
                                    .flex_none()
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(c.border)
                                    .text_size(px(11.))
                                    .text_color(c.muted)
                                    .child(t!("dev.disabled").to_string()),
                            )
                        })
                        .into_any_element(),
                    div()
                        .id(("file", ix))
                        .w(px(widths[FILE]))
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
                        })
                        .into_any_element(),
                    div()
                        .w(px(widths[SOURCE]))
                        .flex_none()
                        .truncate()
                        .text_color(source_color)
                        .child(author::source_name(source))
                        .into_any_element(),
                    div()
                        .w(px(widths[SIDE]))
                        .flex_none()
                        .truncate()
                        .text_color(c.text2)
                        .child(side_label(entry.side))
                        .into_any_element(),
                    div()
                        .w(px(widths[GROUP]))
                        .flex_none()
                        .truncate()
                        .text_color(if group.is_some() { c.text2 } else { c.muted })
                        .child(group.unwrap_or_else(|| "—".into()))
                        .into_any_element(),
                ]))
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

    fn entry_menu(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> ActionMenu {
        let trigger = Button::new(("entry-menu", ix))
            .ghost()
            .size(moonglass::ButtonSize::Xs)
            .icon(IconName::More);
        ActionMenu::new(
            ("entry-actions", ix),
            trigger,
            self.entry_entries(&entry.id, cx),
        )
    }

    /// Opening, side, group, pinning, switching off, update and removal of one entry.
    pub(super) fn entry_entries(&self, id: &str, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let Some(entry) = self.project.as_ref().and_then(|ws| ws.project.entry(id)) else {
            return Vec::new();
        };
        let view = cx.entity().downgrade();
        let id = entry.id.clone();
        let mut entries = Vec::new();
        {
            let (view, id) = (view.clone(), id.clone());
            entries.push(
                MenuEntry::action(t!("dev.open_entry"), move |_, cx| {
                    let id = id.clone();
                    let _ = view.update(cx, |this, cx| this.show(Tab::Entry(id), cx));
                })
                .icon(IconName::ExternalLink),
            );
        }
        entries.push(MenuEntry::Caption(t!("dev.side").into()));
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
            let disabled = entry.disabled;
            entries.push(
                MenuEntry::action(
                    if disabled {
                        t!("dev.enable")
                    } else {
                        t!("dev.disable")
                    },
                    move |_, cx| {
                        let id = id.clone();
                        let _ = view.update(cx, |this, cx| {
                            this.edit(|p| author::set_disabled(p, &id, !disabled), cx)
                        });
                    },
                )
                .icon(IconName::Power),
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
        entries
    }

    /// Looks up newer versions of everything; `loud` reports an up-to-date pack too.
    pub(super) fn check_updates(&mut self, loud: bool, cx: &mut Context<Self>) {
        let Some(ws) = self.project.clone() else {
            return;
        };
        if matches!(self.updates, Updates::Checking) {
            return;
        }
        self.updates = Updates::Checking;
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
                    Ok(plan) if plan.update.is_empty() => {
                        this.updates = Updates::Unchecked;
                        if loud {
                            this.notice = Some(t!("dev.up_to_date").into());
                        }
                    }
                    Ok(plan) => this.updates = Updates::Ready(plan),
                    Err(e) => {
                        this.updates = Updates::Unchecked;
                        if loud {
                            this.error = Some(e.to_string().into());
                        } else {
                            tracing::warn!("cannot check for updates: {e}");
                        }
                    }
                }
                this.refresh_rows(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn review_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Updates::Ready(plan) = &self.updates else {
            return;
        };
        let plan = plan.clone();
        let view = cx.entity().downgrade();
        dialogs::open_preview(t!("dev.updates_title").into(), plan, view, window, cx);
    }

    pub(super) fn update_entry(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.plan(
            t!("dev.updates_title").into(),
            t!("dev.checking_updates").into(),
            move |ws| Box::pin(async move { author::plan_updates(&ws, &[id], None).await }),
            window,
            cx,
        );
    }

    /// Plans moving an entry to another of its versions; the preview shows what comes along.
    pub(super) fn switch_version(
        &mut self,
        id: String,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.plan(
            t!("dev.switch_title").into(),
            t!("dev.switching").into(),
            move |ws| Box::pin(async move { author::plan_switch(&ws, &id, &version).await }),
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
