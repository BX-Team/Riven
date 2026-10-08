use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, UniformListScrollHandle, WeakEntity, Window, div, img, px,
    uniform_list,
};
use rust_i18n::t;

use super::runtime;
use super::state::AppState;
use super::theme::ActiveTheme as _;
use super::ui::{Button, ButtonSize, IconName, MenuEntry, h_flex, scrollbar, v_flex};

const THUMB_WIDTH: u32 = 384;
const TILE_WIDTH: f32 = 220.;
const TILE_HEIGHT: f32 = 124.;
const ROW_HEIGHT: f32 = TILE_HEIGHT + 34.;
const GAP: f32 = 14.;

#[derive(Clone)]
struct Shot {
    path: PathBuf,
    name: SharedString,
    modified: SystemTime,
    /// A small copy for the grid, once made.
    thumb: Option<PathBuf>,
}

/// The Screenshots tab: the instance's `screenshots/` as a grid, newest first.
pub struct ScreenshotsView {
    game_dir: PathBuf,
    shots: Vec<Shot>,
    loading: bool,
    /// Tiles per row at the last layout.
    columns: usize,
    scroll: UniformListScrollHandle,
}

fn thumbs_dir() -> Option<PathBuf> {
    super::mods::cache_dir().map(|d| d.join("thumbs"))
}

/// A cached small copy of a screenshot, named after its path and time so edits make a new one.
fn thumbnail(shot: &Path, modified: SystemTime) -> Option<PathBuf> {
    let stamp = modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let key = format!("{}:{stamp}", shot.display());
    let name = {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut h);
        format!("{:016x}.png", h.finish())
    };
    let dir = thumbs_dir()?;
    let out = dir.join(name);
    if out.is_file() {
        return Some(out);
    }
    let image = image::open(shot).ok()?;
    let small = image.thumbnail(THUMB_WIDTH, THUMB_WIDTH);
    std::fs::create_dir_all(&dir).ok()?;
    small.save_with_format(&out, image::ImageFormat::Png).ok()?;
    Some(out)
}

fn list(dir: &Path) -> Vec<Shot> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut shots: Vec<Shot> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            if !(lower.ends_with(".png") || lower.ends_with(".jpg")) {
                return None;
            }
            let modified = e.metadata().ok()?.modified().ok()?;
            Some(Shot {
                path,
                name: name.into(),
                modified,
                thumb: None,
            })
        })
        .collect();
    shots.sort_by_key(|s| std::cmp::Reverse(s.modified));
    shots
}

impl ScreenshotsView {
    pub fn new(game_dir: PathBuf) -> Self {
        Self {
            game_dir,
            shots: Vec::new(),
            loading: false,
            columns: 4,
            scroll: UniformListScrollHandle::new(),
        }
    }

    fn dir(&self) -> PathBuf {
        self.game_dir.join("screenshots")
    }

    /// Lists the folder, then makes the missing thumbnails a few at a time.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let dir = self.dir();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let shots = runtime::blocking(move || list(&dir))
                .await
                .unwrap_or_default();
            let wanted: Vec<(PathBuf, SystemTime)> =
                shots.iter().map(|s| (s.path.clone(), s.modified)).collect();
            let _ = this.update(cx, |this, cx| {
                this.shots = shots;
                this.loading = false;
                cx.notify();
            });
            for chunk in wanted.chunks(8) {
                let chunk = chunk.to_vec();
                let made = runtime::blocking(move || {
                    chunk
                        .into_iter()
                        .filter_map(|(path, modified)| {
                            let thumb = thumbnail(&path, modified)?;
                            Some((path, thumb))
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                let alive = this.update(cx, |this, cx| {
                    for (path, thumb) in made {
                        if let Some(shot) = this.shots.iter_mut().find(|s| s.path == path) {
                            shot.thumb = Some(thumb);
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    fn delete(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Err(e) = std::fs::remove_file(&path) {
            AppState::global(cx).update(cx, |s, cx| {
                s.toast(
                    super::toast::ToastKind::Error,
                    format!("{}: {e}", path.display()),
                    cx,
                )
            });
            return;
        }
        self.shots.retain(|s| s.path != path);
        cx.notify();
    }

    fn menu(shot: &Shot, view: WeakEntity<Self>) -> Vec<MenuEntry> {
        let (open, show, copy, delete) = (
            shot.path.clone(),
            shot.path.clone(),
            shot.path.clone(),
            shot.path.clone(),
        );
        let name = shot.name.clone();
        vec![
            MenuEntry::action(t!("screenshots.open"), move |_, cx| {
                cx.open_with_system(&open)
            })
            .icon(IconName::ExternalLink),
            MenuEntry::action(t!("mods.show_file"), move |_, cx| cx.reveal_path(&show))
                .icon(IconName::Folder),
            MenuEntry::action(t!("screenshots.copy"), move |_, cx| copy_image(&copy, cx))
                .icon(IconName::Copy),
            MenuEntry::Separator,
            MenuEntry::action(t!("mods.delete"), move |_, cx| {
                let (view, path) = (view.clone(), delete.clone());
                super::dialogs::confirm(
                    t!("screenshots.delete_title", name = name),
                    t!("screenshots.delete_body"),
                    t!("mods.delete"),
                    move |_, cx| {
                        let path = path.clone();
                        let _ = view.update(cx, |this, cx| this.delete(path, cx));
                    },
                    cx,
                );
            })
            .icon(IconName::Trash)
            .danger(),
        ]
    }

    fn render_tile(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors;
        let shot = self.shots.get(ix)?.clone();
        let view = cx.entity().downgrade();
        let opened = shot.clone();
        let entries = Self::menu(&shot, view.clone());
        Some(
            v_flex()
                .id(("shot", ix))
                .w(px(TILE_WIDTH))
                .gap(px(6.))
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    open_viewer(opened.clone(), view.clone(), window, cx)
                })
                .on_mouse_down(MouseButton::Right, move |event, _, cx| {
                    super::file_tree::open_menu(event.position, entries.clone(), cx)
                })
                .child(
                    div()
                        .w(px(TILE_WIDTH))
                        .h(px(TILE_HEIGHT))
                        .rounded(px(8.))
                        .overflow_hidden()
                        .bg(c.row)
                        .border_1()
                        .border_color(c.border)
                        .hover(|s| s.border_color(c.accent))
                        .children(
                            shot.thumb
                                .clone()
                                .map(|t| img(t).size_full().object_fit(gpui_kit::ObjectFit::Cover)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(c.text2)
                        .truncate()
                        .child(super::time::ago(shot.modified)),
                )
                .into_any_element(),
        )
    }
}

fn copy_image(path: &Path, cx: &mut App) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let format = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg"))
    {
        gpui_kit::ImageFormat::Jpeg
    } else {
        gpui_kit::ImageFormat::Png
    };
    let image = gpui_kit::Image::from_bytes(format, bytes);
    cx.write_to_clipboard(ClipboardItem::new_image(&image));
}

impl Render for ScreenshotsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let width = f32::from(window.viewport_size().width) - super::app::SIDEBAR_WIDTH - 36.;
        self.columns = ((width + GAP) / (TILE_WIDTH + GAP)).floor().max(1.) as usize;
        let columns = self.columns;
        let rows = self.shots.len().div_ceil(columns);
        let dir = self.dir();
        let toolbar = h_flex()
            .flex_none()
            .gap(px(10.))
            .px(px(18.))
            .py(px(10.))
            .border_b_1()
            .border_color(c.row)
            .child(
                div()
                    .flex_1()
                    .text_color(c.muted)
                    .child(t!("screenshots.count", n = self.shots.len()).to_string()),
            )
            .child(
                Button::new("shots-reload")
                    .ghost()
                    .icon(IconName::Refresh)
                    .tooltip(t!("configs.reload"))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .child(
                Button::new("shots-folder")
                    .ghost()
                    .icon(IconName::Folder)
                    .tooltip(t!("instance.open_folder"))
                    .on_click(move |_, _, cx| {
                        let _ = std::fs::create_dir_all(&dir);
                        cx.open_with_system(&dir)
                    }),
            );
        let body: AnyElement = if self.shots.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.muted)
                .child(if self.loading {
                    t!("screenshots.loading").to_string()
                } else {
                    t!("screenshots.none").to_string()
                })
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    uniform_list(
                        "shots",
                        rows,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|row| {
                                    h_flex()
                                        .h(px(ROW_HEIGHT))
                                        .px(px(18.))
                                        .pt(px(GAP))
                                        .gap(px(GAP))
                                        .items_start()
                                        .children(
                                            (row * columns..(row + 1) * columns)
                                                .filter_map(|ix| this.render_tile(ix, cx)),
                                        )
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full()
                    .track_scroll(&self.scroll),
                )
                .child(scrollbar(&self.scroll))
                .into_any_element()
        };
        v_flex().size_full().child(toolbar).child(body)
    }
}

/// One screenshot over the window, with what can be done to it.
struct Viewer {
    shot: Shot,
    view: WeakEntity<ScreenshotsView>,
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let height = (f32::from(window.viewport_size().height) - 200.).max(240.);
        let (open, show, copy, delete) = (
            self.shot.path.clone(),
            self.shot.path.clone(),
            self.shot.path.clone(),
            self.shot.path.clone(),
        );
        let (view, name) = (self.view.clone(), self.shot.name.clone());
        v_flex()
            .child(
                div()
                    .h(px(height))
                    .p(px(12.))
                    .bg(c.bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(self.shot.path.clone())
                            .max_w_full()
                            .max_h_full()
                            .object_fit(gpui_kit::ObjectFit::ScaleDown),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(c.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c.text2)
                            .child(self.shot.name.clone()),
                    )
                    .child(
                        Button::new("viewer-copy")
                            .icon(IconName::Copy)
                            .label(t!("screenshots.copy"))
                            .on_click(move |_, _, cx| copy_image(&copy, cx)),
                    )
                    .child(
                        Button::new("viewer-show")
                            .icon(IconName::Folder)
                            .tooltip(t!("mods.show_file"))
                            .on_click(move |_, _, cx| cx.reveal_path(&show)),
                    )
                    .child(
                        Button::new("viewer-open")
                            .icon(IconName::ExternalLink)
                            .tooltip(t!("screenshots.open"))
                            .on_click(move |_, _, cx| cx.open_with_system(&open)),
                    )
                    .child(
                        Button::new("viewer-delete")
                            .ghost()
                            .icon(IconName::Trash)
                            .tooltip(t!("mods.delete"))
                            .on_click(move |_, _, cx| {
                                let (view, path) = (view.clone(), delete.clone());
                                super::dialogs::confirm(
                                    t!("screenshots.delete_title", name = name),
                                    t!("screenshots.delete_body"),
                                    t!("mods.delete"),
                                    move |_, cx| {
                                        let path = path.clone();
                                        let _ = view.update(cx, |this, cx| this.delete(path, cx));
                                    },
                                    cx,
                                );
                            }),
                    )
                    .child(
                        Button::new("viewer-close")
                            .size(ButtonSize::Md)
                            .label(t!("common.close"))
                            .on_click(|_, _, cx| {
                                AppState::global(cx).update(cx, |s, cx| s.close_modal(cx))
                            }),
                    ),
            )
    }
}

fn open_viewer(shot: Shot, view: WeakEntity<ScreenshotsView>, window: &mut Window, cx: &mut App) {
    let width = (f32::from(window.viewport_size().width) - 120.).min(1400.);
    let viewer = cx.new(|_| Viewer { shot, view });
    super::dialogs::open(viewer.into(), width, cx);
}
