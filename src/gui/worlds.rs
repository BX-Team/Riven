use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, div, img, px,
};
use riven_launch::worlds::{self, Backup, Difficulty, GameMode, World};
use rust_i18n::t;

use super::runtime;
use super::state::AppState;
use moonglass::theme::ActiveTheme as _;
use moonglass::{
    ActionMenu, Button, ButtonSize, IconName, MenuEntry, Tone, W_SEMIBOLD, chip, h_flex, icon,
    motion, tooltip, v_flex,
};

const ICON: f32 = 56.;
const MENU_WIDTH: f32 = 260.;

struct Row {
    world: World,
    backups: Vec<Backup>,
}

/// The Worlds tab: the instance's `saves/`, with backups and copies to other instances.
pub struct WorldsView {
    id: String,
    game_dir: PathBuf,
    instance_dir: PathBuf,
    rows: Vec<Row>,
    loading: bool,
    /// The world being zipped, restored, copied or deleted.
    working: Option<String>,
    /// `working` once it has run long enough to show; quick jobs would only flash the buttons.
    busy: Option<String>,
}

/// What a finished job tells the player.
enum Done {
    Quiet,
    Toast(String),
    /// A check on the world's backup buttons for a moment.
    BackedUp,
}

fn backed_up(folder: &str) -> SharedString {
    SharedString::from(format!("world-backed-up:{folder}"))
}

/// "Back up", or a green check with "Backed up" for a moment after it worked.
fn backup_done(id: impl Into<gpui_kit::ElementId>, cx: &App) -> Button {
    let c = cx.theme().colors;
    Button::new(id)
        .outline()
        .child(icon(IconName::Check, c.ok))
        .child(
            div()
                .text_color(c.ok)
                .child(t!("worlds.backed_up").to_string()),
        )
}

const SHOW_BUSY_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

fn read_rows(game: &std::path::Path, instance: &std::path::Path) -> Vec<Row> {
    worlds::list(game)
        .into_iter()
        .map(|world| Row {
            backups: worlds::backups(instance, &world.folder),
            world,
        })
        .collect()
}

impl WorldsView {
    pub fn new(id: String, instance_dir: PathBuf, game_dir: PathBuf) -> Self {
        Self {
            id,
            game_dir,
            instance_dir,
            rows: Vec::new(),
            loading: false,
            working: None,
            busy: None,
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();
        let (game, instance) = (self.game_dir.clone(), self.instance_dir.clone());
        cx.spawn(async move |this, cx| {
            let rows = runtime::blocking(move || read_rows(&game, &instance))
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.rows = rows;
                this.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs `job` off the main thread for one world and lists the worlds again in the same go,
    /// so the result lands in one frame.
    fn run(
        &mut self,
        folder: &str,
        done: Done,
        job: impl FnOnce() -> Result<(), riven_launch::LaunchError> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.working.is_some() {
            return;
        }
        self.working = Some(folder.to_owned());
        let folder = folder.to_owned();
        let shown = folder.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHOW_BUSY_AFTER).await;
            let _ = this.update(cx, |this, cx| {
                if this.working.as_ref() == Some(&shown) {
                    this.busy = Some(shown);
                    cx.notify();
                }
            });
        })
        .detach();
        let (game, instance) = (self.game_dir.clone(), self.instance_dir.clone());
        cx.spawn(async move |this, cx| {
            let result = runtime::blocking(move || (job(), read_rows(&game, &instance)))
                .await
                .map_err(|e| riven_launch::LaunchError::Interrupted(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.working = None;
                this.busy = None;
                let result = match result {
                    Ok((result, rows)) => {
                        this.rows = rows;
                        result
                    }
                    Err(e) => Err(e),
                };
                cx.notify();
                match result {
                    Ok(()) => match done {
                        Done::Quiet => {}
                        Done::Toast(text) => {
                            moonglass::toast::show(moonglass::toast::ToastKind::Success, text, cx)
                        }
                        Done::BackedUp => {
                            moonglass::copy::mark(backed_up(&folder), cx.entity_id(), cx)
                        }
                    },
                    Err(e) => moonglass::toast::show(
                        moonglass::toast::ToastKind::Error,
                        e.to_string(),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    fn backup(&mut self, folder: String, cx: &mut Context<Self>) {
        let (instance, game, f) = (
            self.instance_dir.clone(),
            self.game_dir.clone(),
            folder.clone(),
        );
        self.run(
            &folder,
            Done::BackedUp,
            move || worlds::backup(&instance, &game, &f).map(drop),
            cx,
        );
    }

    fn restore(&mut self, folder: String, backup: PathBuf, cx: &mut Context<Self>) {
        let (game, f) = (self.game_dir.clone(), folder.clone());
        self.run(
            &folder,
            Done::Toast(t!("worlds.restored").into()),
            move || worlds::restore(&game, &f, &backup),
            cx,
        );
    }

    fn copy_to(&mut self, folder: String, to: PathBuf, name: String, cx: &mut Context<Self>) {
        let (game, f) = (self.game_dir.clone(), folder.clone());
        self.run(
            &folder,
            Done::Toast(t!("worlds.copied", name = name).into()),
            move || worlds::copy_to(&game, &f, &to).map(drop),
            cx,
        );
    }

    fn delete(&mut self, folder: String, cx: &mut Context<Self>) {
        let (game, f) = (self.game_dir.clone(), folder.clone());
        self.run(&folder, Done::Quiet, move || worlds::delete(&game, &f), cx);
    }

    fn delete_backup(&mut self, folder: String, backup: PathBuf, cx: &mut Context<Self>) {
        self.run(
            &folder,
            Done::Quiet,
            move || worlds::delete_backup(&backup),
            cx,
        );
    }

    /// "3 worlds · 1.2 GB · backups 2 · 300 MB" over the list.
    fn summary(&self) -> String {
        let size: u64 = self.rows.iter().map(|r| r.world.size).sum();
        let backups: Vec<&Backup> = self.rows.iter().flat_map(|r| &r.backups).collect();
        let mut parts = vec![
            t!("worlds.count", n = self.rows.len()).to_string(),
            super::settings::human_size(size),
        ];
        if !backups.is_empty() {
            parts.push(
                t!(
                    "worlds.backups_total",
                    n = backups.len(),
                    size = super::settings::human_size(backups.iter().map(|b| b.size).sum())
                )
                .to_string(),
            );
        }
        parts.join(" · ")
    }

    fn running(&self, cx: &App) -> bool {
        AppState::global(cx)
            .read(cx)
            .sessions
            .get(&self.id)
            .is_some_and(super::session::Session::busy)
    }

    fn menu(&self, row: &Row, view: WeakEntity<Self>, cx: &Context<Self>) -> Vec<MenuEntry> {
        let world = &row.world;
        let busy = self.working.is_some();
        let running = self.running(cx);
        let folder = world.folder.clone();
        let dir = self.game_dir.join("saves").join(&folder);
        let mut entries = vec![
            MenuEntry::action(t!("instance.open_folder"), move |_, cx| {
                cx.open_with_system(&dir)
            })
            .icon(IconName::Folder),
            {
                let (view, folder) = (view.clone(), folder.clone());
                MenuEntry::action(t!("worlds.backup"), move |_, cx| {
                    let folder = folder.clone();
                    let _ = view.update(cx, |this, cx| this.backup(folder, cx));
                })
                .icon(IconName::Plus)
                .disabled(busy)
            },
        ];
        if let Some(seed) = world.seed {
            entries.push(
                MenuEntry::action(t!("worlds.copy_seed"), move |_, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(seed.to_string()));
                    moonglass::toast::show(
                        moonglass::toast::ToastKind::Success,
                        t!("worlds.seed_copied", seed = seed).to_string(),
                        cx,
                    );
                })
                .icon(IconName::Copy),
            );
        }
        {
            let (view, folder, name) = (view.clone(), folder.clone(), world.name.clone());
            entries.push(
                MenuEntry::action(
                    t!("worlds.open_backups", n = row.backups.len()),
                    move |_, cx| {
                        if let Some(view) = view.upgrade() {
                            open_backups(view, folder.clone(), name.clone(), cx);
                        }
                    },
                )
                .icon(IconName::History),
            );
        }
        let state = AppState::global(cx).read(cx);
        let others: Vec<(String, PathBuf)> = state
            .store
            .as_ref()
            .map(|store| {
                state
                    .instances
                    .iter()
                    .filter(|(id, _)| *id != self.id)
                    .map(|(id, i)| (i.name.clone(), store.game_dir(id)))
                    .collect()
            })
            .unwrap_or_default();
        if !others.is_empty() {
            entries.push(MenuEntry::Caption(t!("worlds.copy_to").into()));
            for (name, game_dir) in others {
                let (view, folder) = (view.clone(), folder.clone());
                entries.push(
                    MenuEntry::action(name.clone(), move |_, cx| {
                        let (folder, to, name) = (folder.clone(), game_dir.clone(), name.clone());
                        let _ = view.update(cx, |this, cx| this.copy_to(folder, to, name, cx));
                    })
                    .icon(IconName::Copy)
                    .disabled(busy),
                );
            }
        }
        entries.push(MenuEntry::Separator);
        let name = world.name.clone();
        entries.push(
            MenuEntry::action(t!("worlds.delete"), move |_, cx| {
                let (view, folder) = (view.clone(), folder.clone());
                super::dialogs::confirm(
                    t!("worlds.delete_title", name = name),
                    t!("worlds.delete_body"),
                    t!("mods.delete"),
                    move |_, cx| {
                        let folder = folder.clone();
                        let _ = view.update(cx, |this, cx| this.delete(folder, cx));
                    },
                    cx,
                );
            })
            .icon(IconName::Trash)
            .danger()
            .disabled(busy || running),
        );
        entries
    }

    fn render_row(&self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let row = &self.rows[ix];
        let world = &row.world;
        let view = cx.entity().downgrade();
        let entries = self.menu(row, view.clone(), cx);
        let working = self.busy.as_deref() == Some(world.folder.as_str());
        let hover = motion::hover(
            SharedString::from(format!("world:{}:hover", world.folder)),
            window,
            cx,
        );
        let border = motion::animate(
            SharedString::from(format!("world:{}:border", world.folder)),
            if hover.on { c.border } else { c.row },
            window,
            cx,
        );
        let mut chips = Vec::new();
        if let Some(mode) = world.mode {
            let tone = match mode {
                GameMode::Hardcore => Tone::Danger,
                GameMode::Creative => Tone::Accent,
                _ => Tone::Ok,
            };
            chips.push(chip(mode_label(mode), tone, cx));
        }
        if let Some(d) = world.difficulty
            && world.mode != Some(GameMode::Hardcore)
        {
            chips.push(chip(difficulty_label(d), Tone::Muted, cx));
        }
        if world.cheats {
            chips.push(chip(t!("worlds.cheats").to_string(), Tone::Warn, cx));
        }
        if let Some(v) = &world.version {
            chips.push(chip(v.clone(), Tone::Muted, cx));
        }
        if let Some(days) = world.days {
            chips.push(chip(
                t!("worlds.day", n = days).to_string(),
                Tone::Muted,
                cx,
            ));
        }
        let last_backup = match row.backups.first() {
            Some(b) => t!(
                "worlds.last_backup",
                time = super::time::since(b.created),
                n = row.backups.len()
            )
            .to_string(),
            None => t!("worlds.no_backups").to_string(),
        };
        let meta = [
            t!(
                "worlds.played",
                time = super::time::since(world.last_played)
            )
            .to_string(),
            super::settings::human_size(world.size),
            last_backup,
        ]
        .join(" · ");
        let picture = match &world.icon {
            Some(path) => img(super::sharp::image(path.clone(), ICON))
                .size(px(ICON))
                .flex_none()
                .rounded(px(9.))
                .into_any_element(),
            None => div()
                .size(px(ICON))
                .flex_none()
                .rounded(px(9.))
                .bg(c.sel)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconName::Globe, c.muted).size(px(22.)))
                .into_any_element(),
        };
        let menu_entries = entries.clone();
        let backup_view = view.clone();
        let backups_button = {
            let (view, folder, name) = (cx.entity(), world.folder.clone(), world.name.clone());
            Button::new(("world-backups", ix))
                .ghost()
                .icon(IconName::History)
                .tooltip(t!("worlds.open_backups", n = row.backups.len()))
                .on_click(move |_, _, cx| {
                    open_backups(view.clone(), folder.clone(), name.clone(), cx)
                })
        };
        let folder = world.folder.clone();
        let backup_button = if moonglass::copy::copied(&backed_up(&world.folder), cx) {
            backup_done(("world-done", ix), cx)
        } else if working {
            Button::new(("world-busy", ix))
                .outline()
                .disabled(true)
                .child(motion::spinner(
                    SharedString::from(format!("world-spin-{ix}")),
                    icon(IconName::Loader, c.muted),
                    cx,
                ))
                .label(t!("worlds.working"))
        } else {
            Button::new(("world-backup", ix))
                .outline()
                .icon(IconName::Plus)
                .label(t!("worlds.backup"))
                .tooltip(t!("worlds.backup_hint"))
                .disabled(self.busy.is_some())
                .on_click(move |_, _, cx| {
                    let folder = folder.clone();
                    let _ = backup_view.update(cx, |this, cx| this.backup(folder, cx));
                })
        };
        hover
            .track(h_flex().id(("world", ix)))
            .gap(px(14.))
            .mx(px(18.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(10.))
            .border_1()
            .border_color(border)
            .bg(c.panel)
            .on_mouse_down(MouseButton::Right, move |event, _, cx| {
                super::file_tree::open_menu(event.position, menu_entries.clone(), cx)
            })
            .child(picture)
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
                            .child(world.name.clone()),
                    )
                    .when(!chips.is_empty(), |col| {
                        col.child(h_flex().gap(px(5.)).flex_wrap().children(chips))
                    })
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
                    .child(backup_button)
                    .child(backups_button)
                    .child(
                        ActionMenu::new(
                            ("world-menu", ix),
                            Button::new(("world-more", ix))
                                .ghost()
                                .icon(IconName::More)
                                .tooltip(t!("mods.actions")),
                            entries,
                        )
                        .width(px(MENU_WIDTH)),
                    ),
            )
            .into_any_element()
    }
}

fn difficulty_label(d: Difficulty) -> String {
    match d {
        Difficulty::Peaceful => t!("worlds.difficulty.peaceful"),
        Difficulty::Easy => t!("worlds.difficulty.easy"),
        Difficulty::Normal => t!("worlds.difficulty.normal"),
        Difficulty::Hard => t!("worlds.difficulty.hard"),
    }
    .into()
}

fn mode_label(mode: GameMode) -> String {
    match mode {
        GameMode::Survival => t!("worlds.mode.survival"),
        GameMode::Creative => t!("worlds.mode.creative"),
        GameMode::Adventure => t!("worlds.mode.adventure"),
        GameMode::Spectator => t!("worlds.mode.spectator"),
        GameMode::Hardcore => t!("worlds.mode.hardcore"),
    }
    .into()
}

impl Render for WorldsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let saves = self.game_dir.join("saves");
        let running = self.running(cx);
        let toolbar = h_flex()
            .flex_none()
            .gap(px(10.))
            .px(px(18.))
            .py(px(10.))
            .border_b_1()
            .border_color(c.row)
            .child(div().flex_1().text_color(c.muted).child(self.summary()))
            .when(running, |bar| {
                bar.child(
                    div()
                        .id("worlds-running")
                        .text_size(px(12.))
                        .text_color(c.warn)
                        .child(t!("worlds.running").to_string())
                        .tooltip(tooltip(t!("worlds.running_hint").into())),
                )
            })
            .child(
                Button::new("worlds-reload")
                    .ghost()
                    .icon(IconName::Refresh)
                    .tooltip(t!("configs.reload"))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .child(
                Button::new("worlds-folder")
                    .ghost()
                    .icon(IconName::Folder)
                    .tooltip(t!("instance.open_folder"))
                    .on_click(move |_, _, cx| {
                        let _ = std::fs::create_dir_all(&saves);
                        cx.open_with_system(&saves)
                    }),
            );
        let body: AnyElement = if self.rows.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.muted)
                .child(if self.loading {
                    t!("worlds.loading").to_string()
                } else {
                    t!("worlds.none").to_string()
                })
                .into_any_element()
        } else {
            let rows: Vec<AnyElement> = (0..self.rows.len())
                .map(|ix| self.render_row(ix, window, cx))
                .collect();
            div()
                .id("worlds")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(v_flex().gap(px(8.)).py(px(12.)).children(rows))
                .into_any_element()
        };
        v_flex().size_full().child(toolbar).child(body)
    }
}

/// Which change a backup row asks to confirm in place.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    Restore,
    Delete,
}

/// Every backup of one world, each with restore, show and delete; confirmations stay in the row.
struct BackupsDialog {
    view: Entity<WorldsView>,
    folder: String,
    name: String,
    asking: Option<(PathBuf, Ask)>,
    _observe: Subscription,
}

fn open_backups(view: Entity<WorldsView>, folder: String, name: String, cx: &mut App) {
    let dialog = cx.new(|cx| BackupsDialog {
        _observe: cx.observe(&view, |_, _, cx| cx.notify()),
        view,
        folder,
        name,
        asking: None,
    });
    super::dialogs::open(dialog.into(), 560., cx);
}

impl BackupsDialog {
    fn render_backup(
        &self,
        ix: usize,
        backup: &Backup,
        locked: (bool, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let (busy, running) = locked;
        let path = backup.path.clone();
        let asking = self
            .asking
            .as_ref()
            .filter(|(p, _)| *p == path)
            .map(|(_, ask)| *ask);
        let row = h_flex()
            .id(("backup", ix))
            .gap(px(10.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(if asking.is_some() { c.row } else { c.panel })
            .border_1()
            .border_color(c.row)
            .child(icon(IconName::History, c.muted).size(px(16.)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(W_SEMIBOLD)
                            .child(super::time::since(backup.created)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .truncate()
                            .child(match asking {
                                Some(Ask::Restore) => t!("worlds.restore_body").to_string(),
                                Some(Ask::Delete) => t!("worlds.delete_backup_body").to_string(),
                                None => super::settings::human_size(backup.size),
                            }),
                    ),
            );
        let buttons: Vec<AnyElement> = match asking {
            Some(kind) => {
                let (view, folder, target) = (self.view.clone(), self.folder.clone(), path.clone());
                vec![
                    Button::new(("backup-cancel", ix))
                        .ghost()
                        .label(t!("common.cancel"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.asking = None;
                            cx.notify();
                        }))
                        .into_any_element(),
                    Button::new(("backup-confirm", ix))
                        .danger()
                        .label(match kind {
                            Ask::Restore => t!("worlds.restore"),
                            Ask::Delete => t!("mods.delete"),
                        })
                        .disabled(busy || (kind == Ask::Restore && running))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.asking = None;
                            let (folder, target) = (folder.clone(), target.clone());
                            view.update(cx, |w, cx| match kind {
                                Ask::Restore => w.restore(folder, target, cx),
                                Ask::Delete => w.delete_backup(folder, target, cx),
                            });
                        }))
                        .into_any_element(),
                ]
            }
            None => {
                let show = path.clone();
                vec![
                    Button::new(("backup-restore", ix))
                        .outline()
                        .icon(IconName::Undo)
                        .label(t!("worlds.restore"))
                        .tooltip(if running {
                            t!("worlds.running_hint")
                        } else {
                            t!("worlds.restore_hint")
                        })
                        .disabled(busy || running)
                        .on_click(ask(path.clone(), Ask::Restore, cx))
                        .into_any_element(),
                    Button::new(("backup-show", ix))
                        .ghost()
                        .icon(IconName::Folder)
                        .tooltip(t!("mods.show_file"))
                        .on_click(move |_, _, cx| cx.reveal_path(&show))
                        .into_any_element(),
                    Button::new(("backup-delete", ix))
                        .ghost()
                        .icon(IconName::Trash)
                        .tooltip(t!("mods.delete"))
                        .disabled(busy)
                        .on_click(ask(path.clone(), Ask::Delete, cx))
                        .into_any_element(),
                ]
            }
        };
        row.child(h_flex().flex_none().gap(px(6.)).children(buttons))
            .into_any_element()
    }
}

/// Turns a backup row into its confirmation for `kind`.
fn ask(
    path: PathBuf,
    kind: Ask,
    cx: &mut Context<BackupsDialog>,
) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |this, _, _, cx| {
        this.asking = Some((path.clone(), kind));
        cx.notify();
    })
}

impl Render for BackupsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let world = self.view.read(cx);
        let backups: Vec<Backup> = world
            .rows
            .iter()
            .find(|r| r.world.folder == self.folder)
            .map(|r| r.backups.clone())
            .unwrap_or_default();
        let working = world.busy.as_deref() == Some(self.folder.as_str());
        let locked = (world.busy.is_some(), world.running(cx));
        let total: u64 = backups.iter().map(|b| b.size).sum();
        let body: AnyElement = if backups.is_empty() {
            v_flex()
                .items_center()
                .gap(px(6.))
                .py(px(28.))
                .child(icon(IconName::History, c.muted).size(px(22.)))
                .child(
                    div()
                        .text_color(c.muted)
                        .child(t!("worlds.no_backups_yet").to_string()),
                )
                .into_any_element()
        } else {
            let rows: Vec<AnyElement> = backups
                .iter()
                .enumerate()
                .map(|(ix, b)| self.render_backup(ix, b, locked, cx))
                .collect();
            div()
                .id("backups")
                .max_h(px(360.))
                .overflow_y_scroll()
                .child(v_flex().gap(px(6.)).children(rows))
                .into_any_element()
        };
        let (view, folder) = (self.view.clone(), self.folder.clone());
        let done = moonglass::copy::copied(&backed_up(&self.folder), cx);
        let create = if done {
            backup_done("backups-done", cx).size(ButtonSize::Md)
        } else {
            Button::new("backups-create")
                .primary()
                .size(ButtonSize::Md)
                .map(|b| {
                    if working {
                        b.label(t!("worlds.working")).disabled(true)
                    } else {
                        b.icon(IconName::Plus).label(t!("worlds.backup"))
                    }
                })
                .disabled(locked.0)
                .on_click(move |_, _, cx| {
                    let folder = folder.clone();
                    view.update(cx, |w, cx| w.backup(folder, cx));
                })
        };
        let close = Button::new("backups-close")
            .outline()
            .size(ButtonSize::Md)
            .label(t!("common.close"))
            .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx)));
        let description = if backups.is_empty() {
            t!("worlds.backups_hint").to_string()
        } else {
            format!(
                "{} · {}",
                t!("worlds.backups", n = backups.len()),
                super::settings::human_size(total)
            )
        };
        super::dialogs::dialog_shell(
            t!("worlds.backups_title", name = self.name.clone()),
            description,
            body,
            vec![close, create],
            cx,
        )
    }
}
