use gpui_kit::base::input::{Editor, EditorState, InputEditorStyle, InputEvent};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use riven_format::PackPath;
use rust_i18n::t;

use super::highlight::{self, Flavor, ThemeStyles};
use super::{DevView, OVERRIDES, OpenFile, PROJECT_FILE, Tab, dialogs};
use crate::gui::file_tree::{Node, TreeEvent, is_image};
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{
    Button, ButtonSize, IconName, MenuEntry, caption, h_flex, icon, motion, v_flex,
};

const TREE_ROW: f32 = 26.;

/// The `riven.json` row above the tree, lit while hovered.
fn tree_row(
    key: String,
    depth: usize,
    on: bool,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> (gpui_kit::Stateful<gpui_kit::Div>, bool) {
    let c = cx.theme().colors;
    let hover = motion::hover(SharedString::from(format!("tree:{key}")), window, cx);
    let lit = hover.on;
    let row = hover
        .track(h_flex().id(SharedString::from(format!("tree-{key}"))))
        .h(px(TREE_ROW))
        .pl(px(8. + depth as f32 * 12.))
        .pr(px(2.))
        .gap(px(6.))
        .rounded(px(5.))
        .cursor_pointer()
        .map(|r| match (on, lit) {
            (true, _) => r.bg(c.sel).text_color(c.text),
            (false, true) => r.bg(c.row).text_color(c.text),
            _ => r.text_color(c.text2),
        });
    (row, lit)
}

fn project_file() -> PackPath {
    PackPath::new(PROJECT_FILE).expect("riven.json is a valid pack path")
}

/// A path under `overrides/` as the tree names it: `common/config/a.toml`.
fn in_tree(path: &PackPath) -> Option<&str> {
    path.as_str()
        .strip_prefix(OVERRIDES)
        .and_then(|rest| rest.strip_prefix('/'))
}

fn from_tree(path: &str) -> Option<PackPath> {
    PackPath::new(format!("{OVERRIDES}/{path}")).ok()
}

impl DevView {
    pub(super) fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        self.releases.changes = super::releases::Changes::Stale;
        let entries = match &self.project {
            Some(ws) => ws.overrides().unwrap_or_else(|e| {
                self.error = Some(e.to_string().into());
                Vec::new()
            }),
            None => Vec::new(),
        };
        let nodes = entries
            .iter()
            .filter_map(|e| {
                Some(Node {
                    path: in_tree(&e.path)?.to_owned(),
                    dir: e.dir,
                })
            })
            .collect();
        self.tree.update(cx, |tree, cx| tree.set_nodes(nodes, cx));
    }

    pub(super) fn on_tree(&mut self, event: &TreeEvent, cx: &mut Context<Self>) {
        match event {
            TreeEvent::Open(path) => {
                if let Some(path) = from_tree(path) {
                    self.open_file(path, cx);
                }
            }
            TreeEvent::Menu { node, position } => {
                let entries = self.tree_menu(node.as_ref(), cx);
                crate::gui::file_tree::open_menu(*position, entries, cx);
            }
            TreeEvent::Move { from, to } => self.move_path(from, to, cx),
        }
    }

    /// What a right click on a tree node offers; files make new ones next to themselves.
    fn tree_menu(&self, node: Option<&Node>, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let view = cx.entity().downgrade();
        let folder = match node {
            Some(n) if n.dir => n.path.clone(),
            Some(n) => n.parent().to_owned(),
            None => "common".to_owned(),
        };
        let mut menu = Vec::new();
        if let Some(n) = node.filter(|n| !n.dir) {
            let (view, path) = (view.clone(), n.path.clone());
            menu.push(
                MenuEntry::action(t!("dev.open_file"), move |_, cx| {
                    if let Some(path) = from_tree(&path) {
                        let _ = view.update(cx, |this, cx| this.open_file(path, cx));
                    }
                })
                .icon(IconName::File),
            );
        }
        for make_folder in [false, true] {
            let (view, at) = (view.clone(), folder.clone());
            let label = if make_folder {
                t!("dev.new_folder")
            } else {
                t!("dev.new_file")
            };
            menu.push(
                MenuEntry::action(label, move |window, cx| {
                    dialogs::open_new_path(
                        view.clone(),
                        format!("{OVERRIDES}/{at}/"),
                        make_folder,
                        window,
                        cx,
                    )
                })
                .icon(if make_folder {
                    IconName::FolderPlus
                } else {
                    IconName::FilePlus
                }),
            );
        }
        let Some(node) = node else {
            return menu;
        };
        if let Some(full) = self
            .project
            .as_ref()
            .and_then(|ws| ws.override_path(&from_tree(&node.path)?).ok())
        {
            menu.push(
                MenuEntry::action(t!("mods.show_file"), move |_, cx| cx.reveal_path(&full))
                    .icon(IconName::Folder),
            );
        }
        menu.push(MenuEntry::Separator);
        let path = node.path.clone();
        menu.push(
            MenuEntry::action(t!("dev.remove"), move |_, cx| {
                if let Some(key) = from_tree(&path) {
                    let _ = view.update(cx, |this, cx| this.confirm_delete(key, cx));
                }
            })
            .icon(IconName::Trash)
            .danger(),
        );
        menu
    }

    /// Moves a file or folder dropped on another folder; open tabs follow the move.
    fn move_path(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        let (Some(ws), Some(source), Some(into)) = (&self.project, from_tree(from), from_tree(to))
        else {
            return;
        };
        if to.is_empty() || source.as_str().rsplit_once('/').map(|(p, _)| p) == Some(into.as_str())
        {
            return;
        }
        match ws.move_override(&source, &into) {
            Ok(moved) => {
                let prefix = format!("{}/", source.as_str());
                let open: Vec<PackPath> = self
                    .tabs
                    .iter()
                    .filter_map(|t| match t {
                        Tab::File(p) if *p == source || p.as_str().starts_with(&prefix) => {
                            Some(p.clone())
                        }
                        _ => None,
                    })
                    .collect();
                for path in open {
                    self.drop_tab(Tab::File(path), cx);
                }
                self.refresh_tree(cx);
                let shown = in_tree(&moved).unwrap_or_default().to_owned();
                self.tree.update(cx, |tree, cx| tree.reveal(&shown, cx));
                self.notice = Some(t!("dev.moved", name = moved.file_name(), to = to).into());
                self.error = None;
                self.refresh_git(cx);
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
        cx.notify();
    }

    pub(super) fn open_file(&mut self, path: PackPath, cx: &mut Context<Self>) {
        self.show(Tab::File(path), cx);
    }

    /// Reads the open file into an editor the first time its tab is shown.
    pub(super) fn ensure_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Tab::File(path) = self.active.clone() else {
            return;
        };
        if self.files.contains_key(&path) || is_image(path.as_str()) {
            return;
        }
        let Some(ws) = &self.project else {
            return;
        };
        let read_only = path == project_file();
        let text = if read_only {
            std::fs::read_to_string(ws.dir.join(PROJECT_FILE)).map_err(|e| e.to_string())
        } else {
            ws.read_override(&path).map_err(|e| e.to_string())
        };
        let text = match text {
            Ok(text) => text,
            Err(e) => {
                self.error = Some(e.into());
                self.drop_tab(Tab::File(path), cx);
                return;
            }
        };
        let flavor = Flavor::of(path.file_name());
        let initial = text.clone();
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(flavor.language())
                .default_value(initial);
            state.set_highlighter_factory(highlight::factory(), cx);
            state.set_readonly(read_only, cx);
            state
        });
        let key = path.clone();
        let change = cx.subscribe(&editor, move |this, editor, event: &InputEvent, cx| {
            if let InputEvent::Change = event
                && let Some(file) = this.files.get_mut(&key)
            {
                let dirty = editor.read(cx).value().as_ref() != file.saved;
                if dirty != file.dirty {
                    file.dirty = dirty;
                    cx.notify();
                }
            }
        });
        editor.update(cx, |s, cx| s.focus(window, cx));
        self.files.insert(
            path,
            OpenFile {
                editor,
                saved: text,
                dirty: false,
                _change: change,
            },
        );
    }

    /// Drops the read-only `riven.json` view so it is read again after a change.
    pub(super) fn forget_project_file(&mut self) {
        self.files.remove(&project_file());
    }

    pub(super) fn save_file(&mut self, path: &PackPath, cx: &mut Context<Self>) {
        let (Some(ws), Some(file)) = (&self.project, self.files.get_mut(path)) else {
            return;
        };
        if *path == project_file() || !file.dirty {
            return;
        }
        let text = file.editor.read(cx).value().to_string();
        match ws.write_override(path, &text) {
            Ok(()) => {
                file.saved = text;
                file.dirty = false;
                self.releases.changes = super::releases::Changes::Stale;
                self.error = None;
                self.notice = Some(t!("dev.saved", name = path.file_name()).into());
                self.refresh_git(cx);
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
        cx.notify();
    }

    /// Creates a file or folder inside `overrides/` and opens the file.
    pub(super) fn create_path(
        &mut self,
        path: &str,
        folder: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(ws) = &self.project else {
            return Ok(());
        };
        let path = PackPath::new(path.trim_end_matches('/')).map_err(|e| e.to_string())?;
        if folder {
            ws.create_override_dir(&path)
        } else {
            ws.create_override(&path)
        }
        .map_err(|e| e.to_string())?;
        self.refresh_tree(cx);
        if let Some(shown) = in_tree(&path).map(str::to_owned) {
            self.tree.update(cx, |tree, cx| {
                tree.reveal(&shown, cx);
                if folder {
                    tree.expand(&shown, cx);
                }
            });
        }
        if !folder {
            self.open_file(path, cx);
        }
        cx.notify();
        Ok(())
    }

    fn delete_path(&mut self, path: PackPath, cx: &mut Context<Self>) {
        let Some(ws) = &self.project else {
            return;
        };
        if let Err(e) = ws.delete_override(&path) {
            self.error = Some(e.to_string().into());
            cx.notify();
            return;
        }
        let gone: Vec<Tab> = self
            .tabs
            .iter()
            .filter(|t| match t {
                Tab::File(p) => {
                    p == &path || p.as_str().starts_with(&format!("{}/", path.as_str()))
                }
                Tab::Section(_) => false,
            })
            .cloned()
            .collect();
        for tab in gone {
            self.drop_tab(tab, cx);
        }
        self.refresh_tree(cx);
        cx.notify();
    }

    fn confirm_delete(&self, path: PackPath, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        crate::gui::dialogs::confirm(
            t!("dev.delete_file_title", name = path.file_name()),
            t!("dev.delete_file_body"),
            t!("dev.remove"),
            move |_, cx| {
                let path = path.clone();
                let _ = view.update(cx, |this, cx| this.delete_path(path, cx));
            },
            cx,
        );
    }

    /// The Files part of the left column: `riven.json` and the `overrides/` tree.
    pub(super) fn render_files(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        if self.project.is_none() {
            return div().flex_1().into_any_element();
        }
        let view = cx.entity().downgrade();
        let active = match &self.active {
            Tab::File(p) => Some(p.as_str().to_owned()),
            Tab::Section(_) => None,
        };
        let in_overrides = match &self.active {
            Tab::File(p) => in_tree(p).map(str::to_owned),
            Tab::Section(_) => None,
        };
        self.tree
            .update(cx, |tree, cx| tree.set_active(in_overrides, cx));
        let project = project_file();
        let (top, _) = tree_row(
            PROJECT_FILE.into(),
            0,
            active.as_deref() == Some(PROJECT_FILE),
            window,
            cx,
        );
        let top = top
            .child(icon(IconName::File, c.muted).size(px(13.)))
            .child(div().truncate().child(PROJECT_FILE))
            .on_click(cx.listener(move |this, _, _, cx| this.open_file(project.clone(), cx)));
        let new_file = view.clone();
        v_flex()
            .flex_1()
            .min_h_0()
            .mt(px(10.))
            .child(
                h_flex()
                    .flex_none()
                    .justify_between()
                    .pr(px(2.))
                    .child(caption(t!("dev.files"), cx))
                    .child(
                        Button::new("dev-new-file")
                            .ghost()
                            .size(ButtonSize::Xs)
                            .icon(IconName::Plus)
                            .tooltip(t!("dev.new_file"))
                            .on_click(move |_, window, cx| {
                                dialogs::open_new_path(
                                    new_file.clone(),
                                    format!("{OVERRIDES}/common/config/"),
                                    false,
                                    window,
                                    cx,
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(mono)
                    .text_size(px(12.))
                    .child(top),
            )
            .child(div().flex_1().min_h_0().child(self.tree.clone()))
            .into_any_element()
    }

    /// A picture tab: the image at its size, scaled down to fit, over a checkerboard-free backdrop.
    fn render_image(&self, path: &PackPath, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let c = theme.colors;
        let full = self
            .project
            .as_ref()
            .and_then(|ws| ws.override_path(path).ok());
        let size = full
            .as_ref()
            .and_then(|p| image::image_dimensions(p).ok())
            .map(|(w, h)| format!("{w}×{h}"))
            .unwrap_or_default();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(c.row)
                    .font_family(theme.mono.clone())
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(path.as_str().to_owned()),
                    )
                    .child(size),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(24.))
                    .bg(c.bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(full.map(|p| {
                        use gpui_kit::StyledImage as _;
                        gpui_kit::img(p)
                            .max_w_full()
                            .max_h_full()
                            .object_fit(gpui_kit::ObjectFit::ScaleDown)
                    })),
            )
            .into_any_element()
    }

    /// An editor tab: the file's path, Save, and the text with line numbers.
    pub(super) fn render_file(&self, path: &PackPath, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let c = theme.colors;
        let mono = theme.mono.clone();
        if is_image(path.as_str()) {
            return self.render_image(path, cx);
        }
        let Some(file) = self.files.get(path) else {
            return div().into_any_element();
        };
        let read_only = *path == project_file();
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
        file.editor.update(cx, |s, _| s.set_editor_style(style));
        let save_path = path.clone();
        let key_path = path.clone();
        v_flex()
            .size_full()
            .child(
                h_flex()
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
                            .child(path.as_str().to_owned()),
                    )
                    .when(read_only, |row| {
                        row.child(
                            h_flex()
                                .gap(px(6.))
                                .text_color(c.muted)
                                .child(icon(IconName::Lock, c.muted).size(px(13.)))
                                .child(t!("dev.read_only").to_string()),
                        )
                    })
                    .when(!read_only, |row| {
                        row.child(
                            Button::new("dev-save")
                                .size(ButtonSize::Xs)
                                .label(t!("dev.save"))
                                .tooltip(t!("dev.save_hint"))
                                .disabled(!file.dirty)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_file(&save_path, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .id("dev-editor")
                    .flex_1()
                    .min_h_0()
                    .bg(c.bg)
                    .font_family(mono)
                    .text_size(px(12.5))
                    .line_height(px(20.))
                    .text_color(c.text)
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        let key = &event.keystroke;
                        if key.modifiers.secondary() && key.key == "s" {
                            cx.stop_propagation();
                            this.save_file(&key_path, cx);
                        }
                    }))
                    .child(Editor::new(&file.editor)),
            )
            .into_any_element()
    }
}
