use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, UniformListScrollHandle, WeakEntity, Window, div, img, px,
    uniform_list,
};
use riven_format::Kind;
use rust_i18n::t;

use riven_launch::instances::Instances;
use riven_launch::own::Workbench;
use riven_resolve::Plan;

use super::logs::LogsView;
use super::mods::{self, ModRow, ModsTable, Origin, SortBy};
use super::runtime;
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::time;
use super::ui::{
    ActionMenu, Button, IconName, MenuEntry, Switch, Tabs, TextField, h_flex, icon, motion,
    scrollbar, tooltip, v_flex,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Mods,
    ResourcePacks,
    Shaders,
    Configs,
    Logs,
    Screenshots,
}

const TABS: [Tab; 6] = [
    Tab::Mods,
    Tab::ResourcePacks,
    Tab::Shaders,
    Tab::Configs,
    Tab::Logs,
    Tab::Screenshots,
];

impl Tab {
    fn label(self) -> SharedString {
        match self {
            Tab::Mods => t!("instance.tabs.mods"),
            Tab::ResourcePacks => t!("instance.tabs.resourcepacks"),
            Tab::Shaders => t!("instance.tabs.shaders"),
            Tab::Configs => t!("instance.tabs.configs"),
            Tab::Logs => t!("instance.tabs.logs"),
            Tab::Screenshots => t!("instance.tabs.screenshots"),
        }
        .into()
    }

    /// The content kind a tab lists, for the tabs that list content files.
    fn content(self) -> Option<Kind> {
        match self {
            Tab::Mods => Some(Kind::Mod),
            Tab::ResourcePacks => Some(Kind::ResourcePack),
            Tab::Shaders => Some(Kind::ShaderPack),
            _ => None,
        }
    }
}

/// The game folder a content kind lives in.
fn folder(kind: Kind) -> &'static str {
    match kind {
        Kind::ResourcePack => "resourcepacks",
        Kind::ShaderPack => "shaderpacks",
        _ => "mods",
    }
}

const VERSION_WIDTH: f32 = 160.;
const MODIFIED_WIDTH: f32 = 130.;
const ACTIONS_WIDTH: f32 = 96.;
const ROW_HEIGHT: f32 = 38.;

/// Updates of the player's own mods, looked up on request.
enum Updates {
    Unchecked,
    Checking,
    Ready(Arc<Workbench>, Plan),
    Applying,
}

/// One instance: its header, tabs and the content of the open tab.
pub struct InstanceView {
    pub id: String,
    name: SharedString,
    version: Option<SharedString>,
    game_dir: PathBuf,
    tab: Tab,
    /// The content kind the table holds.
    kind: Kind,
    search: Entity<InputState>,
    mods: ModsTable,
    scroll: UniformListScrollHandle,
    logs: Entity<LogsView>,
    loading: bool,
    /// Installed from a pack, so its mods stay locked until the player allows their own.
    from_pack: bool,
    updates: Updates,
    /// The outcome of the last mod action; `true` marks a failure.
    notice: Option<(bool, SharedString)>,
    /// A removal or switch is running.
    working: bool,
    /// When the rows and the Modrinth icons arrived, for their entrance.
    shown_at: Option<Instant>,
    icons_at: Option<Instant>,
    _search: Subscription,
    _state: Subscription,
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
        let from_pack = riven_launch::own::from_pack(&game_dir);
        let mut view = Self {
            id,
            name,
            version,
            game_dir,
            tab: Tab::Mods,
            kind: Kind::Mod,
            search,
            mods: ModsTable::default(),
            scroll: UniformListScrollHandle::new(),
            logs,
            loading: true,
            from_pack,
            updates: Updates::Unchecked,
            notice: None,
            working: false,
            shown_at: None,
            icons_at: None,
            _search,
            _state: cx.observe(&AppState::global(cx), |_, _, cx| cx.notify()),
        };
        view.reload(window, cx);
        view
    }

    /// Shows another tab, reading its folder when it lists other content.
    fn select(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        if let Some(kind) = tab.content()
            && kind != self.kind
        {
            self.kind = kind;
            self.mods.set_rows(Vec::new());
            self.notice = None;
            self.reload(window, cx);
        }
        cx.notify();
    }

    /// Reads the content folder again, then fills in titles and icons from Modrinth.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.from_pack = riven_launch::own::from_pack(&self.game_dir);
        self.updates = Updates::Unchecked;
        let dir = self.game_dir.clone();
        let game_dir = dir.clone();
        let kind = self.kind;
        let id = self.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let rows = runtime::blocking(move || mods::scan_rows(&dir, folder(kind)))
                .await
                .unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                if this.kind != kind {
                    return;
                }
                if this.mods.rows().is_empty() {
                    this.shown_at = Some(Instant::now());
                }
                this.set_rows(rows.clone());
                let hint = match kind {
                    Kind::Mod => t!("mods.search_count", n = rows.len()),
                    _ => t!("mods.search_files", n = rows.len()),
                }
                .to_string();
                this.search
                    .update(cx, |s, cx| s.set_placeholder(hint, window, cx));
                cx.notify();
            });
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let described = runtime::blocking(move || {
                mods::describe(&game_dir, rows, &move |done, total| {
                    let _ = tx.send((done, total));
                })
            });
            let state = cx.update(|_, cx| AppState::global(cx)).ok();
            let mut shown = 0;
            while let Some((done, total)) = rx.recv().await {
                if done < total && done < shown + total.div_ceil(50) {
                    continue;
                }
                shown = done;
                if let Some(state) = &state {
                    let id = id.clone();
                    let _ = cx.update(|_, cx| {
                        state.update(cx, |s, cx| {
                            if done < total {
                                s.scans.insert(id, (done, total));
                            } else {
                                s.scans.remove(&id);
                            }
                            cx.notify();
                        })
                    });
                }
            }
            if let Some(state) = &state {
                let _ = cx.update(|_, cx| {
                    state.update(cx, |s, cx| {
                        if s.scans.remove(&id).is_some() {
                            cx.notify();
                        }
                    })
                });
            }
            let rows = described.await.unwrap_or_default();
            let hashes: Vec<(String, String)> = rows
                .iter()
                .filter_map(|r| r.details.as_ref())
                .map(|d| (d.sha512.clone(), d.sha1.clone()))
                .collect();
            let _ = this.update(cx, |this, cx| {
                if this.kind != kind {
                    return;
                }
                this.loading = false;
                this.set_rows(rows);
                cx.notify();
            });
            let found = runtime::spawn(mods::identify(hashes))
                .await
                .unwrap_or_default();
            if found.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if this.kind != kind {
                    return;
                }
                let mut rows = this.mods.rows().to_vec();
                for row in &mut rows {
                    let Some(details) = &row.details else {
                        continue;
                    };
                    if let Some(hit) = found.get(&details.sha512) {
                        row.title = hit.title.clone().into();
                        row.icon = hit.icon.clone();
                        row.project = Some(hit.project.clone());
                    }
                }
                this.set_rows(rows);
                this.icons_at = Some(Instant::now());
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows `rows`, marking the ones a checked update would replace.
    fn set_rows(&mut self, mut rows: Vec<ModRow>) {
        let plan = match &self.updates {
            Updates::Ready(_, plan) => Some(plan),
            _ => None,
        };
        for row in &mut rows {
            let path = format!("{}/{}", folder(self.kind), row.file.name);
            row.update = plan
                .and_then(|p| {
                    p.update
                        .iter()
                        .find(|(old, _)| old.file.path.as_str() == path)
                })
                .map(|(_, new)| new.file.path.file_name().to_owned().into());
        }
        self.mods.set_rows(rows);
    }

    /// Packs and shaders are always editable; mods only in own instances and unlocked pack ones.
    fn editable(&self, cx: &App) -> bool {
        self.kind != Kind::Mod
            || !self.from_pack
            || AppState::global(cx)
                .read(cx)
                .instance(&self.id)
                .is_some_and(|i| i.own_mods)
    }

    fn has_loader(&self, cx: &App) -> bool {
        AppState::global(cx)
            .read(cx)
            .instance(&self.id)
            .is_some_and(|i| i.loader.is_some())
    }

    fn store(&self, cx: &App) -> Option<Instances> {
        AppState::global(cx).read(cx).store.clone()
    }

    /// Runs a change to the mods in the background, then reads the folder again.
    fn change<F>(&mut self, work: F, window: &mut Window, cx: &mut Context<Self>)
    where
        F: Future<Output = Result<(), riven_launch::LaunchError>> + Send + 'static,
    {
        if self.working {
            return;
        }
        self.working = true;
        self.notice = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = runtime::spawn(work)
                .await
                .unwrap_or_else(|_| Err(riven_launch::LaunchError::Game("cancelled".into())));
            let _ = this.update_in(cx, |this, window, cx| {
                this.working = false;
                if let Err(e) = result {
                    tracing::warn!("{e}");
                    this.notice = Some((true, e.to_string().into()));
                }
                this.reload(window, cx);
            });
        })
        .detach();
    }

    fn set_enabled(&mut self, row: &ModRow, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let file = row.file.clone();
        self.change(
            async move { riven_launch::mods::set_enabled(&file, on).map(drop) },
            window,
            cx,
        );
    }

    /// Removes an own mod with its unused dependencies; asks first for hand-dropped files.
    fn remove(&mut self, row: &ModRow, window: &mut Window, cx: &mut Context<Self>) {
        match row.origin.clone() {
            Origin::Pack => {}
            Origin::Own(entry) => {
                let Some(store) = self.store(cx) else {
                    return;
                };
                let id = self.id.clone();
                self.change(
                    async move {
                        let bench = Workbench::open_own(&store, &id).await?;
                        let plan = bench.remove(&entry)?;
                        bench.apply(&plan).await.map(drop)
                    },
                    window,
                    cx,
                );
            }
            Origin::Manual => {
                let file = row.file.clone();
                let view = cx.entity().downgrade();
                super::dialogs::confirm(
                    t!("confirm.delete_mod_title", name = row.title),
                    t!("confirm.delete_mod_body"),
                    t!("mods.delete"),
                    move |window, cx| {
                        let file = file.clone();
                        let _ = view.update(cx, |this, cx| {
                            this.change(
                                async move { riven_launch::mods::delete(&file) },
                                window,
                                cx,
                            )
                        });
                    },
                    cx,
                );
            }
        }
    }

    fn check_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store(cx) else {
            return;
        };
        let id = self.id.clone();
        self.updates = Updates::Checking;
        self.notice = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let found = runtime::spawn(async move {
                let bench = Workbench::open(&store, &id).await?;
                let plan = bench.updates().await?;
                Ok::<_, riven_launch::LaunchError>((Arc::new(bench), plan))
            })
            .await
            .unwrap_or_else(|_| Err(riven_launch::LaunchError::Game("cancelled".into())));
            let _ = this.update(cx, |this, cx| {
                match found {
                    Ok((_, plan)) if plan.update.is_empty() => {
                        this.updates = Updates::Unchecked;
                        this.notice = Some((false, t!("mods.up_to_date").into()));
                    }
                    Ok((bench, plan)) => {
                        this.updates = Updates::Ready(bench, plan);
                        let rows = this.mods.rows().to_vec();
                        this.set_rows(rows);
                    }
                    Err(e) => {
                        this.updates = Updates::Unchecked;
                        this.notice = Some((true, e.to_string().into()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Updates::Ready(bench, plan) = std::mem::replace(&mut self.updates, Updates::Applying)
        else {
            return;
        };
        let n = plan.update.len();
        self.notice = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = runtime::spawn(async move { bench.apply(&plan).await.map(drop) })
                .await
                .unwrap_or_else(|_| Err(riven_launch::LaunchError::Game("cancelled".into())));
            let _ = this.update_in(cx, |this, window, cx| {
                this.notice = Some(match result {
                    Ok(()) => (false, t!("mods.updated", n = n).into()),
                    Err(e) => (true, e.to_string().into()),
                });
                this.reload(window, cx);
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
                move |ix, window, cx| {
                    let _ = view.update(cx, |this, cx| this.select(TABS[ix], window, cx));
                },
            ))
            .child({
                let id = self.id.clone();
                Button::new("instance-settings")
                    .ghost()
                    .icon(IconName::Settings)
                    .tooltip(t!("instance.tabs.settings"))
                    .on_click(move |_, window, cx| {
                        super::instance_settings::open(id.clone(), false, window, cx)
                    })
            })
    }

    /// What the toolbar offers: adding and updating own mods, or why it cannot.
    fn render_mod_actions(&self, cx: &mut Context<Self>) -> Vec<gpui_kit::AnyElement> {
        let c = cx.theme().colors;
        if !self.editable(cx) {
            return vec![
                h_flex()
                    .gap(px(6.))
                    .text_color(c.muted)
                    .child(icon(IconName::Lock, c.muted).size(px(14.)))
                    .child(t!("mods.locked").to_string())
                    .into_any_element(),
            ];
        }
        let mut out = Vec::new();
        let has_own = self
            .mods
            .rows()
            .iter()
            .any(|r| matches!(r.origin, Origin::Own(_)));
        if has_own {
            let (label, busy) = match &self.updates {
                Updates::Unchecked => (t!("mods.check_updates"), false),
                Updates::Checking => (t!("mods.checking_updates"), true),
                Updates::Ready(_, plan) => (t!("mods.update_n", n = plan.update.len()), false),
                Updates::Applying => (t!("mods.updating"), true),
            };
            let ready = matches!(self.updates, Updates::Ready(..));
            let button = Button::new("own-updates")
                .icon(IconName::Refresh)
                .label(label)
                .disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if ready {
                        this.apply_updates(window, cx)
                    } else {
                        this.check_updates(window, cx)
                    }
                }));
            out.push(if ready { button.primary() } else { button }.into_any_element());
        }
        let can_add = self.kind != Kind::Mod || self.has_loader(cx);
        let view = cx.entity().downgrade();
        let id = self.id.clone();
        let kind = self.kind;
        out.push(
            Button::new("add-mods")
                .primary()
                .icon(IconName::Plus)
                .label(t!("mods.add"))
                .disabled(!can_add)
                .when(!can_add, |b| b.tooltip(t!("mods.needs_loader")))
                .on_click(move |_, window, cx| {
                    super::add_mods::open(id.clone(), kind, view.clone(), window, cx)
                })
                .into_any_element(),
        );
        out
    }

    fn render_mods(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let dir = self.game_dir.join(folder(self.kind));
        let busy = self.working;
        let actions = self.render_mod_actions(cx);
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
                    .when(busy, |row| {
                        row.child(h_flex().flex_none().gap(px(8.)).text_color(c.muted).child(
                            motion::spinner(
                                "mods-spinner",
                                icon(IconName::Loader, c.muted).size(px(14.)),
                                cx,
                            ),
                        ))
                    })
                    .child(div().flex_1().min_w_0().truncate().when_some(
                        self.notice.clone(),
                        |d, (failed, text)| {
                            d.text_color(if failed { c.warn } else { c.muted })
                                .child(text)
                        },
                    ))
                    .children(actions)
                    .child(
                        Button::new("open-mods")
                            .ghost()
                            .icon(IconName::Folder)
                            .tooltip(t!("instance.open_folder"))
                            .on_click(move |_, _, cx| {
                                let _ = std::fs::create_dir_all(&dir);
                                cx.open_with_system(&dir)
                            }),
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
                    .child(scrollbar(&self.scroll))
                    .when(self.mods.len() == 0 && !self.loading, |area| {
                        area.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(c.muted)
                                .child(
                                    match self.kind {
                                        Kind::ResourcePack => t!("mods.empty_resourcepacks"),
                                        Kind::ShaderPack => t!("mods.empty_shaders"),
                                        _ => t!("mods.empty_mods"),
                                    }
                                    .to_string(),
                                ),
                        )
                    }),
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
            .child(div().w(px(ACTIONS_WIDTH)))
    }

    /// A row's menu: its page and file, plus switch and removal for the player's own files.
    fn row_menu(&self, row: &ModRow, view: WeakEntity<Self>, cx: &App) -> Vec<MenuEntry> {
        let mut entries = Vec::new();
        if let Some(project) = row.project.clone() {
            entries.push(
                MenuEntry::action(t!("mods.open_page"), move |_, cx| {
                    cx.open_url(&format!("https://modrinth.com/project/{project}"))
                })
                .icon(IconName::ExternalLink),
            );
        }
        let path = row.file.path.clone();
        entries.push(
            MenuEntry::action(t!("mods.show_file"), move |_, cx| cx.reveal_path(&path))
                .icon(IconName::Folder),
        );
        if row.origin == Origin::Pack || !self.editable(cx) {
            return entries;
        }
        let on = !row.file.enabled;
        let (toggled, removed, remove_view) = (row.clone(), row.clone(), view.clone());
        entries.push(
            MenuEntry::action(
                if on {
                    t!("mods.enable")
                } else {
                    t!("mods.disable")
                },
                move |window, cx| {
                    let _ = view.update(cx, |this, cx| this.set_enabled(&toggled, on, window, cx));
                },
            )
            .icon(IconName::Power)
            .disabled(self.working),
        );
        entries.push(MenuEntry::Separator);
        entries.push(
            MenuEntry::action(t!("mods.delete"), move |window, cx| {
                let _ = remove_view.update(cx, |this, cx| this.remove(&removed, window, cx));
            })
            .icon(IconName::Trash)
            .danger()
            .disabled(self.working),
        );
        entries
    }

    /// The switch, or a lock for files the pack manages, and the "⋯" menu of a row.
    fn render_row_actions(
        &self,
        ix: usize,
        row: &ModRow,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let c = cx.theme().colors;
        let view = cx.entity().downgrade();
        let menu = ActionMenu::new(
            ("mod-menu", ix),
            Button::new(("mod-more", ix))
                .ghost()
                .icon(IconName::More)
                .tooltip(t!("mods.actions")),
            self.row_menu(row, view.clone(), cx),
        );
        let cell = h_flex().w(px(ACTIONS_WIDTH)).justify_end().gap(px(6.));
        let lead = if row.origin == Origin::Pack || !self.editable(cx) {
            div()
                .id(("pack-lock", ix))
                .px(px(7.))
                .child(icon(IconName::Lock, c.muted).size(px(14.)))
                .tooltip(tooltip(t!("mods.from_pack").into()))
                .into_any_element()
        } else {
            let toggled = row.clone();
            Switch::new(("mod-switch", ix), row.file.enabled)
                .accessible(row.title.clone())
                .on_change(move |on, window, cx| {
                    let _ = view.update(cx, |this, cx| this.set_enabled(&toggled, on, window, cx));
                })
                .into_any_element()
        };
        cell.child(lead).child(menu).into_any_element()
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
        let actions = self.render_row_actions(ix, row, cx);
        let menu = {
            let row = row.clone();
            let view = cx.entity().downgrade();
            move |event: &gpui_kit::MouseDownEvent, _: &mut Window, cx: &mut App| {
                let Ok(entries) =
                    view.read_with(cx, |this, cx| this.row_menu(&row, view.clone(), cx))
                else {
                    return;
                };
                AppState::global(cx)
                    .update(cx, |s, cx| s.open_context_menu(event.position, entries, cx));
            }
        };
        let own_badge = self.from_pack && matches!(row.origin, Origin::Own(_));
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
                .on_mouse_down(MouseButton::Right, menu)
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
                        )
                        .when(own_badge, |h| {
                            h.child(
                                div()
                                    .flex_none()
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .bg(c.sel)
                                    .text_size(px(11.))
                                    .text_color(c.text2)
                                    .child(t!("mods.own").to_string()),
                            )
                        }),
                )
                .child(match row.update.clone() {
                    Some(next) => div()
                        .id(("mod-update", ix))
                        .w(px(VERSION_WIDTH))
                        .pl(px(12.))
                        .text_right()
                        .truncate()
                        .text_color(c.accent)
                        .child(format!("↑ {}", row.version))
                        .tooltip(tooltip(next))
                        .into_any_element(),
                    None => div()
                        .w(px(VERSION_WIDTH))
                        .pl(px(12.))
                        .text_right()
                        .truncate()
                        .text_color(c.text2)
                        .child(row.version.clone())
                        .into_any_element(),
                })
                .child(
                    div()
                        .w(px(MODIFIED_WIDTH))
                        .text_right()
                        .text_color(c.muted)
                        .child(time::ago(row.file.modified)),
                )
                .child(actions),
        )
    }
}

impl Render for InstanceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.tab {
            Tab::Mods | Tab::ResourcePacks | Tab::Shaders => {
                self.render_mods(cx).into_any_element()
            }
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
