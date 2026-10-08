use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{App, ClipboardItem, Context, EntityId, Global, SharedString};

use super::{Button, IconName};

/// How long a copy button shows its check.
pub const COPIED: Duration = Duration::from_millis(1500);

/// Copy buttons that copied a moment ago, by id, with the copy's serial.
#[derive(Default)]
struct Copies {
    marks: HashMap<SharedString, u64>,
    serial: u64,
}

impl Global for Copies {}

/// Whether the copy button `id` copied a moment ago.
pub fn copied(id: &str, cx: &App) -> bool {
    cx.try_global::<Copies>()
        .is_some_and(|c| c.marks.contains_key(id))
}

/// Puts `text` on the clipboard and shows the check on button `id` of `view` for a moment.
pub fn copy(id: SharedString, text: String, view: EntityId, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    mark(id, view, cx);
}

/// Shows the check on button `id` of `view` for a moment, after something else copied.
pub fn mark(id: SharedString, view: EntityId, cx: &mut App) {
    let copies = cx.default_global::<Copies>();
    copies.serial += 1;
    let serial = copies.serial;
    copies.marks.insert(id.clone(), serial);
    cx.notify(view);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(COPIED).await;
        cx.update(|cx| {
            let copies = cx.default_global::<Copies>();
            if copies.marks.get(&id) == Some(&serial) {
                copies.marks.remove(&id);
                cx.notify(view);
            }
        });
    })
    .detach();
}

/// A button with the copy icon that turns into a check once `text` is copied.
pub fn copy_button<V: 'static>(
    id: impl Into<SharedString>,
    text: impl Fn(&mut App) -> String + 'static,
    cx: &Context<V>,
) -> Button {
    let id = id.into();
    let view = cx.entity_id();
    Button::new(id.clone())
        .icon(if copied(&id, cx) {
            IconName::Check
        } else {
            IconName::Copy
        })
        .on_click(move |_, _, cx| {
            let text = text(cx);
            copy(id.clone(), text, view, cx)
        })
}
