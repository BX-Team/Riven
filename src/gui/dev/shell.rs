use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use rust_i18n::t;

use super::git::{GitOp, Repo};
use super::{Activity, Check, DevView, Panel, Tab, Updates, dialogs, display_path};
use crate::gui::session::Phase;
use crate::gui::state::{AppState, Route};
use crate::gui::theme::ActiveTheme as _;
use crate::gui::ui::{
    ActionMenu, Button, ButtonSize, IconName, MenuEntry, h_flex, icon, motion, tooltip, v_flex,
};

pub(super) const TOP_HEIGHT: f32 = 42.;
pub(super) const ACTIVITY_WIDTH: f32 = 46.;
const STATUS_HEIGHT: f32 = 26.;

impl DevView {
    /// The project menu: recent projects, opening a folder, creating a project.
    fn project_menu(&self, cx: &mut Context<Self>) -> ActionMenu {
        let name: SharedString = self
            .project
            .as_ref()
            .map(|ws| ws.project.name.clone().into())
            .unwrap_or_else(|| t!("dev.no_project").into());
        let trigger = Button::new("dev-project")
            .ghost()
            .child(icon(IconName::Package, cx.theme().colors.text2).size(px(14.)))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
            .child(icon(IconName::ChevronDown, cx.theme().colors.muted).size(px(12.)));
        let current = self.project.as_ref().map(|ws| ws.dir.clone());
        let view = cx.entity().downgrade();
        let mut entries = Vec::new();
        let recent = AppState::global(cx)
            .read(cx)
            .settings
            .recent_projects
            .clone();
        if !recent.is_empty() {
            entries.push(MenuEntry::Caption(t!("dev.recent").into()));
        }
        for path in recent {
            let dir = PathBuf::from(&path);
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let open = current.as_ref() == Some(&dir);
            let view = view.clone();
            entries.push(
                MenuEntry::action(format!("{name}  {}", display_path(&dir)), move |_, cx| {
                    let dir = dir.clone();
                    let _ = view.update(cx, |this, cx| this.open(dir, cx));
                })
                .checked(open),
            );
        }
        entries.push(MenuEntry::Caption(t!("dev.new").into()));
        let browse = view.clone();
        entries.push(
            MenuEntry::action(t!("dev.open"), move |window, cx| {
                let _ = browse.update(cx, |this, cx| this.browse(window, cx));
            })
            .icon(IconName::Folder),
        );
        let create = view.clone();
        entries.push(
            MenuEntry::action(t!("dev.create"), move |window, cx| {
                dialogs::open_create(create.clone(), window, cx)
            })
            .icon(IconName::Plus),
        );
        for (origin, label, icon) in [
            (
                dialogs::Origin::Mrpack,
                t!("dev.from_mrpack"),
                IconName::Package,
            ),
            (
                dialogs::Origin::Packwiz,
                t!("dev.from_packwiz"),
                IconName::Package,
            ),
            (
                dialogs::Origin::Clone,
                t!("dev.from_clone"),
                IconName::Terminal,
            ),
        ] {
            let view = view.clone();
            let entry = MenuEntry::action(label, move |window, cx| {
                dialogs::open_new_from(view.clone(), origin, window, cx)
            })
            .icon(icon);
            entries.push(if origin == dialogs::Origin::Clone {
                entry.disabled(!self.git.installed)
            } else {
                entry
            });
        }
        ActionMenu::new("dev-project-menu", trigger, entries)
            .width(px(420.))
            .left()
    }

    /// The branch menu: local branches to switch to and a new one.
    fn branch_menu(&self, cx: &mut Context<Self>) -> Option<ActionMenu> {
        let Repo::Ready(status) = &self.git.repo else {
            return None;
        };
        let c = cx.theme().colors;
        let current = status.branch.clone();
        let label: SharedString = current
            .clone()
            .unwrap_or_else(|| t!("dev.git_detached").to_string())
            .into();
        let dirty = !status.files.is_empty();
        let trigger = Button::new("dev-branch")
            .ghost()
            .child(
                div()
                    .size(px(7.))
                    .rounded(px(4.))
                    .bg(if dirty { c.warn } else { c.ok }),
            )
            .child(div().text_color(c.text2).child(label))
            .child(icon(IconName::ChevronDown, c.muted).size(px(12.)));
        let view = cx.entity().downgrade();
        let busy = self.git.running.is_some() || !self.git.installed;
        let mut entries = vec![MenuEntry::Caption(t!("dev.git_branches").into())];
        for branch in &status.branches {
            let (view, name) = (view.clone(), branch.clone());
            let on = current.as_deref() == Some(branch.as_str());
            entries.push(
                MenuEntry::action(branch.clone(), move |_, cx| {
                    let name = name.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.run_git(
                            GitOp::Switch {
                                branch: name,
                                create: false,
                            },
                            |this, cx| this.after_checkout(cx),
                            cx,
                        )
                    });
                })
                .icon(IconName::GitBranch)
                .checked(on)
                .disabled(on || busy),
            );
        }
        entries.push(MenuEntry::Separator);
        entries.push(
            MenuEntry::action(t!("dev.git_new_branch"), move |window, cx| {
                dialogs::open_new_branch(view.clone(), window, cx)
            })
            .icon(IconName::Plus)
            .disabled(busy),
        );
        Some(ActionMenu::new("dev-branch-menu", trigger, entries).width(px(280.)))
    }

    fn layout_toggle(
        &self,
        id: &'static str,
        name: IconName,
        on: bool,
        tip: SharedString,
        toggle: fn(&mut riven_format::DevPanel),
        cx: &mut Context<Self>,
    ) -> Button {
        let c = cx.theme().colors;
        Button::new(id)
            .ghost()
            .child(icon(name, if on { c.text } else { c.muted }).size(px(15.)))
            .tooltip(tip)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_layout(toggle, cx);
                cx.notify();
            }))
    }

    /// Project and branch on the left, search in the middle, actions and panel switches on the right.
    pub(super) fn render_top(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let layout = self.layout(cx);
        let open = self.project.is_some();
        let view = cx.entity().downgrade();
        h_flex()
            .flex_none()
            .h(px(TOP_HEIGHT))
            .gap(px(6.))
            .px(px(8.))
            .border_b_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(self.project_menu(cx))
            .when(open, |row| {
                row.children(self.branch_menu(cx).map(|menu| {
                    h_flex()
                        .gap(px(2.))
                        .child(div().text_color(c.muted).child("/"))
                        .child(menu)
                }))
            })
            .child(div().flex_1())
            .when_some(self.busy.clone(), |row, text| {
                row.child(
                    h_flex()
                        .gap(px(8.))
                        .max_w(px(260.))
                        .text_color(c.muted)
                        .child(motion::spinner(
                            "dev-busy",
                            icon(IconName::Loader, c.muted).size(px(14.)),
                            cx,
                        ))
                        .child(div().truncate().child(text)),
                )
            })
            .when(open, |row| {
                let add = view.clone();
                row.child(
                    Button::new("dev-add-content")
                        .ghost()
                        .icon(IconName::Plus)
                        .label(t!("dev.add_content"))
                        .disabled(self.busy.is_some())
                        .on_click(move |_, window, cx| dialogs::open_add(add.clone(), window, cx)),
                )
                .child(
                    Button::new("dev-release")
                        .ghost()
                        .icon(IconName::Tag)
                        .label(t!("dev.release"))
                        .on_click(cx.listener(|this, _, _, cx| this.show(Tab::Releases, cx))),
                )
                .child(self.render_test_controls(cx))
                .child(div().w(px(1.)).h(px(20.)).mx(px(4.)).bg(c.border))
                .child(self.layout_toggle(
                    "dev-toggle-sidebar",
                    IconName::PanelLeft,
                    !layout.sidebar_hidden,
                    t!("dev.toggle_sidebar").into(),
                    |p| p.sidebar_hidden = !p.sidebar_hidden,
                    cx,
                ))
                .child(self.layout_toggle(
                    "dev-toggle-panel",
                    IconName::PanelBottom,
                    !layout.hidden,
                    t!("dev.toggle_panel").into(),
                    |p| p.hidden = !p.hidden,
                    cx,
                ))
                .child(self.layout_toggle(
                    "dev-toggle-inspector",
                    IconName::PanelRight,
                    !layout.inspector_hidden,
                    t!("dev.toggle_inspector").into(),
                    |p| p.inspector_hidden = !p.inspector_hidden,
                    cx,
                ))
            })
    }

    fn activity_button(
        &self,
        activity: Activity,
        name: IconName,
        tip: SharedString,
        badge: Option<(usize, Hsla)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let on = self.activity == activity && !self.layout(cx).sidebar_hidden;
        div()
            .id(SharedString::from(format!("dev-activity-{activity:?}")))
            .relative()
            .size(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .cursor_pointer()
            .when(on, |d| d.bg(c.sel))
            .when(!on, |d| d.hover(|s| s.bg(c.row)))
            .tooltip(tooltip(tip))
            .on_click(
                cx.listener(move |this, _, window, cx| this.pick_activity(activity, window, cx)),
            )
            .child(icon(name, if on { c.text } else { c.muted }).size(px(18.)))
            .when(on, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(-5.))
                        .top(px(9.))
                        .w(px(3.))
                        .h(px(18.))
                        .rounded(px(2.))
                        .bg(c.accent),
                )
            })
            .when_some(badge.filter(|(n, _)| *n > 0), |d, (n, color)| {
                d.child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .right(px(0.))
                        .min_w(px(15.))
                        .h(px(15.))
                        .px(px(3.))
                        .rounded(px(8.))
                        .bg(color)
                        .text_color(c.on_accent)
                        .text_size(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if n > 99 { "99+".into() } else { n.to_string() }),
                )
            })
            .into_any_element()
    }

    /// The narrow bar of sidebar switches, with the launcher settings at the bottom.
    pub(super) fn render_activity(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let updates = self.updatable().len();
        let changes = match &self.git.repo {
            Repo::Ready(status) => status.files.len(),
            _ => 0,
        };
        v_flex()
            .flex_none()
            .w(px(ACTIVITY_WIDTH))
            .h_full()
            .items_center()
            .gap(px(4.))
            .py(px(8.))
            .border_r_1()
            .border_color(c.border)
            .bg(c.panel)
            .child(self.activity_button(
                Activity::Content,
                IconName::LayoutGrid,
                t!("dev.content").into(),
                Some((updates, c.accent)),
                cx,
            ))
            .child(self.activity_button(
                Activity::Files,
                IconName::File,
                t!("dev.files").into(),
                None,
                cx,
            ))
            .child(
                self.activity_button(
                    Activity::Search,
                    IconName::Search,
                    format!(
                        "{} ({})",
                        t!("dev.search"),
                        if cfg!(target_os = "macos") {
                            "⌘K"
                        } else {
                            "Ctrl K"
                        }
                    )
                    .into(),
                    None,
                    cx,
                ),
            )
            .child(self.activity_button(
                Activity::Git,
                IconName::GitBranch,
                "Git".into(),
                Some((changes, c.warn)),
                cx,
            ))
            .child(div().flex_1())
            .child(
                Button::new("dev-open-settings")
                    .ghost()
                    .child(icon(IconName::Settings, c.muted).size(px(17.)))
                    .tooltip(t!("sidebar.settings"))
                    .on_click(|_, _, cx| {
                        AppState::global(cx).update(cx, |s, cx| s.navigate(Route::Settings, cx))
                    }),
            )
    }

    fn tab_label(&self, tab: &Tab) -> SharedString {
        let project = self.project.as_ref().map(|ws| &ws.project);
        match tab {
            Tab::Pack => project.map(|p| p.name.clone()).unwrap_or_default().into(),
            Tab::List(view) => view.label(project),
            Tab::Entry(id) => project
                .and_then(|p| p.entry(id))
                .map_or_else(|| id.clone(), |e| e.name.clone())
                .into(),
            Tab::File(path) => path.file_name().to_owned().into(),
            Tab::Releases => t!("dev.releases").into(),
            Tab::History => t!("dev.history").into(),
        }
    }

    pub(super) fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let closable = self.tabs.len() > 1;
        h_flex()
            .id("dev-tabs")
            .flex_none()
            .h(px(34.))
            .overflow_x_scroll()
            .bg(c.panel)
            .border_b_1()
            .border_color(c.border)
            .children(self.tabs.iter().map(|tab| {
                let on = *tab == self.active;
                let dirty = match tab {
                    Tab::File(path) => self.files.get(path).is_some_and(|f| f.dirty),
                    _ => false,
                };
                let key = tab.key();
                let (show, close) = (tab.clone(), tab.clone());
                h_flex()
                    .id(SharedString::from(format!("dev-tab-{key}")))
                    .flex_none()
                    .relative()
                    .h_full()
                    .pl(px(12.))
                    .pr(px(if closable { 6. } else { 14. }))
                    .gap(px(7.))
                    .border_r_1()
                    .border_color(c.border)
                    .cursor_pointer()
                    .map(|t| {
                        if on {
                            t.bg(c.bg).text_color(c.text)
                        } else {
                            t.text_color(c.muted).hover(|s| s.text_color(c.text2))
                        }
                    })
                    .when(on, |t| {
                        t.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h(px(2.))
                                .bg(c.accent),
                        )
                    })
                    .when_some(
                        match tab {
                            Tab::File(path) => Some(path.as_str().to_owned()),
                            _ => None,
                        },
                        |t, path| t.tooltip(tooltip(path.into())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.show(show.clone(), cx)))
                    .child(icon(tab.icon(), if on { c.text2 } else { c.muted }).size(px(13.)))
                    .child(div().max_w(px(220.)).truncate().child(self.tab_label(tab)))
                    .when(dirty, |t| t.child(div().text_color(c.accent).child("●")))
                    .when(closable, |t| {
                        t.child(
                            Button::new(SharedString::from(format!("dev-close-{key}")))
                                .ghost()
                                .size(ButtonSize::Xs)
                                .icon(IconName::Close)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_tab(close.clone(), cx)
                                })),
                        )
                    })
            }))
    }

    fn status_item(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let c = cx.theme().colors;
        h_flex()
            .id(id)
            .h_full()
            .px(px(8.))
            .gap(px(5.))
            .cursor_pointer()
            .hover(|s| s.bg(c.row).text_color(c.text))
    }

    /// Git sync, problems, the test instance and the pack's game on one line at the bottom.
    pub(super) fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = cx.theme().colors;
        let mono = cx.theme().mono.clone();
        let problems = self.problem_count();
        let (remote, sync) = match &self.git.repo {
            Repo::Ready(status) => (
                Some(match &status.upstream {
                    Some(up) => format!(
                        "{} ↑{} ↓{}",
                        up.split('/').next().unwrap_or(up),
                        status.ahead,
                        status.behind
                    ),
                    None => t!("dev.git_local").to_string(),
                }),
                (status.ahead + status.behind > 0)
                    .then(|| t!("dev.status_sync", push = status.ahead, pull = status.behind)),
            ),
            Repo::None => (Some(t!("dev.git_none_short").to_string()), None),
            _ => (None, None),
        };
        let checking = matches!(self.check, Check::Running);
        let updates = match &self.updates {
            Updates::Ready(plan) => Some(plan.update.len()),
            _ => None,
        };
        let state = AppState::global(cx).read(cx);
        let test = self.test_instance().and_then(|id| {
            let instance = state.instance(&id)?;
            let phase = state.sessions.get(&id).map(|s| s.phase.clone());
            Some((instance.name.clone(), phase))
        });
        let game = self.project.as_ref().map(|ws| {
            let p = &ws.project;
            format!(
                "{} · {} {}",
                p.minecraft,
                crate::gui::launch_bar::loader_display(p.loader.kind),
                p.loader.version
            )
        });
        h_flex()
            .flex_none()
            .h(px(STATUS_HEIGHT))
            .px(px(4.))
            .border_t_1()
            .border_color(c.border)
            .bg(c.panel)
            .text_size(px(12.))
            .text_color(c.muted)
            .when_some(remote, |row, remote| {
                row.child(
                    self.status_item("dev-status-git", cx)
                        .child(icon(IconName::GitBranch, c.muted).size(px(12.)))
                        .child(remote)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.activity = Activity::Git;
                            if this.layout(cx).sidebar_hidden {
                                this.pick_activity(Activity::Git, window, cx);
                            }
                            cx.notify();
                        })),
                )
            })
            .child(
                self.status_item("dev-status-problems", cx)
                    .child(
                        icon(
                            IconName::Alert,
                            if problems > 0 {
                                crate::gui::theme::danger()
                            } else {
                                c.muted
                            },
                        )
                        .size(px(12.)),
                    )
                    .child(if checking {
                        t!("dev.checking").to_string()
                    } else {
                        problems.to_string()
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.open_panel(Panel::Problems, cx))),
            )
            .when_some(updates.filter(|n| *n > 0), |row, n| {
                row.child(
                    self.status_item("dev-status-updates", cx)
                        .text_color(c.accent)
                        .child(icon(IconName::Refresh, c.accent).size(px(12.)))
                        .child(t!("dev.update_n", n = n).to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show(Tab::List(super::View::Updates), cx)
                        })),
                )
            })
            .child(div().flex_1())
            .when_some(test, |row, (name, phase)| {
                let (dot, text) = match &phase {
                    Some(Phase::Running { since, .. }) => (
                        c.ok,
                        t!(
                            "dev.status_test_running",
                            time = since.elapsed().as_secs() / 60
                        )
                        .to_string(),
                    ),
                    Some(Phase::Working { stage, .. }) => {
                        (c.accent, crate::gui::launch_bar::stage_label(*stage))
                    }
                    Some(Phase::Failed(_)) => (
                        crate::gui::theme::danger(),
                        t!("dev.test_failed").to_string(),
                    ),
                    _ => (c.muted, t!("dev.status_test_idle").to_string()),
                };
                row.child(
                    self.status_item("dev-status-test", cx)
                        .child(div().size(px(7.)).rounded(px(4.)).bg(dot))
                        .child(name)
                        .child(div().text_color(c.muted).child(text))
                        .on_click(cx.listener(|this, _, _, cx| this.open_panel(Panel::Log, cx))),
                )
            })
            .when_some(sync, |row, sync| {
                row.child(
                    self.status_item("dev-status-sync", cx)
                        .child(sync.to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.pick_activity(Activity::Git, window, cx)
                        })),
                )
            })
            .child(
                self.status_item("dev-status-review", cx)
                    .child(
                        div()
                            .text_color(c.text2)
                            .child(t!("dev.review").to_string()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.show(Tab::History, cx))),
            )
            .when_some(game, |row, game| {
                row.child(div().px(px(8.)).font_family(mono).child(game))
            })
    }

    /// Opens the bottom panel on `panel`.
    pub(super) fn open_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.panel = panel;
        if self.layout(cx).hidden {
            self.set_layout(|p| p.hidden = false, cx);
        }
        self.panel_seen = usize::MAX;
        cx.notify();
    }
}
