use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, img, px, relative,
};
use gpui_base::input::{InputEvent, InputState};
use moonglass::theme::ActiveTheme as _;
use moonglass::toast::ToastKind;
use moonglass::{
    ActionMenu, Button, ButtonSize, IconName, MenuEntry, Modal, Switch, Tabs, TextField, Tone,
    UiText as _, W_SEMIBOLD, caption, chip, h_flex, icon, motion, nav_row, placeholder, v_flex,
};

use crate::add::AddDialog;
use crate::assets;
use crate::data::{
    Hit, INSTANCES, Instance, Item, LOG, MOD_HITS, PACK_HITS, SHADER_HITS, STEPS, project_page,
};
use crate::settings::{Page, Prefs};

pub const SIDEBAR_WIDTH: f32 = 240.;
const VERSION_WIDTH: f32 = 160.;
const MODIFIED_WIDTH: f32 = 130.;
const ACTIONS_WIDTH: f32 = 96.;
const ROW_HEIGHT: f32 = 38.;
const MENU_WIDTH: f32 = 200.;

pub const DOWNLOAD_URL: &str = "https://bxteam.org/downloads/riven";

const TABS: [&str; 7] = [
    "Mods",
    "Resource packs",
    "Shaders",
    "Configs",
    "Logs",
    "Worlds",
    "Screenshots",
];
const MODS_TAB: usize = 0;
const LOGS_TAB: usize = 4;
const WORLDS_TAB: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Instance(usize),
    Settings(Page),
    Developer,
}

/// The content tabs: each lists files of one kind.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mods,
    ResourcePacks,
    Shaders,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Mods, Kind::ResourcePacks, Kind::Shaders];

    fn of_tab(tab: usize) -> Option<Kind> {
        Kind::ALL.get(tab).copied()
    }

    fn index(self) -> usize {
        self as usize
    }

    pub fn noun(self) -> &'static str {
        match self {
            Kind::Mods => "mods",
            Kind::ResourcePacks => "resource packs",
            Kind::Shaders => "shaders",
        }
    }

    fn folder(self) -> &'static str {
        match self {
            Kind::Mods => "mods",
            Kind::ResourcePacks => "resourcepacks",
            Kind::Shaders => "shaderpacks",
        }
    }

    pub fn hits(self) -> &'static [Hit] {
        match self {
            Kind::Mods => MOD_HITS,
            Kind::ResourcePacks => PACK_HITS,
            Kind::Shaders => SHADER_HITS,
        }
    }
}

#[derive(Clone)]
struct Row {
    item: Item,
    on: bool,
}

#[derive(Clone, PartialEq)]
enum Phase {
    Idle,
    Working { step: usize },
    Running,
}

/// The launcher's main window with nothing behind it: the same layout, made-up instances.
pub struct Demo {
    route: Route,
    /// Where Settings' "Back" returns to.
    pub(crate) last_instance: usize,
    tab: usize,
    search: Entity<InputState>,
    filter: String,
    /// The files of each instance, per content kind; the player can add, switch and delete them.
    content: Vec<[Vec<Row>; 3]>,
    /// Backups made of each instance's worlds.
    backups: Vec<Vec<u32>>,
    /// The instance being launched or running, and how far it got.
    launch: Option<(usize, Phase)>,
    /// The game output of the last launch, and the instance it came from.
    log: Option<(usize, Vec<&'static str>)>,
    modal: Option<Modal>,
    modal_serial: u64,
    pub(crate) prefs: Prefs,
    _search: Subscription,
}

impl Demo {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let _search = cx.subscribe(&search, |this, input, event, cx| {
            if let InputEvent::Change = event {
                this.filter = input.read(cx).value().to_lowercase();
                cx.notify();
            }
        });
        let rows = |items: &[Item]| {
            items
                .iter()
                .map(|item| Row {
                    item: item.clone(),
                    on: true,
                })
                .collect::<Vec<_>>()
        };
        Self {
            route: Route::Instance(0),
            last_instance: 0,
            tab: MODS_TAB,
            search,
            filter: String::new(),
            content: INSTANCES
                .iter()
                .map(|i| [rows(i.mods), rows(i.resource_packs), rows(i.shaders)])
                .collect(),
            backups: INSTANCES
                .iter()
                .map(|i| i.worlds.iter().map(|w| w.backups).collect())
                .collect(),
            launch: None,
            log: None,
            modal: None,
            modal_serial: 0,
            prefs: Prefs::new(window, cx),
            _search,
        }
    }

    pub(crate) fn navigate(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        if self.route == route {
            return;
        }
        if let Route::Instance(ix) = route {
            self.last_instance = ix;
        }
        let same_instance =
            matches!((self.route, route), (Route::Instance(a), Route::Instance(b)) if a == b);
        self.route = route;
        if !same_instance {
            self.tab = MODS_TAB;
        }
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn rows(&self, ix: usize, kind: Kind) -> &[Row] {
        &self.content[ix][kind.index()]
    }

    /// Whether an instance already has a file by this name.
    pub fn has(&self, ix: usize, kind: Kind, name: &str) -> bool {
        self.rows(ix, kind).iter().any(|r| r.item.name == name)
    }

    pub fn add(&mut self, ix: usize, kind: Kind, hit: &Hit, cx: &mut Context<Self>) {
        if self.has(ix, kind, hit.name) {
            return;
        }
        let rows = &mut self.content[ix][kind.index()];
        rows.push(Row {
            item: Item::added(hit),
            on: true,
        });
        rows.sort_by_key(|r| r.item.name.to_lowercase());
        cx.notify();
    }

    fn remove(&mut self, ix: usize, kind: Kind, name: &'static str, cx: &mut Context<Self>) {
        self.content[ix][kind.index()].retain(|r| r.item.name != name);
        moonglass::toast::show(ToastKind::Success, format!("{name} deleted"), cx);
        cx.notify();
    }

    fn set_on(&mut self, ix: usize, kind: Kind, name: &str, on: bool, cx: &mut Context<Self>) {
        if let Some(row) = self.content[ix][kind.index()]
            .iter_mut()
            .find(|r| r.item.name == name)
        {
            row.on = on;
        }
        cx.notify();
    }

    fn open_add(&mut self, ix: usize, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let demo = cx.entity().downgrade();
        let view = cx.new(|cx| AddDialog::new(demo, ix, kind, window, cx));
        self.modal_serial += 1;
        self.modal = Some(Modal {
            view: view.into(),
            width: px(640.),
            focus: cx.focus_handle(),
            serial: self.modal_serial,
            closing: false,
        });
        cx.notify();
    }

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        self.modal = None;
        cx.notify();
    }

    fn phase(&self, ix: usize) -> Phase {
        match &self.launch {
            Some((at, phase)) if *at == ix => phase.clone(),
            _ => Phase::Idle,
        }
    }

    /// Walks through the steps a real launch reports, then "runs" and prints a log.
    fn play(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.launch.is_some() {
            return;
        }
        self.launch = Some((ix, Phase::Working { step: 0 }));
        self.log = Some((ix, Vec::new()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            for step in 1..STEPS.len() {
                cx.background_executor()
                    .timer(Duration::from_millis(650))
                    .await;
                let going = this.update(cx, |this, cx| {
                    let on = matches!(this.launch, Some((at, Phase::Working { .. })) if at == ix);
                    if on {
                        this.launch = Some((ix, Phase::Working { step }));
                        cx.notify();
                    }
                    on
                });
                if !matches!(going, Ok(true)) {
                    return;
                }
            }
            cx.background_executor()
                .timer(Duration::from_millis(650))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.launch = Some((ix, Phase::Running));
                cx.notify();
            });
            for line in LOG {
                cx.background_executor()
                    .timer(Duration::from_millis(180))
                    .await;
                let running = this.update(cx, |this, cx| {
                    let on = matches!(this.launch, Some((at, Phase::Running)) if at == ix);
                    if on && let Some((_, log)) = &mut this.log {
                        log.push(line);
                        cx.notify();
                    }
                    on
                });
                if !matches!(running, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.launch = None;
        cx.notify();
    }

    fn render_title(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        h_flex()
            .flex_none()
            .h(px(40.))
            .px(px(12.))
            .gap(px(8.))
            .bg(c.panel)
            .border_b_1()
            .border_color(c.border)
            .child(img(assets::LOGO).size(px(18.)).rounded(px(4.)))
            .child(
                div()
                    .font_weight(W_SEMIBOLD)
                    .text_size(px(12.5))
                    .child("Riven Launcher"),
            )
            .child(
                div()
                    .px(px(6.))
                    .rounded(px(4.))
                    .bg(c.sel)
                    .text_size(px(11.))
                    .text_color(c.text2)
                    .child("Demo"),
            )
            .child(div().flex_1())
            .child(
                Button::new("get-riven")
                    .size(ButtonSize::Xs)
                    .icon(IconName::Download)
                    .label("Get Riven")
                    .on_click(|_, _, cx| cx.open_url(DOWNLOAD_URL)),
            )
    }

    fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let selected = match self.route {
            Route::Instance(ix) => Some(ix),
            _ => None,
        };
        let mut rows: Vec<AnyElement> = INSTANCES
            .iter()
            .enumerate()
            .map(|(ix, instance)| {
                let has_pack = instance.pack.is_some();
                nav_row(("instance", ix), selected == Some(ix), window, cx)
                    .child(
                        div()
                            .size(px(8.))
                            .mx(px(5.))
                            .flex_none()
                            .rounded(px(4.))
                            .map(|d| {
                                if has_pack {
                                    d.bg(c.ok)
                                } else {
                                    d.border_1().border_color(c.muted)
                                }
                            }),
                    )
                    .child(div().truncate().child(instance.name))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.navigate(Route::Instance(ix), window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        rows.push(
            nav_row("new-instance", false, window, cx)
                .text_color(c.muted)
                .child("+ New instance")
                .on_click(|_, _, cx| unavailable("Creating instances", cx))
                .into_any_element(),
        );
        let list = motion::highlighted(
            "sidebar",
            v_flex().gap(px(2.)),
            rows,
            selected,
            |d| d.rounded(px(6.)).bg(c.sel),
            window,
            cx,
        );
        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .bg(c.panel)
            .border_r_1()
            .border_color(c.border)
            .px(px(8.))
            .py(px(12.))
            .gap(px(2.))
            .child(caption("INSTANCES", cx))
            .child(
                div()
                    .id("instances")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                nav_row("open-developer", self.route == Route::Developer, window, cx)
                    .child(icon(IconName::Code, c.text2))
                    .child("Developer")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.navigate(Route::Developer, window, cx)
                    })),
            )
            .child(
                nav_row("open-settings", false, window, cx)
                    .child(icon(IconName::Settings, c.text2))
                    .child("Settings")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.navigate(Route::Settings(Page::General), window, cx)
                    })),
            )
    }

    fn render_instance(
        &self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let instance = &INSTANCES[ix];
        let view = cx.entity().downgrade();
        let header = h_flex()
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
                            .child(instance.name),
                    )
                    .children(
                        instance
                            .pack
                            .map(|v| div().flex_none().text_color(c.muted).child(v)),
                    ),
            )
            .child(Tabs::new(
                "instance-tabs",
                TABS.iter().map(|t| SharedString::from(*t)).collect(),
                self.tab,
                move |tab, window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.tab = tab;
                        this.search
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        cx.notify();
                    });
                },
            ))
            .child(
                Button::new("instance-settings")
                    .ghost()
                    .icon(IconName::Settings)
                    .tooltip("Instance settings")
                    .on_click(|_, _, cx| unavailable("Instance settings", cx)),
            );
        let content = match (self.tab, Kind::of_tab(self.tab)) {
            (_, Some(kind)) => self
                .render_content(instance, ix, kind, window, cx)
                .into_any_element(),
            (LOGS_TAB, _) => self.render_log(ix, cx).into_any_element(),
            (WORLDS_TAB, _) => self.render_worlds(ix, window, cx).into_any_element(),
            (tab, _) => placeholder(
                IconName::Package,
                TABS[tab].into(),
                "This part of the launcher works on your own files, so it is only in the app."
                    .into(),
                cx,
            )
            .child(get_riven().mt(px(8.)))
            .into_any_element(),
        };
        v_flex().size_full().child(header).child(motion::enter(
            ("instance-tab", self.tab),
            v_flex().flex_1().min_h_0().child(content),
            8.,
            window,
            cx,
        ))
    }

    fn render_content(
        &self,
        instance: &Instance,
        ix: usize,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let theme = cx.theme();
        let filter = self.filter.clone();
        let all = self.rows(ix, kind);
        let shown: Vec<usize> = all
            .iter()
            .enumerate()
            .filter(|(_, r)| filter.is_empty() || r.item.name.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect();
        let locked = kind == Kind::Mods && instance.pack.is_some();
        let can_add = kind != Kind::Mods || instance.loader.is_some();
        let toolbar = h_flex()
            .flex_none()
            .gap(px(10.))
            .px(px(18.))
            .py(px(10.))
            .border_b_1()
            .border_color(c.row)
            .child(
                TextField::new(&self.search)
                    .leading(IconName::Search)
                    .clearable()
                    .w(px(280.)),
            )
            .child(div().flex_1())
            .when(locked, |row| {
                row.child(
                    h_flex()
                        .gap(px(6.))
                        .text_color(c.muted)
                        .child(icon(IconName::Lock, c.muted).size(px(14.)))
                        .child("Mods come from the pack"),
                )
            })
            .child(
                Button::new("add-content")
                    .primary()
                    .icon(IconName::Plus)
                    .label(format!("Add {}", kind.noun()))
                    .disabled(!can_add)
                    .when(!can_add, |b| {
                        b.tooltip("Vanilla instances cannot have mods; pick a loader in Settings")
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open_add(ix, kind, window, cx)),
                    ),
            );
        let head = h_flex()
            .flex_none()
            .px(px(18.))
            .py(px(9.))
            .font_family(theme.mono.clone())
            .text_size(px(11.))
            .text_color(c.muted)
            .font_weight(FontWeight::MEDIUM)
            .child(div().flex_1().child("NAME"))
            .child(div().w(px(VERSION_WIDTH)).text_right().child("VERSION"))
            .child(div().w(px(MODIFIED_WIDTH)).text_right().child("MODIFIED"))
            .child(div().w(px(ACTIONS_WIDTH)));
        let rows: Vec<AnyElement> = shown
            .iter()
            .map(|&m| self.render_row(ix, kind, m, window, cx))
            .collect();
        let empty = rows.is_empty();
        let nothing = all.is_empty();
        let hint = match kind {
            Kind::Mods if !can_add => "Vanilla runs without mods. Pick a loader to add some.",
            Kind::Mods => "No mods. Add some, or install a pack.",
            Kind::ResourcePacks => "No resource packs yet.",
            Kind::Shaders if instance.loader.is_none() => {
                "No shaders yet. They need Iris, which runs on a loader."
            }
            Kind::Shaders => "No shaders yet.",
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .child(head)
            .child(
                div()
                    .id(("content", kind.index()))
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(empty, |area| {
                        area.child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(c.muted)
                                .child(if nothing {
                                    hint
                                } else {
                                    "Nothing matches the search."
                                }),
                        )
                    }),
            )
    }

    /// A row's menu: its page and file, plus switch and removal for the player's own files.
    fn row_menu(&self, ix: usize, kind: Kind, row: &Row, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let item = &row.item;
        let mut entries = Vec::new();
        if let Some(page) = item.icon.and_then(project_page) {
            entries.push(
                MenuEntry::action("Open on Modrinth", move |_, cx| cx.open_url(&page))
                    .icon(IconName::ExternalLink),
            );
        }
        let (name, folder) = (item.name, kind.folder());
        entries.push(
            MenuEntry::action("Show the file", move |_, cx| {
                moonglass::toast::show(
                    ToastKind::Info,
                    format!("The app opens {folder}/ in your file manager with {name} selected."),
                    cx,
                )
            })
            .icon(IconName::Folder),
        );
        if item.from_pack {
            return entries;
        }
        let view = cx.entity().downgrade();
        let removing = view.clone();
        let on = !row.on;
        entries.push(
            MenuEntry::action(if on { "Enable" } else { "Disable" }, move |_, cx| {
                let _ = view.update(cx, |this, cx| this.set_on(ix, kind, name, on, cx));
            })
            .icon(IconName::Power),
        );
        entries.push(MenuEntry::Separator);
        entries.push(
            MenuEntry::action("Delete", move |_, cx| {
                let _ = removing.update(cx, |this, cx| this.remove(ix, kind, name, cx));
            })
            .icon(IconName::Trash)
            .danger(),
        );
        entries
    }

    fn render_row(
        &self,
        ix: usize,
        kind: Kind,
        m: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let row = &self.rows(ix, kind)[m];
        let item = &row.item;
        let on = row.on;
        let key = format!("item:{ix}:{}:{}", kind.index(), item.name);
        let hover = motion::hover(SharedString::from(format!("{key}:hover")), window, cx);
        let bg = motion::animate(
            SharedString::from(format!("{key}:bg")),
            if hover.on { c.row } else { c.row.opacity(0.) },
            window,
            cx,
        );
        let lead = if item.from_pack {
            div()
                .id(("pack-lock", m))
                .px(px(7.))
                .child(icon(IconName::Lock, c.muted).size(px(14.)))
                .tooltip(moonglass::tooltip("Installed by the pack".into()))
                .into_any_element()
        } else {
            let view = cx.entity().downgrade();
            let name = item.name;
            Switch::new(("item-switch", m), on)
                .accessible(item.name)
                .on_change(move |on, _, cx| {
                    let _ = view.update(cx, |this, cx| this.set_on(ix, kind, name, on, cx));
                })
                .into_any_element()
        };
        let menu = ActionMenu::new(
            ("item-menu", m),
            Button::new(("item-more", m))
                .ghost()
                .icon(IconName::More)
                .tooltip("Actions"),
            self.row_menu(ix, kind, row, cx),
        )
        .width(px(MENU_WIDTH));
        let own_badge = kind == Kind::Mods && INSTANCES[ix].pack.is_some() && !item.from_pack;
        hover
            .track(h_flex().id(("item", m)))
            .w_full()
            .h(px(ROW_HEIGHT))
            .px(px(18.))
            .border_t_1()
            .border_color(c.row)
            .bg(bg)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(10.))
                    .child(
                        div()
                            .size(px(24.))
                            .flex_none()
                            .rounded(px(5.))
                            .bg(c.sel)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(c.accent)
                            .map(|tile| match item.icon {
                                Some(url) => tile.child(img(url).size(px(24.)).rounded(px(5.))),
                                None => tile.child(initials(item.name)),
                            }),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .when(!on, |d| d.line_through().text_color(c.muted))
                            .child(item.name),
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
                                .child("own"),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(VERSION_WIDTH))
                    .pl(px(12.))
                    .text_right()
                    .truncate()
                    .text_color(c.text2)
                    .child(item.version),
            )
            .child(
                div()
                    .w(px(MODIFIED_WIDTH))
                    .text_right()
                    .text_color(c.muted)
                    .child(item.modified),
            )
            .child(
                h_flex()
                    .w(px(ACTIONS_WIDTH))
                    .justify_end()
                    .gap(px(6.))
                    .child(lead)
                    .child(menu),
            )
            .into_any_element()
    }

    fn render_worlds(
        &self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let worlds = INSTANCES[ix].worlds;
        if worlds.is_empty() {
            return placeholder(
                IconName::Globe,
                "No worlds yet".into(),
                "They appear here after the first game.".into(),
                cx,
            )
            .into_any_element();
        }
        let running = self.phase(ix) != Phase::Idle;
        let cards = worlds.iter().enumerate().map(|(w, world)| {
            let backups = self.backups[ix][w];
            let key = format!("world:{ix}:{w}");
            let hover = motion::hover(SharedString::from(format!("{key}:hover")), window, cx);
            let border = motion::animate(
                SharedString::from(format!("{key}:border")),
                if hover.on { c.border } else { c.row },
                window,
                cx,
            );
            let mode_tone = match world.mode {
                "Hardcore" => Tone::Danger,
                "Creative" => Tone::Accent,
                _ => Tone::Ok,
            };
            let mut chips = vec![chip(world.mode, mode_tone, cx)];
            if world.mode != "Hardcore" {
                chips.push(chip(world.difficulty, Tone::Muted, cx));
            }
            chips.push(chip(INSTANCES[ix].minecraft, Tone::Muted, cx));
            chips.push(chip(format!("day {}", world.day), Tone::Muted, cx));
            let meta = format!(
                "played {} · {} · {}",
                world.played,
                world.size,
                match backups {
                    0 => "no backups".to_owned(),
                    1 => "1 backup".to_owned(),
                    n => format!("{n} backups"),
                }
            );
            let name = world.name;
            let seed = 4_206_913_377_u64.wrapping_mul(w as u64 + ix as u64 + 1);
            let entries = vec![
                MenuEntry::action("Copy seed", move |_, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(seed.to_string()));
                    moonglass::toast::show(ToastKind::Success, format!("Seed {seed} copied"), cx)
                })
                .icon(IconName::Copy),
                MenuEntry::action("Copy to instance", move |_, cx| {
                    unavailable("Copying worlds", cx)
                })
                .icon(IconName::FolderPlus),
                MenuEntry::Separator,
                MenuEntry::action("Delete world", move |_, cx| {
                    unavailable("Deleting worlds", cx)
                })
                .icon(IconName::Trash)
                .danger()
                .disabled(running),
            ];
            hover
                .track(h_flex().id(("world", w)))
                .gap(px(14.))
                .mx(px(18.))
                .px(px(14.))
                .py(px(12.))
                .rounded(px(10.))
                .border_1()
                .border_color(border)
                .bg(c.panel)
                .child(
                    div()
                        .size(px(48.))
                        .flex_none()
                        .rounded(px(9.))
                        .bg(c.sel)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(IconName::Globe, c.muted).size(px(22.))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(5.))
                        .child(
                            div()
                                .font_weight(W_SEMIBOLD)
                                .text_size(px(14.))
                                .truncate()
                                .child(name),
                        )
                        .child(h_flex().gap(px(5.)).flex_wrap().children(chips))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(c.muted)
                                .truncate()
                                .child(meta),
                        ),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap(px(6.))
                        .child(
                            Button::new(("world-backup", w))
                                .outline()
                                .icon(IconName::Plus)
                                .label("Back up")
                                .tooltip("Save a backup of the world; the game may be running")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.backups[ix][w] += 1;
                                    moonglass::toast::show(
                                        ToastKind::Success,
                                        format!("{name} backed up"),
                                        cx,
                                    );
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("world-backups", w))
                                .ghost()
                                .icon(IconName::History)
                                .tooltip(format!("Backups ({backups})"))
                                .on_click(|_, _, cx| unavailable("Restoring backups", cx)),
                        )
                        .child(
                            ActionMenu::new(
                                ("world-menu", w),
                                Button::new(("world-more", w))
                                    .ghost()
                                    .icon(IconName::More)
                                    .tooltip("Actions"),
                                entries,
                            )
                            .width(px(MENU_WIDTH)),
                        ),
                )
        });
        v_flex()
            .id("worlds")
            .size_full()
            .overflow_y_scroll()
            .py(px(14.))
            .gap(px(10.))
            .child(
                div()
                    .px(px(18.))
                    .text_color(c.muted)
                    .child(format!("Worlds: {}", worlds.len())),
            )
            .children(cards)
            .into_any_element()
    }

    fn render_log(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let lines = match &self.log {
            Some((at, lines)) if *at == ix && !lines.is_empty() => lines.clone(),
            _ => Vec::new(),
        };
        if lines.is_empty() {
            return placeholder(
                IconName::Terminal,
                "No log yet".into(),
                "Press Play: the game's output appears here as it runs.".into(),
                cx,
            )
            .into_any_element();
        }
        div()
            .id("log")
            .size_full()
            .overflow_y_scroll()
            .px(px(18.))
            .py(px(12.))
            .font_family(mono)
            .text_size(px(12.))
            .line_height(relative(1.6))
            .text_color(c.text2)
            .children(lines.into_iter().map(|line| div().child(line)))
            .into_any_element()
    }

    fn render_bar(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let instance = &INSTANCES[ix];
        let loader = match instance.loader {
            Some((name, version)) => format!("{} · {name} {version}", instance.minecraft),
            None => format!("{} · Vanilla", instance.minecraft),
        };
        let line = |text: String, color| {
            div()
                .text_size(px(12.))
                .text_color(color)
                .truncate()
                .child(text)
        };
        let phase = self.phase(ix);
        let status = match &phase {
            Phase::Idle => match instance.pack {
                Some(version) => line(format!("{version} — the latest release"), c.ok),
                None => line(instance.played.to_owned(), c.muted),
            }
            .into_any_element(),
            Phase::Working { step } => {
                let done = (*step + 1) as f32 / STEPS.len() as f32;
                v_flex()
                    .gap(px(6.))
                    .child(line(STEPS[*step].to_owned(), c.text2))
                    .child(
                        div().h(px(4.)).w_full().rounded(px(2.)).bg(c.row).child(
                            div()
                                .h_full()
                                .w(relative(done))
                                .rounded(px(2.))
                                .bg(c.accent),
                        ),
                    )
                    .into_any_element()
            }
            Phase::Running => {
                line("Running · the log is in the Logs tab".to_owned(), c.ok).into_any_element()
            }
        };
        let button = match phase {
            Phase::Idle => Button::new("play")
                .primary()
                .size(ButtonSize::Lg)
                .icon(IconName::Play)
                .label("Play")
                .disabled(self.launch.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.play(ix, cx))),
            Phase::Working { .. } => Button::new("cancel")
                .outline()
                .size(ButtonSize::Lg)
                .icon(IconName::Close)
                .label("Cancel")
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
            Phase::Running => Button::new("stop")
                .outline()
                .size(ButtonSize::Lg)
                .icon(IconName::Stop)
                .label("Stop")
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
        };
        let mark = if instance.loader.is_some() {
            assets::FABRIC
        } else {
            assets::VANILLA
        };
        h_flex()
            .flex_none()
            .min_h(px(72.))
            .gap(px(20.))
            .px(px(18.))
            .py(px(12.))
            .border_t_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(12.))
                    .child(
                        div()
                            .size(px(44.))
                            .flex_none()
                            .rounded(px(9.))
                            .bg(c.sel)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(IconName::Custom(mark), c.accent).size(px(22.))),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(W_SEMIBOLD)
                                    .text_size(px(14.))
                                    .truncate()
                                    .child(instance.name),
                            )
                            .child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(12.))
                                    .text_color(c.muted)
                                    .truncate()
                                    .child(loader),
                            ),
                    ),
            )
            .child(div().flex_1().min_w_0().child(status))
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(10.))
                    .child(
                        Button::new("account")
                            .outline()
                            .h(px(40.))
                            .rounded(px(8.))
                            .gap(px(8.))
                            .child(
                                div()
                                    .size(px(22.))
                                    .rounded(px(5.))
                                    .bg(c.sel)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(c.accent)
                                    .child("S"),
                            )
                            .child(div().child("Steve"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(Route::Settings(Page::Accounts), window, cx)
                            })),
                    )
                    .child(button),
            )
    }
}

/// A screen the demo has no data for, with the way to the real thing.
fn get_riven() -> Button {
    Button::new("get-riven-inline")
        .primary()
        .size(ButtonSize::Md)
        .icon(IconName::Download)
        .label("Download Riven")
        .on_click(|_, _, cx| cx.open_url(DOWNLOAD_URL))
}

fn unavailable(what: &str, cx: &mut App) {
    moonglass::toast::show(
        ToastKind::Info,
        format!("{what} works on your own files, so it is only in the app."),
        cx,
    );
}

fn initials(name: &str) -> SharedString {
    name.split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
        .into()
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        moonglass::theme::tick(window, cx);
        let c = cx.theme().colors;
        let (side, content) = match self.route {
            Route::Instance(ix) => (
                self.render_sidebar(window, cx).into_any_element(),
                self.render_instance(ix, window, cx).into_any_element(),
            ),
            Route::Settings(page) => (
                self.render_settings_nav(page, window, cx)
                    .into_any_element(),
                self.render_settings(page, window, cx).into_any_element(),
            ),
            Route::Developer => (
                self.render_sidebar(window, cx).into_any_element(),
                placeholder(
                    IconName::Code,
                    "Developer mode".into(),
                    "The content table, dependency tree, config editor, releases and git for your own pack."
                        .into(),
                    cx,
                )
                .child(get_riven().mt(px(8.)))
                .into_any_element(),
            ),
        };
        let route_key = SharedString::from(match self.route {
            Route::Instance(ix) => format!("route:instance:{ix}"),
            Route::Settings(_) => "route:settings".into(),
            Route::Developer => "route:developer".into(),
        });
        let body = h_flex()
            .size_full()
            .items_start()
            .child(side)
            .child(motion::enter(
                route_key,
                div().flex_1().min_w_0().h_full().child(content),
                8.,
                window,
                cx,
            ));
        let bar = match self.route {
            Route::Instance(ix) => Some(self.render_bar(ix, cx)),
            _ => None,
        };
        let toast_bottom = if bar.is_some() { 88. } else { 16. };
        let view = cx.entity().downgrade();
        let modal = self.modal.as_ref().map(|m| {
            moonglass::modal(
                m,
                move |_, cx| {
                    let _ = view.update(cx, |this, cx| this.close_modal(cx));
                },
                window,
                cx,
            )
            .into_any_element()
        });
        v_flex()
            .relative()
            .size_full()
            .bg(c.bg)
            .ui_text(cx)
            .child(self.render_title(cx))
            .child(div().flex_1().min_h_0().child(body))
            .children(bar)
            .children(modal)
            .child(moonglass::toast::render(toast_bottom, window, cx))
    }
}
