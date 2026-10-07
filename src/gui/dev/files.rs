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
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{
    ActionMenu, Button, ButtonSize, IconName, MenuEntry, caption, h_flex, icon, motion, v_flex,
};

const TREE_ROW: f32 = 26.;

/// One row of the file tree, lit while hovered; returns whether it is.
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

/// Folders above `path` inside `overrides/`, outermost first.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/')
        .map(move |(i, _)| &path[..i])
        .filter(|a| *a != OVERRIDES)
}

impl DevView {
    pub(super) fn refresh_tree(&mut self) {
        self.releases.changes = super::releases::Changes::Stale;
        self.tree = match &self.project {
            Some(ws) => ws.overrides().unwrap_or_else(|e| {
                self.error = Some(e.to_string().into());
                Vec::new()
            }),
            None => Vec::new(),
        };
    }

    pub(super) fn open_file(&mut self, path: PackPath, cx: &mut Context<Self>) {
        self.show(Tab::File(path), cx);
    }

    /// Reads the open file into an editor the first time its tab is shown.
    pub(super) fn ensure_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Tab::File(path) = self.active.clone() else {
            return;
        };
        if self.files.contains_key(&path) {
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
        for dir in ancestors(path.as_str()) {
            self.expanded.insert(dir.to_owned());
        }
        if folder {
            self.expanded.insert(path.as_str().to_owned());
        }
        self.refresh_tree();
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
        self.refresh_tree();
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
        let mut rows: Vec<AnyElement> = Vec::new();
        let project = project_file();
        let (top, _) = tree_row(
            PROJECT_FILE.into(),
            0,
            active.as_deref() == Some(PROJECT_FILE),
            window,
            cx,
        );
        rows.push(
            top.child(icon(IconName::File, c.muted).size(px(13.)))
                .child(div().truncate().child(PROJECT_FILE))
                .on_click(cx.listener(move |this, _, _, cx| this.open_file(project.clone(), cx)))
                .into_any_element(),
        );
        for entry in &self.tree {
            let path = entry.path.as_str();
            if !ancestors(path).all(|a| self.expanded.contains(a)) {
                continue;
            }
            let depth = ancestors(path).count();
            let open = self.expanded.contains(path);
            let (r, lit) = tree_row(
                path.to_owned(),
                depth,
                active.as_deref() == Some(path),
                window,
                cx,
            );
            let key = entry.path.clone();
            let glyph = match (entry.dir, open) {
                (true, true) => IconName::ChevronDown,
                (true, false) => IconName::ChevronRight,
                (false, _) => IconName::File,
            };
            let dir = entry.dir;
            let mut menu = Vec::new();
            if dir {
                for folder in [false, true] {
                    let (view, at) = (view.clone(), key.as_str().to_owned());
                    let label = if folder {
                        t!("dev.new_folder")
                    } else {
                        t!("dev.new_file")
                    };
                    menu.push(
                        MenuEntry::action(label, move |window, cx| {
                            dialogs::open_new_path(
                                view.clone(),
                                format!("{at}/"),
                                folder,
                                window,
                                cx,
                            )
                        })
                        .icon(if folder {
                            IconName::Folder
                        } else {
                            IconName::Plus
                        }),
                    );
                }
            }
            {
                let key = key.clone();
                let view = view.clone();
                menu.push(
                    MenuEntry::action(t!("dev.remove"), move |_, cx| {
                        let key = key.clone();
                        let _ = view.update(cx, |this, cx| this.confirm_delete(key, cx));
                    })
                    .icon(IconName::Trash)
                    .danger(),
                );
            }
            rows.push(
                r.child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .gap(px(6.))
                        .child(icon(glyph, c.muted).size(px(13.)))
                        .child(div().truncate().child(entry.path.file_name().to_owned()))
                        .on_mouse_down(
                            gpui_kit::MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if dir {
                                    let k = key.as_str().to_owned();
                                    if !this.expanded.remove(&k) {
                                        this.expanded.insert(k);
                                    }
                                    cx.notify();
                                } else {
                                    this.open_file(key.clone(), cx);
                                }
                            }),
                        ),
                )
                .child(
                    div().flex_none().when(!lit, |d| d.invisible()).child(
                        ActionMenu::new(
                            SharedString::from(format!("tree-menu-{path}")),
                            Button::new(SharedString::from(format!("tree-more-{path}")))
                                .ghost()
                                .size(ButtonSize::Xs)
                                .icon(IconName::More),
                            menu,
                        )
                        .width(px(190.)),
                    ),
                )
                .into_any_element(),
            );
        }
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
                v_flex()
                    .id("dev-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .font_family(mono)
                    .text_size(px(12.))
                    .gap(px(1.))
                    .children(rows),
            )
            .into_any_element()
    }

    /// An editor tab: the file's path, Save, and the text with line numbers.
    pub(super) fn render_file(&self, path: &PackPath, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let c = theme.colors;
        let mono = theme.mono.clone();
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
