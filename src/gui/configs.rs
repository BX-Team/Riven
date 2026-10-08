use std::path::{Path, PathBuf};

use gpui_kit::base::input::{Editor, EditorState, InputEditorStyle, InputEvent};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, StyledImage as _,
    Subscription, Window, div, img, px,
};
use rust_i18n::t;

use super::dev::highlight::{self, Flavor, ThemeStyles};
use super::file_tree::{self, FileTree, Node, TreeEvent, is_image};
use super::runtime;
use super::theme::ActiveTheme as _;
use super::ui::{Button, ButtonSize, IconName, MenuEntry, h_flex, icon, v_flex};

/// What the tab lists from the game folder.
const TOPS: [&str; 4] = ["config", "defaultconfigs", "kubejs", "options.txt"];
/// Bigger files are not opened as text.
const MAX_TEXT: u64 = 4 * 1024 * 1024;
const TREE_WIDTH: f32 = 280.;

enum Opened {
    Text {
        editor: Entity<EditorState>,
        saved: String,
        dirty: bool,
        _change: Subscription,
    },
    Image(PathBuf),
}

/// The Configs tab of an instance: its config folders as a tree and the open file.
pub struct ConfigsView {
    game_dir: PathBuf,
    tree: Entity<FileTree>,
    open: Option<(String, Opened)>,
    loaded: bool,
    /// The instance came from a pack, whose configs the player only reads.
    locked: bool,
    error: Option<SharedString>,
    notice: Option<SharedString>,
    _subs: Vec<Subscription>,
}

impl ConfigsView {
    pub fn new(game_dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|_| FileTree::new("configs-tree"));
        let subs =
            vec![
                cx.subscribe_in(&tree, window, |this, _, event: &TreeEvent, window, cx| {
                    this.on_tree(event, window, cx)
                }),
            ];
        Self {
            game_dir,
            tree,
            open: None,
            loaded: false,
            locked: false,
            error: None,
            notice: None,
            _subs: subs,
        }
    }

    /// Lists the folders again; the first time the tab shows, and on request.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.loaded = true;
        self.locked = riven_launch::own::from_pack(&self.game_dir);
        let dir = self.game_dir.clone();
        cx.spawn(async move |this, cx| {
            let nodes = runtime::blocking(move || file_tree::scan(&dir, &TOPS))
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.tree.update(cx, |tree, cx| {
                    tree.set_nodes(nodes, cx);
                    tree.expand("config", cx);
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.loaded {
            self.reload(cx);
        }
    }

    fn full(&self, path: &str) -> Option<PathBuf> {
        let rel = Path::new(path);
        let safe = rel
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        safe.then(|| self.game_dir.join(rel))
    }

    fn on_tree(&mut self, event: &TreeEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TreeEvent::Open(path) => self.open_file(path.clone(), window, cx),
            TreeEvent::Menu { node, position } => {
                let entries = self.menu(node.as_ref(), cx);
                file_tree::open_menu(*position, entries, cx);
            }
            TreeEvent::Move { .. } => {}
        }
    }

    fn menu(&self, node: Option<&Node>, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let view = cx.entity().downgrade();
        let mut entries = Vec::new();
        let Some(node) = node else {
            let reload = view.clone();
            entries.push(
                MenuEntry::action(t!("configs.reload"), move |_, cx| {
                    let _ = reload.update(cx, |this, cx| this.reload(cx));
                })
                .icon(IconName::Refresh),
            );
            return entries;
        };
        if !node.dir {
            let (open, path) = (view.clone(), node.path.clone());
            entries.push(
                MenuEntry::action(t!("dev.open_file"), move |window, cx| {
                    let _ = open.update(cx, |this, cx| this.open_file(path.clone(), window, cx));
                })
                .icon(IconName::File),
            );
        }
        if let Some(full) = self.full(&node.path) {
            let shown = full.clone();
            entries.push(
                MenuEntry::action(t!("mods.show_file"), move |_, cx| cx.reveal_path(&shown))
                    .icon(IconName::Folder),
            );
            if !node.dir {
                entries.push(
                    MenuEntry::action(t!("configs.open_outside"), move |_, cx| {
                        cx.open_with_system(&full)
                    })
                    .icon(IconName::ExternalLink),
                );
            }
        }
        entries
    }

    fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(full) = self.full(&path) else {
            return;
        };
        self.error = None;
        self.notice = None;
        if is_image(&path) {
            self.open = Some((path.clone(), Opened::Image(full)));
            self.tree
                .update(cx, |tree, cx| tree.set_active(Some(path), cx));
            cx.notify();
            return;
        }
        let read = std::fs::metadata(&full)
            .map_err(|e| e.to_string())
            .and_then(|m| {
                if m.len() > MAX_TEXT {
                    Err(t!("configs.too_big").to_string())
                } else {
                    std::fs::read(&full).map_err(|e| e.to_string())
                }
            })
            .and_then(|bytes| {
                String::from_utf8(bytes).map_err(|_| t!("configs.binary").to_string())
            });
        let text = match read {
            Ok(text) => text,
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
                return;
            }
        };
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let initial = text.clone();
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(Flavor::of(&name).language())
                .default_value(initial);
            state.set_highlighter_factory(highlight::factory(), cx);
            state.set_readonly(self.locked, cx);
            state
        });
        let change = cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
            if let InputEvent::Change = event
                && let Some((_, Opened::Text { saved, dirty, .. })) = &mut this.open
            {
                let now = editor.read(cx).value().as_ref() != saved.as_str();
                if now != *dirty {
                    *dirty = now;
                    cx.notify();
                }
            }
        });
        self.open = Some((
            path.clone(),
            Opened::Text {
                editor,
                saved: text,
                dirty: false,
                _change: change,
            },
        ));
        self.tree
            .update(cx, |tree, cx| tree.set_active(Some(path), cx));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some((
            path,
            Opened::Text {
                editor,
                saved,
                dirty,
                ..
            },
        )) = &mut self.open
        else {
            return;
        };
        if !*dirty || self.locked {
            return;
        }
        let full = self.game_dir.join(path.as_str());
        let text = editor.read(cx).value().to_string();
        let tmp = full.with_file_name(format!(
            ".{}.tmp",
            full.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        ));
        let written = std::fs::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &full));
        match written {
            Ok(()) => {
                *saved = text;
                *dirty = false;
                self.error = None;
                self.notice =
                    Some(t!("dev.saved", name = path.rsplit('/').next().unwrap_or(path)).into());
            }
            Err(e) => self.error = Some(format!("{}: {e}", full.display()).into()),
        }
        cx.notify();
    }

    fn render_open(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let c = theme.colors;
        let mono = theme.mono.clone();
        let Some((path, opened)) = &self.open else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.muted)
                .child(t!("configs.pick").to_string())
                .into_any_element();
        };
        let head = h_flex()
            .flex_none()
            .gap(px(10.))
            .px(px(14.))
            .py(px(6.))
            .border_b_1()
            .border_color(c.row)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono.clone())
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .child(path.clone()),
            )
            .when_some(self.notice.clone(), |row, n| {
                row.child(div().text_size(px(12.)).text_color(c.ok).child(n))
            });
        match opened {
            Opened::Image(full) => v_flex()
                .size_full()
                .child(head)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .p(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            img(full.clone())
                                .max_w_full()
                                .max_h_full()
                                .object_fit(gpui_kit::ObjectFit::ScaleDown),
                        ),
                )
                .into_any_element(),
            Opened::Text { editor, dirty, .. } => {
                let style = InputEditorStyle {
                    foreground: c.text,
                    muted_foreground: c.muted,
                    background: c.bg,
                    border: c.border,
                    selection: c.accent.opacity(0.3),
                    caret: c.accent,
                    highlight_styles: ThemeStyles::new(&c),
                    editor_active_line: Some(c.row),
                    editor_gutter_background: Some(c.bg),
                    ..InputEditorStyle::default()
                };
                editor.update(cx, |s, _| s.set_editor_style(style));
                v_flex()
                    .size_full()
                    .child(if self.locked {
                        head.child(
                            h_flex()
                                .gap(px(6.))
                                .text_size(px(12.))
                                .text_color(c.muted)
                                .child(icon(IconName::Lock, c.muted).size(px(13.)))
                                .child(t!("configs.locked").to_string()),
                        )
                    } else {
                        head.child(
                            Button::new("config-save")
                                .size(ButtonSize::Xs)
                                .label(t!("dev.save"))
                                .tooltip(t!("dev.save_hint"))
                                .disabled(!*dirty)
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        )
                    })
                    .child(
                        div()
                            .id("config-editor")
                            .flex_1()
                            .min_h_0()
                            .bg(c.bg)
                            .font_family(mono)
                            .text_size(px(12.5))
                            .line_height(px(20.))
                            .text_color(c.text)
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                let key = &event.keystroke;
                                if key.modifiers.secondary() && key.key == "s" {
                                    cx.stop_propagation();
                                    this.save(cx);
                                }
                            }))
                            .child(Editor::new(editor)),
                    )
                    .into_any_element()
            }
        }
    }
}

impl Render for ConfigsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let dir = self.game_dir.join("config");
        h_flex()
            .size_full()
            .items_start()
            .child(
                v_flex()
                    .w(px(TREE_WIDTH))
                    .flex_none()
                    .h_full()
                    .border_r_1()
                    .border_color(c.border)
                    .p(px(8.))
                    .gap(px(6.))
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .text_color(c.muted)
                                    .child(t!("dev.files").to_string()),
                            )
                            .child(
                                Button::new("configs-reload")
                                    .ghost()
                                    .size(ButtonSize::Xs)
                                    .icon(IconName::Refresh)
                                    .tooltip(t!("configs.reload"))
                                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                            )
                            .child(
                                Button::new("configs-folder")
                                    .ghost()
                                    .size(ButtonSize::Xs)
                                    .icon(IconName::Folder)
                                    .tooltip(t!("instance.open_folder"))
                                    .on_click(move |_, _, cx| cx.open_with_system(&dir)),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(self.tree.clone())),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .when_some(self.error.clone(), |col, e| {
                        col.child(
                            h_flex()
                                .flex_none()
                                .gap(px(8.))
                                .px(px(14.))
                                .py(px(8.))
                                .text_color(c.warn)
                                .child(icon(IconName::Close, c.warn).size(px(13.)))
                                .child(e),
                        )
                    })
                    .child(div().flex_1().min_h_0().child(self.render_open(cx))),
            )
    }
}
