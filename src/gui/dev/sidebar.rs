use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use riven_format::{Kind, Side};
use rust_i18n::t;

use super::{Activity, DevView, Edge, Tab, Updates, View, dialogs, kind_label};
use moonglass::theme::ActiveTheme as _;
use moonglass::{
    ActionMenu, Button, ButtonSize, IconName, MenuEntry, TextField, h_flex, icon, v_flex,
};

pub(super) const SIDEBAR_WIDTH: f32 = 250.;
const ROW: f32 = 27.;
/// Results of each kind the search shows.
const SEARCH_LIMIT: usize = 60;

const KINDS: [Kind; 4] = [
    Kind::Mod,
    Kind::ResourcePack,
    Kind::ShaderPack,
    Kind::DataPack,
];

impl DevView {
    pub(super) fn render_sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let body = match self.activity {
            Activity::Content => self.render_content_sidebar(cx),
            Activity::Files => self.render_files(window, cx),
            Activity::Search => self.render_search_sidebar(cx),
            Activity::Git => self.render_git_sidebar(cx),
        };
        v_flex()
            .relative()
            .flex_shrink(1.)
            .min_w(px(180.))
            .w(px(self.width(Edge::Sidebar, cx)))
            .h_full()
            .border_r_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(body)
            .child(self.edge_handle(Edge::Sidebar, cx).right(px(-3.)))
            .into_any_element()
    }

    /// A sidebar heading with room for buttons on the right.
    pub(super) fn side_head(&self, title: SharedString, cx: &mut Context<Self>) -> gpui_kit::Div {
        let c = cx.theme().colors;
        h_flex()
            .flex_none()
            .h(px(36.))
            .pl(px(14.))
            .pr(px(6.))
            .gap(px(2.))
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.text)
                    .child(title),
            )
    }

    /// A row naming a view with how many entries it holds; selected while its tab is open.
    fn view_row(
        &self,
        view: View,
        label: SharedString,
        indent: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let count = self.view_count(&view);
        let on = self.active == Tab::List(view.clone());
        let mono = cx.theme().mono.clone();
        let tab = Tab::List(view.clone());
        h_flex()
            .id(SharedString::from(format!("dev-view-{view:?}")))
            .h(px(ROW))
            .mx(px(6.))
            .pl(px(if indent { 22. } else { 10. }))
            .pr(px(10.))
            .gap(px(8.))
            .rounded(px(6.))
            .cursor_pointer()
            .map(|r| {
                if on {
                    r.bg(c.sel).text_color(c.text)
                } else {
                    r.text_color(c.text2).hover(|s| s.bg(c.row))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| this.show(tab.clone(), cx)))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(
                div()
                    .flex_none()
                    .font_family(mono)
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .child(count.to_string()),
            )
            .into_any_element()
    }

    /// A foldable caption over a group of rows.
    fn fold_head(
        &self,
        key: &'static str,
        title: SharedString,
        extra: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let open = !self.folded.contains(key);
        h_flex()
            .id(SharedString::from(format!("dev-fold-{key}")))
            .h(px(ROW))
            .mt(px(8.))
            .mx(px(6.))
            .px(px(6.))
            .gap(px(4.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_color(c.text2)
            .font_weight(FontWeight::SEMIBOLD)
            .hover(|s| s.bg(c.row))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.folded.remove(key) {
                    this.folded.insert(key);
                }
                cx.notify();
            }))
            .child(
                icon(
                    if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                    c.muted,
                )
                .size(px(12.)),
            )
            .child(div().flex_1().child(title))
            .children(extra)
            .into_any_element()
    }

    fn content_menu(&self, cx: &mut Context<Self>) -> ActionMenu {
        let view = cx.entity().downgrade();
        let ready = matches!(self.updates, Updates::Ready(_));
        let checking = matches!(self.updates, Updates::Checking);
        let (check, review, pack, releases) =
            (view.clone(), view.clone(), view.clone(), view.clone());
        let entries = vec![
            MenuEntry::action(t!("dev.check_updates"), move |_, cx| {
                let _ = check.update(cx, |this, cx| this.check_updates(true, cx));
            })
            .icon(IconName::Refresh)
            .disabled(checking),
            MenuEntry::action(t!("dev.updates_title"), move |window, cx| {
                let _ = review.update(cx, |this, cx| this.review_updates(window, cx));
            })
            .icon(IconName::ArrowUp)
            .disabled(!ready),
            MenuEntry::Separator,
            MenuEntry::action(t!("dev.pack_overview"), move |_, cx| {
                let _ = pack.update(cx, |this, cx| this.show(Tab::Pack, cx));
            })
            .icon(IconName::Package),
            MenuEntry::action(t!("dev.releases"), move |_, cx| {
                let _ = releases.update(cx, |this, cx| this.show(Tab::Releases, cx));
            })
            .icon(IconName::Tag),
        ];
        ActionMenu::new(
            "dev-content-menu",
            Button::new("dev-content-more")
                .ghost()
                .size(ButtonSize::Xs)
                .icon(IconName::More),
            entries,
        )
        .width(px(240.))
    }

    fn render_content_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(ws) = &self.project else {
            return div().into_any_element();
        };
        let project = &ws.project;
        let view = cx.entity().downgrade();
        let add = view.clone();
        let mut rows: Vec<AnyElement> = KINDS
            .iter()
            .filter(|k| **k != Kind::DataPack || self.view_count(&View::Kind(**k)) > 0)
            .map(|k| self.view_row(View::Kind(*k), kind_label(*k), false, cx))
            .collect();
        rows.push(self.fold_head("views", t!("dev.views").into(), None, cx));
        if !self.folded.contains("views") {
            let views = [
                View::Updates,
                View::Changed,
                View::Problems,
                View::Dependencies,
                View::Side(Side::Client),
                View::Side(Side::Server),
                View::Disabled,
            ];
            for view in views {
                let label = view.label(Some(project));
                rows.push(self.view_row(view, label, true, cx));
            }
        }
        let new_group = view.clone();
        rows.push(
            self.fold_head(
                "groups",
                t!("dev.groups").into(),
                Some(
                    Button::new("dev-group-new")
                        .ghost()
                        .size(ButtonSize::Xs)
                        .icon(IconName::Plus)
                        .tooltip(t!("dev.group_add"))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            dialogs::open_group(new_group.clone(), None, window, cx)
                        })
                        .into_any_element(),
                ),
                cx,
            ),
        );
        if !self.folded.contains("groups") {
            if project.groups.is_empty() {
                rows.push(
                    div()
                        .px(px(28.))
                        .py(px(4.))
                        .text_size(px(12.))
                        .text_color(cx.theme().colors.muted)
                        .child(t!("dev.groups_none").to_string())
                        .into_any_element(),
                );
            }
            for group in &project.groups {
                rows.push(self.view_row(
                    View::Group(group.id.clone()),
                    group.name.clone().into(),
                    true,
                    cx,
                ));
            }
        }
        v_flex()
            .size_full()
            .child(
                self.side_head(t!("dev.content").into(), cx)
                    .child(
                        Button::new("dev-content-add")
                            .ghost()
                            .size(ButtonSize::Xs)
                            .icon(IconName::Plus)
                            .tooltip(t!("dev.add_content"))
                            .disabled(self.busy.is_some())
                            .on_click(move |_, window, cx| {
                                dialogs::open_add(add.clone(), window, cx)
                            }),
                    )
                    .child(self.content_menu(cx)),
            )
            .child(
                v_flex()
                    .id("dev-content-sidebar")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb(px(10.))
                    .gap(px(1.))
                    .children(rows),
            )
            .into_any_element()
    }

    fn search_row(
        &self,
        id: SharedString,
        name: IconName,
        title: String,
        detail: String,
        open: Tab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let on = self.active == open;
        h_flex()
            .id(id)
            .mx(px(6.))
            .px(px(10.))
            .py(px(5.))
            .gap(px(8.))
            .rounded(px(6.))
            .cursor_pointer()
            .map(|r| {
                if on {
                    r.bg(c.sel)
                } else {
                    r.hover(|s| s.bg(c.row))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| this.show(open.clone(), cx)))
            .child(icon(name, c.muted).size(px(13.)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().text_color(c.text).child(title))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .font_family(cx.theme().mono.clone())
                            .text_color(c.muted)
                            .child(detail),
                    ),
            )
            .into_any_element()
    }

    /// Entries and override files whose names hold the query.
    fn render_search_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let needle = self.search.read(cx).value().trim().to_lowercase();
        let mut rows = Vec::new();
        if let (Some(ws), false) = (&self.project, needle.is_empty()) {
            let entries: Vec<_> = ws
                .project
                .content
                .iter()
                .filter(|e| {
                    e.name.to_lowercase().contains(&needle)
                        || e.id.contains(&needle)
                        || e.file.path.as_str().to_lowercase().contains(&needle)
                })
                .take(SEARCH_LIMIT)
                .collect();
            let files: Vec<_> = self
                .override_files
                .iter()
                .filter(|p| p.as_str().to_lowercase().contains(&needle))
                .take(SEARCH_LIMIT)
                .collect();
            if !entries.is_empty() {
                rows.push(caption_row(t!("dev.content").into(), entries.len(), cx));
            }
            for (i, e) in entries.into_iter().enumerate() {
                rows.push(self.search_row(
                    SharedString::from(format!("dev-search-entry-{i}")),
                    IconName::Package,
                    e.name.clone(),
                    e.file.path.as_str().to_owned(),
                    Tab::Entry(e.id.clone()),
                    cx,
                ));
            }
            if !files.is_empty() {
                rows.push(caption_row(t!("dev.files").into(), files.len(), cx));
            }
            for (i, path) in files.into_iter().enumerate() {
                rows.push(self.search_row(
                    SharedString::from(format!("dev-search-file-{i}")),
                    IconName::File,
                    path.file_name().to_owned(),
                    path.as_str().to_owned(),
                    Tab::File(path.clone()),
                    cx,
                ));
            }
            if rows.is_empty() {
                rows.push(
                    div()
                        .px(px(16.))
                        .py(px(8.))
                        .text_color(c.muted)
                        .child(t!("dev.search_nothing").to_string())
                        .into_any_element(),
                );
            }
        }
        v_flex()
            .size_full()
            .child(self.side_head(t!("dev.search").into(), cx))
            .child(
                div().flex_none().px(px(10.)).pb(px(8.)).child(
                    TextField::new(&self.search)
                        .leading(IconName::Search)
                        .clearable(),
                ),
            )
            .child(
                v_flex()
                    .id("dev-search-results")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb(px(10.))
                    .gap(px(1.))
                    .children(rows),
            )
            .into_any_element()
    }
}

fn caption_row(title: SharedString, n: usize, cx: &Context<DevView>) -> AnyElement {
    let c = cx.theme().colors;
    h_flex()
        .mt(px(6.))
        .px(px(16.))
        .h(px(24.))
        .text_size(px(11.))
        .text_color(c.muted)
        .child(div().flex_1().child(title.to_uppercase()))
        .child(n.to_string())
        .into_any_element()
}
