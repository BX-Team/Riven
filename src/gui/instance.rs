use std::path::PathBuf;
use std::time::Instant;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::base::{Popover, box_shadow};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, UniformListScrollHandle, Window, div, hsla, img, px, uniform_list,
};
use rust_i18n::t;

use riven_launch::instances::Instances;

use super::instance_settings::InstanceSettings;
use super::logs::LogsView;
use super::mods::{self, ModsTable, SortBy};
use super::runtime;
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::time;
use super::ui::{
    Button, IconName, Tabs, TextField, UiText as _, h_flex, icon, motion, scrollbar, v_flex,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Mods,
    Configs,
    Logs,
    Screenshots,
    Settings,
}

const TABS: [Tab; 5] = [
    Tab::Mods,
    Tab::Configs,
    Tab::Logs,
    Tab::Screenshots,
    Tab::Settings,
];

impl Tab {
    fn label(self) -> SharedString {
        match self {
            Tab::Mods => t!("instance.tabs.mods"),
            Tab::Configs => t!("instance.tabs.configs"),
            Tab::Logs => t!("instance.tabs.logs"),
            Tab::Screenshots => t!("instance.tabs.screenshots"),
            Tab::Settings => t!("instance.tabs.settings"),
        }
        .into()
    }
}

const VERSION_WIDTH: f32 = 160.;
const MODIFIED_WIDTH: f32 = 130.;
const ROW_HEIGHT: f32 = 38.;

/// One instance: its header, tabs and the content of the open tab.
pub struct InstanceView {
    pub id: String,
    name: SharedString,
    version: Option<SharedString>,
    game_dir: PathBuf,
    tab: Tab,
    search: Entity<InputState>,
    mods: ModsTable,
    scroll: UniformListScrollHandle,
    settings: Entity<InstanceSettings>,
    logs: Entity<LogsView>,
    loading: bool,
    /// When the rows and the Modrinth icons arrived, for their entrance.
    shown_at: Option<Instant>,
    icons_at: Option<Instant>,
    _search: Subscription,
}

impl InstanceView {
    pub fn new(
        id: String,
        name: SharedString,
        version: Option<SharedString>,
        store: Instances,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let game_dir = store.game_dir(&id);
        let settings = cx.new(|cx| InstanceSettings::new(id.clone(), store, window, cx));
        let logs = super::logs::view(id.clone(), game_dir.clone(), cx);
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("mods.search").to_string()));
        let _search = cx.subscribe(&search, |this, input, event, cx| {
            if let InputEvent::Change = event {
                let text = input.read(cx).value().to_string();
                this.mods.set_filter(&text);
                cx.notify();
            }
        });
        let mut view = Self {
            id,
            name,
            version,
            game_dir,
            tab: Tab::Mods,
            search,
            mods: ModsTable::default(),
            scroll: UniformListScrollHandle::new(),
            settings,
            logs,
            loading: true,
            shown_at: None,
            icons_at: None,
            _search,
        };
        view.reload(window, cx);
        view
    }

    /// Reads `mods/` again, then fills in titles and icons from Modrinth.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        let dir = self.game_dir.clone();
        let game_dir = dir.clone();
        cx.spawn_in(window, async move |this, cx| {
            let rows = runtime::blocking(move || mods::scan_rows(&dir))
                .await
                .unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                if this.mods.rows().is_empty() {
                    this.shown_at = Some(Instant::now());
                }
                this.mods.set_rows(rows.clone());
                let hint = t!("mods.search_count", n = rows.len()).to_string();
                this.search
                    .update(cx, |s, cx| s.set_placeholder(hint, window, cx));
                cx.notify();
            });
            let rows = runtime::blocking(move || mods::describe(&game_dir, rows))
                .await
                .unwrap_or_default();
            let hashes: Vec<(String, String)> = rows
                .iter()
                .filter_map(|r| r.details.as_ref())
                .map(|d| (d.sha512.clone(), d.sha1.clone()))
                .collect();
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.mods.set_rows(rows);
                cx.notify();
            });
            let found = runtime::spawn(mods::identify(hashes))
                .await
                .unwrap_or_default();
            if found.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                let mut rows = this.mods.rows().to_vec();
                for row in &mut rows {
                    let Some(details) = &row.details else {
                        continue;
                    };
                    if let Some(hit) = found.get(&details.sha512) {
                        row.title = hit.title.clone().into();
                        row.icon = hit.icon.clone();
                    }
                }
                this.mods.set_rows(rows);
                this.icons_at = Some(Instant::now());
                cx.notify();
            });
        })
        .detach();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let selected = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        let view = cx.entity().downgrade();
        let name: SharedString = AppState::global(cx)
            .read(cx)
            .instance(&self.id)
            .map(|i| i.name.clone().into())
            .unwrap_or_else(|| self.name.clone());
        h_flex()
            .flex_none()
            .gap(px(12.))
            .px(px(18.))
            .py(px(14.))
            .border_b_1()
            .border_color(c.border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .items_baseline()
                    .text_size(px(17.))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .truncate()
                            .child(name.clone()),
                    )
                    .children(
                        self.version
                            .clone()
                            .map(|v| div().flex_none().text_color(c.muted).child(v)),
                    ),
            )
            .child(Tabs::new(
                "instance-tabs",
                TABS.iter().map(|t| t.label()).collect(),
                selected,
                move |ix, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.tab = TABS[ix];
                        cx.notify();
                    });
                },
            ))
            .child(actions_menu(
                self.id.clone(),
                name.to_string(),
                self.game_dir.clone(),
            ))
    }

    fn render_mods(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let dir = self.game_dir.join("mods");
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(10.))
                    .px(px(18.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(c.row)
                    .child(
                        TextField::new(&self.search)
                            .leading(IconName::Search)
                            .w(px(280.)),
                    )
                    .when(self.loading, |row| {
                        row.child(
                            h_flex()
                                .gap(px(8.))
                                .text_color(c.muted)
                                .child(motion::spinner(
                                    "mods-spinner",
                                    icon(IconName::Loader, c.muted).size(px(14.)),
                                    cx,
                                ))
                                .child(t!("mods.reading").to_string()),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("open-mods")
                            .label(t!("instance.open_folder"))
                            .on_click(move |_, _, cx| cx.open_with_system(&dir)),
                    ),
            )
            .child(self.render_table_head(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "mods",
                            self.mods.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                                range
                                    .filter_map(|ix| this.render_row(ix, window, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .size_full()
                        .track_scroll(&self.scroll),
                    )
                    .child(scrollbar(&self.scroll)),
            )
    }

    fn render_table_head(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let c = theme.colors;
        let sort = self.mods.sort();
        let arrow = move |by: SortBy| match sort {
            Some((s, desc)) if s == by => Some(if desc { " ↓" } else { " ↑" }),
            _ => None,
        };
        let head = |id: &'static str, label: String, by: SortBy, cx: &mut Context<Self>| {
            div()
                .id(id)
                .cursor_pointer()
                .hover(|s| s.text_color(c.text2))
                .child(format!("{label}{}", arrow(by).unwrap_or("")))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.mods.toggle_sort(by);
                    cx.notify();
                }))
        };
        h_flex()
            .flex_none()
            .px(px(18.))
            .py(px(9.))
            .font_family(theme.mono.clone())
            .text_size(px(11.))
            .text_color(c.muted)
            .font_weight(FontWeight::MEDIUM)
            .child(div().flex_1().child(head(
                "sort-name",
                t!("mods.name").to_uppercase(),
                SortBy::Name,
                cx,
            )))
            .child(
                div()
                    .w(px(VERSION_WIDTH))
                    .text_right()
                    .child(t!("mods.version").to_uppercase()),
            )
            .child(h_flex().w(px(MODIFIED_WIDTH)).justify_end().child(head(
                "sort-modified",
                t!("mods.modified").to_uppercase(),
                SortBy::Modified,
                cx,
            )))
    }

    fn render_row(
        &self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let c = cx.theme().colors;
        let row = self.mods.get(ix)?;
        let disabled = !row.file.enabled;
        let hover = motion::hover(
            SharedString::from(format!("mod:{}:hover", row.file.name)),
            window,
            cx,
        );
        let bg = motion::animate(
            SharedString::from(format!("mod:{}:bg", row.file.name)),
            if hover.on { c.row } else { c.row.opacity(0.) },
            window,
            cx,
        );
        let t = motion::cascade(self.shown_at, ix, window, cx);
        let icon_t = motion::cascade(self.icons_at, 0, window, cx);
        Some(
            hover
                .track(h_flex().id(ix))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px(px(18.))
                .border_t_1()
                .border_color(c.row)
                .bg(bg)
                .opacity(t)
                .pt(px((1. - t) * 6.))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(10.))
                        .child(match &row.icon {
                            Some(path) => img(path.clone())
                                .size(px(24.))
                                .flex_none()
                                .rounded(px(5.))
                                .opacity(icon_t)
                                .into_any_element(),
                            None => div()
                                .size(px(24.))
                                .flex_none()
                                .rounded(px(5.))
                                .bg(c.sel)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(IconName::Package, c.muted).size(px(14.)))
                                .into_any_element(),
                        })
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .when(disabled, |d| d.line_through().text_color(c.muted))
                                .child(row.title.clone()),
                        ),
                )
                .child(
                    div()
                        .w(px(VERSION_WIDTH))
                        .pl(px(12.))
                        .text_right()
                        .truncate()
                        .text_color(c.text2)
                        .child(row.version.clone()),
                )
                .child(
                    div()
                        .w(px(MODIFIED_WIDTH))
                        .text_right()
                        .text_color(c.muted)
                        .child(time::ago(row.file.modified)),
                ),
        )
    }
}

impl Render for InstanceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.tab {
            Tab::Mods => self.render_mods(cx).into_any_element(),
            Tab::Settings => self.settings.clone().into_any_element(),
            Tab::Logs => self.logs.clone().into_any_element(),
            other => super::app::placeholder(
                IconName::Package,
                other.label(),
                t!("common.coming_soon").into(),
                cx,
            )
            .into_any_element(),
        };
        let tab = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .child(motion::enter(
                ("instance-tab", tab),
                v_flex().flex_1().min_h_0().child(content),
                8.,
                window,
                cx,
            ))
    }
}

/// The "⋯" menu of an instance: its folder, a copy, deletion.
fn actions_menu(id: String, name: String, game_dir: PathBuf) -> impl IntoElement {
    let trigger = Button::new("instance-actions")
        .ghost()
        .icon(IconName::More)
        .tooltip(t!("instance.actions"));
    Popover::new("instance-actions-menu")
        .anchor(Anchor::TopRight)
        .offset(px(4.))
        .trigger(trigger)
        .content(move |_, window, cx| {
            let c = cx.theme().colors;
            let popover = cx.entity();
            let busy = AppState::global(cx)
                .read(cx)
                .sessions
                .get(&id)
                .is_some_and(super::session::Session::busy);
            let items: Vec<(IconName, SharedString, bool, ItemAction)> = vec![
                (
                    IconName::Folder,
                    t!("instance.open_folder").into(),
                    false,
                    ItemAction::Folder(game_dir.clone()),
                ),
                (
                    IconName::Copy,
                    t!("instance.duplicate").into(),
                    false,
                    ItemAction::Duplicate(id.clone()),
                ),
                (
                    IconName::Trash,
                    t!("instance.delete").into(),
                    busy,
                    ItemAction::Delete(id.clone(), name.clone()),
                ),
            ];
            let rows: Vec<_> = items
                .into_iter()
                .enumerate()
                .map(|(i, (glyph, label, disabled, action))| {
                    let danger = matches!(action, ItemAction::Delete(..));
                    let hover = motion::hover(("action-hover", i), window, cx);
                    let bg = motion::animate(
                        ("action-bg", i),
                        if hover.on && !disabled {
                            c.row
                        } else {
                            c.row.opacity(0.)
                        },
                        window,
                        cx,
                    );
                    let ink = if danger {
                        super::theme::danger()
                    } else {
                        c.text2
                    };
                    let popover = popover.clone();
                    hover
                        .track(h_flex().id(i))
                        .h(px(30.))
                        .px(px(8.))
                        .gap(px(10.))
                        .rounded(px(6.))
                        .bg(bg)
                        .text_color(ink)
                        .when(disabled, |r| r.opacity(0.5))
                        .when(!disabled, |r| {
                            r.cursor_pointer().on_click(move |_, window, cx| {
                                popover.update(cx, |p, cx| p.dismiss(window, cx));
                                action.run(cx);
                            })
                        })
                        .child(icon(glyph, ink))
                        .child(label)
                })
                .collect();
            let list = v_flex()
                .ui_text(cx)
                .w(px(220.))
                .p(px(4.))
                .gap(px(2.))
                .rounded(px(8.))
                .border_1()
                .border_color(c.border)
                .bg(c.panel)
                .shadow(vec![box_shadow(0., 12., 32., 0., hsla(0., 0., 0., 0.35))])
                .children(rows);
            motion::enter("actions-in", list, -6., window, cx)
        })
}

#[derive(Clone)]
enum ItemAction {
    Folder(PathBuf),
    Duplicate(String),
    Delete(String, String),
}

impl ItemAction {
    fn run(&self, cx: &mut gpui_kit::App) {
        match self {
            ItemAction::Folder(dir) => cx.open_with_system(dir),
            ItemAction::Duplicate(id) => {
                AppState::global(cx).update(cx, |s, cx| s.duplicate_instance(id, cx))
            }
            ItemAction::Delete(id, name) => {
                super::instance_settings::confirm_delete(id.clone(), name.clone(), cx)
            }
        }
    }
}
