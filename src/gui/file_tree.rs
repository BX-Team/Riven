use std::collections::HashSet;
use std::path::Path;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, EventEmitter, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Pixels, Point, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, UniformListScrollHandle, Window, div, px, uniform_list,
};

use super::theme::ActiveTheme as _;
use super::ui::{IconName, h_flex, icon, scrollbar};

const ROW: f32 = 26.;

/// A file or folder of a tree, by its `/`-separated path under the tree's root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub path: String,
    pub dir: bool,
}

impl Node {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// The folder holding it, `""` at the root.
    pub fn parent(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(p, _)| p)
    }
}

pub enum TreeEvent {
    Open(String),
    /// A right click on a node, or on the empty space below them (`None`).
    Menu {
        node: Option<Node>,
        position: Point<Pixels>,
    },
    /// A node dropped on a folder; `to` is `""` for the root.
    Move {
        from: String,
        to: String,
    },
}

/// What a dragged row carries.
#[derive(Clone)]
struct Dragged {
    path: String,
    name: SharedString,
}

struct Ghost(SharedString);

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        h_flex()
            .gap(px(6.))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .bg(c.panel)
            .border_1()
            .border_color(c.accent)
            .text_size(px(12.))
            .text_color(c.text)
            .child(icon(IconName::File, c.muted).size(px(13.)))
            .child(self.0.clone())
    }
}

pub fn is_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Folders above `path`, outermost first.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(i, _)| &path[..i])
}

/// A folder tree that lists only the rows in view, so large folders stay smooth.
pub struct FileTree {
    id: SharedString,
    nodes: Vec<Node>,
    expanded: HashSet<String>,
    active: Option<String>,
    /// Indexes of the nodes whose folders are all open.
    shown: Vec<usize>,
    movable: bool,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<TreeEvent> for FileTree {}

impl FileTree {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            nodes: Vec::new(),
            expanded: HashSet::new(),
            active: None,
            shown: Vec::new(),
            movable: false,
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// Lets rows be dragged onto folders.
    pub fn movable(mut self) -> Self {
        self.movable = true;
        self
    }

    pub fn set_nodes(&mut self, nodes: Vec<Node>, cx: &mut Context<Self>) {
        self.nodes = nodes;
        self.refresh(cx);
    }

    pub fn set_active(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if self.active != path {
            self.active = path;
            cx.notify();
        }
    }

    /// Opens `path` and every folder above it.
    pub fn reveal(&mut self, path: &str, cx: &mut Context<Self>) {
        for dir in ancestors(path) {
            self.expanded.insert(dir.to_owned());
        }
        self.refresh(cx);
    }

    pub fn expand(&mut self, path: &str, cx: &mut Context<Self>) {
        self.expanded.insert(path.to_owned());
        self.refresh(cx);
    }

    fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_owned());
        }
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let expanded = &self.expanded;
        self.shown = (0..self.nodes.len())
            .filter(|&i| ancestors(&self.nodes[i].path).all(|a| expanded.contains(a)))
            .collect();
        cx.notify();
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let c = cx.theme().colors;
        let node = self.nodes.get(*self.shown.get(ix)?)?.clone();
        let depth = ancestors(&node.path).count();
        let open = self.expanded.contains(&node.path);
        let on = self.active.as_deref() == Some(node.path.as_str());
        let glyph = match (node.dir, open) {
            (true, true) => IconName::ChevronDown,
            (true, false) => IconName::ChevronRight,
            (false, _) if is_image(&node.path) => IconName::Image,
            (false, _) => IconName::File,
        };
        let clicked = node.clone();
        let menu = node.clone();
        let row = h_flex()
            .id(SharedString::from(format!("{}-{}", self.id, node.path)))
            .w_full()
            .h(px(ROW))
            .pl(px(8. + depth as f32 * 12.))
            .pr(px(4.))
            .gap(px(6.))
            .rounded(px(5.))
            .cursor_pointer()
            .map(|r| {
                if on {
                    r.bg(c.sel).text_color(c.text)
                } else {
                    r.text_color(c.text2)
                        .hover(|s| s.bg(c.row).text_color(c.text))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if clicked.dir {
                    this.toggle(&clicked.path, cx);
                } else {
                    cx.emit(TreeEvent::Open(clicked.path.clone()));
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_, event: &gpui_kit::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    cx.emit(TreeEvent::Menu {
                        node: Some(menu.clone()),
                        position: event.position,
                    });
                }),
            )
            .child(icon(glyph, c.muted).size(px(13.)))
            .child(div().truncate().child(node.name().to_owned()));
        let row = if self.movable {
            let dragged = Dragged {
                path: node.path.clone(),
                name: node.name().to_owned().into(),
            };
            let row = row.on_drag(dragged, |d, _, _, cx| cx.new(|_| Ghost(d.name.clone())));
            if node.dir {
                let to = node.path.clone();
                row.drag_over::<Dragged>(move |style, _, _, cx| {
                    style.bg(cx.theme().colors.accent.opacity(0.18))
                })
                .on_drop(cx.listener(move |_, dragged: &Dragged, _, cx| {
                    cx.stop_propagation();
                    cx.emit(TreeEvent::Move {
                        from: dragged.path.clone(),
                        to: to.clone(),
                    });
                }))
            } else {
                row
            }
        } else {
            row
        };
        Some(row.into_any_element())
    }
}

impl Render for FileTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mono = cx.theme().mono.clone();
        let movable = self.movable;
        div()
            .id(SharedString::from(format!("{}-area", self.id)))
            .relative()
            .size_full()
            .font_family(mono)
            .text_size(px(12.))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, event: &gpui_kit::MouseDownEvent, _, cx| {
                    cx.emit(TreeEvent::Menu {
                        node: None,
                        position: event.position,
                    });
                }),
            )
            .when(movable, |area| {
                area.on_drop(cx.listener(|_, dragged: &Dragged, _, cx| {
                    cx.emit(TreeEvent::Move {
                        from: dragged.path.clone(),
                        to: String::new(),
                    });
                }))
            })
            .child(
                uniform_list(
                    SharedString::from(format!("{}-list", self.id)),
                    self.shown.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|ix| this.render_row(ix, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .track_scroll(&self.scroll),
            )
            .child(scrollbar(&self.scroll))
    }
}

/// Files and folders under each existing `root/<top>` (all of `root` without `tops`), folders first, no symlinks.
pub fn scan(root: &Path, tops: &[&str]) -> Vec<Node> {
    fn walk(root: &Path, rel: &str, out: &mut Vec<Node>) {
        let Ok(entries) = std::fs::read_dir(root.join(rel)) else {
            return;
        };
        let mut found: Vec<(bool, String)> = entries
            .flatten()
            .filter_map(|e| {
                let kind = e.file_type().ok()?;
                (!kind.is_symlink())
                    .then(|| (!kind.is_dir(), e.file_name().to_string_lossy().into_owned()))
            })
            .collect();
        found.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });
        for (is_file, name) in found {
            let path = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            out.push(Node {
                path: path.clone(),
                dir: !is_file,
            });
            if !is_file {
                walk(root, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    if tops.is_empty() {
        walk(root, "", &mut out);
        return out;
    }
    for top in tops {
        let Ok(meta) = std::fs::symlink_metadata(root.join(top)) else {
            continue;
        };
        out.push(Node {
            path: (*top).to_owned(),
            dir: meta.is_dir(),
        });
        if meta.is_dir() {
            walk(root, top, &mut out);
        }
    }
    out
}

pub fn open_menu(position: Point<Pixels>, entries: Vec<super::ui::MenuEntry>, cx: &mut App) {
    super::state::AppState::global(cx)
        .update(cx, |s, cx| s.open_context_menu(position, entries, cx));
}
