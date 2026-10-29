use std::rc::Rc;

use gpui_kit::base::input::{self, EditorState, InputEditorStyle};
use gpui_kit::{Action, App, AppContext as _, Edges, Entity, Window, px};
use rust_i18n::t;

use moonglass::MenuEntry;
use moonglass::code::{self as highlight, Flavor, ThemeStyles};
use moonglass::theme::ActiveTheme as _;

/// A text editor for the file `name`: highlighting by its extension and the usual right-click menu.
pub fn new(
    name: &str,
    text: String,
    read_only: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    let editor = cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .language(Flavor::of(name).language())
            .default_value(text);
        state.set_highlighter_factory(highlight::factory(), cx);
        state.set_readonly(read_only, cx);
        state.set_editor_paddings(Edges {
            top: px(6.),
            left: px(12.),
            ..Edges::default()
        });
        state
    });
    let weak = editor.downgrade();
    editor.update(cx, |state, _| {
        state.on_context_menu(Rc::new(move |_, can, position, _, cx| {
            let Some(editor) = weak.upgrade() else {
                return;
            };
            let item = |label: String, enabled: bool, action: Box<dyn Action>| {
                let editor = editor.clone();
                MenuEntry::action(label, move |window, cx| {
                    editor.update(cx, |s, cx| s.focus(window, cx));
                    window.dispatch_action(action.boxed_clone(), cx);
                })
                .disabled(!enabled)
            };
            let entries = vec![
                item(
                    t!("editor.undo").into(),
                    can.is_editable(),
                    Box::new(input::Undo),
                ),
                item(
                    t!("editor.redo").into(),
                    can.is_editable(),
                    Box::new(input::Redo),
                ),
                MenuEntry::Separator,
                item(
                    t!("editor.cut").into(),
                    can.is_editable() && can.is_copyable(),
                    Box::new(input::Cut),
                ),
                item(
                    t!("editor.copy").into(),
                    can.is_copyable(),
                    Box::new(input::Copy),
                ),
                item(
                    t!("editor.paste").into(),
                    can.is_editable(),
                    Box::new(input::Paste),
                ),
                MenuEntry::Separator,
                item(
                    t!("editor.select_all").into(),
                    true,
                    Box::new(input::SelectAll),
                ),
            ];
            super::file_tree::open_menu(position, entries, cx);
        }));
    });
    editor
}

/// The editor's colors from the current theme.
pub fn style(cx: &App) -> InputEditorStyle {
    let c = cx.theme().colors;
    InputEditorStyle {
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
    }
}
