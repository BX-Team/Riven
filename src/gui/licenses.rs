use std::sync::OnceLock;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, UniformListScrollHandle, Window, div, px, relative, uniform_list,
};
use gpui_base::input::{InputEvent, InputState};
use rust_i18n::t;

use super::dialogs;
use super::state::AppState;
use moonglass::theme::ActiveTheme as _;
use moonglass::{Button, ButtonSize, IconName, TextField, W_SEMIBOLD, h_flex, scrollbar, v_flex};

const LIST_WIDTH: f32 = 280.;
/// Room on the list's right for its scrollbar.
const GUTTER: f32 = 14.;
const HEIGHT: f32 = 440.;
const ROW_HEIGHT: f32 = 30.;
const LICENSE_WIDTH: f32 = 72.;

/// A license text and the components shipped under it, from `scripts/licenses.sh`.
struct License {
    id: String,
    name: String,
    /// Paragraphs with the hard line breaks of the license file joined, so the text wraps.
    paragraphs: Vec<String>,
}

/// One component and the license it ships under.
struct Component {
    name: SharedString,
    license: usize,
}

struct Catalog {
    licenses: Vec<License>,
    components: Vec<Component>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let raw: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/licenses.json")).unwrap_or_default();
        let mut licenses = Vec::new();
        let mut components = Vec::new();
        for entry in raw.as_array().into_iter().flatten() {
            let field = |key: &str| entry[key].as_str().unwrap_or_default().to_owned();
            let license = licenses.len();
            for name in entry["crates"].as_array().into_iter().flatten() {
                if let Some(name) = name.as_str() {
                    components.push(Component {
                        name: name.to_owned().into(),
                        license,
                    });
                }
            }
            licenses.push(License {
                id: field("id"),
                name: field("name"),
                paragraphs: paragraphs(&field("text")),
            });
        }
        components.sort_by_key(|c| c.name.to_lowercase());
        Catalog {
            licenses,
            components,
        }
    })
}

fn paragraphs(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .map(|p| p.lines().map(str::trim).collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect()
}

/// How many open-source components the build ships with; one under two licenses counts once.
pub fn count() -> usize {
    let names: std::collections::HashSet<&str> = catalog()
        .components
        .iter()
        .map(|c| c.name.as_ref())
        .collect();
    names.len()
}

pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| LicensesDialog::new(window, cx));
    dialogs::open(view.into(), 820., cx);
}

/// Every component with its license: a searchable list, and the selected one's text.
pub struct LicensesDialog {
    search: Entity<InputState>,
    /// Indexes into the catalog's components that match the search.
    shown: Vec<usize>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    text_scroll: gpui::ScrollHandle,
    _search: Subscription,
}

impl LicensesDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("licenses.search").to_string()));
        search.update(cx, |s, cx| s.focus(window, cx));
        let _search = cx.subscribe(&search, |this, input, event, cx| {
            if let InputEvent::Change = event {
                let filter = input.read(cx).value().to_lowercase();
                this.filter(&filter);
                cx.notify();
            }
        });
        let mut this = Self {
            search,
            shown: Vec::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            text_scroll: gpui::ScrollHandle::new(),
            _search,
        };
        this.filter("");
        this
    }

    fn filter(&mut self, filter: &str) {
        let c = catalog();
        self.shown = c
            .components
            .iter()
            .enumerate()
            .filter(|(_, comp)| {
                filter.is_empty()
                    || comp.name.to_lowercase().contains(filter)
                    || c.licenses[comp.license].id.to_lowercase().contains(filter)
            })
            .map(|(i, _)| i)
            .collect();
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let c = cx.theme().colors;
        let at = *self.shown.get(ix)?;
        let comp = &catalog().components[at];
        let selected = self.selected == Some(at);
        Some(
            h_flex()
                .id(("component", ix))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px(px(10.))
                .gap(px(8.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(selected, |r| r.bg(c.sel))
                .when(!selected, |r| r.hover(|r| r.bg(c.row)))
                .child(div().flex_1().min_w_0().truncate().child(comp.name.clone()))
                .child(
                    div()
                        .flex_none()
                        .w(px(LICENSE_WIDTH))
                        .text_right()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(c.muted)
                        .child(catalog().licenses[comp.license].id.clone()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(at);
                    this.text_scroll.set_offset(gpui::point(px(0.), px(0.)));
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    fn render_text(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let c = cx.theme().colors;
        let Some(at) = self.selected else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.muted)
                .child(t!("licenses.pick").to_string())
                .into_any_element();
        };
        let catalog = catalog();
        let comp = &catalog.components[at];
        let license = &catalog.licenses[comp.license];
        div()
            .relative()
            .size_full()
            .child(
                v_flex()
                    .id("license-text")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.text_scroll)
                    .pr(px(12.))
                    .gap(px(10.))
                    .child(
                        v_flex()
                            .gap(px(2.))
                            .child(
                                div()
                                    .font_weight(W_SEMIBOLD)
                                    .text_size(px(14.))
                                    .child(comp.name.clone()),
                            )
                            .child(div().text_color(c.muted).child(license.name.clone())),
                    )
                    .children(license.paragraphs.iter().map(|p| {
                        div()
                            .text_size(px(12.))
                            .line_height(relative(1.5))
                            .text_color(c.text2)
                            .child(p.clone())
                    })),
            )
            .child(scrollbar(&self.text_scroll))
            .into_any_element()
    }
}

impl Render for LicensesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let list = v_flex()
            .w(px(LIST_WIDTH + GUTTER))
            .flex_none()
            .h_full()
            .gap(px(8.))
            .child(
                div().mr(px(GUTTER)).child(
                    TextField::new(&self.search)
                        .leading(IconName::Search)
                        .clearable(),
                ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .pr(px(GUTTER))
                    .child(
                        uniform_list(
                            "components",
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
                    .child(scrollbar(&self.scroll)),
            );
        let body = h_flex().h(px(HEIGHT)).items_start().child(list).child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .pl(px(16.))
                .border_l_1()
                .border_color(c.border)
                .child(self.render_text(cx)),
        );
        dialogs::dialog_shell(
            t!("licenses.title"),
            t!("licenses.description", n = count()),
            body,
            vec![
                Button::new("close-licenses")
                    .outline()
                    .size(ButtonSize::Md)
                    .label(t!("common.close"))
                    .on_click(|_, _, cx| {
                        AppState::global(cx).update(cx, |s, cx| s.close_modal(cx))
                    }),
            ],
            cx,
        )
    }
}
