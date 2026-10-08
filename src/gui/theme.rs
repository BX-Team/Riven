use gpui_kit::base::{ScrollbarMode, ScrollbarStyles, ScrollbarTheme, ThemeAppearance};
use std::time::{Duration, Instant};

use gpui_kit::base::Interpolate;
use gpui_kit::{App, Global, Hsla, Rgba, SharedString, Window, WindowAppearance, rgb};
use riven_format::{Appearance, ThemeMode};

/// The twelve colors every Riven theme defines.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Hsla,
    pub panel: Hsla,
    pub border: Hsla,
    pub row: Hsla,
    pub sel: Hsla,
    pub text: Hsla,
    pub text2: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub ok: Hsla,
    pub warn: Hsla,
}

impl Palette {
    fn from_hex(c: [u32; 12]) -> Self {
        let h = |i: usize| Hsla::from(rgb(c[i]));
        Self {
            bg: h(0),
            panel: h(1),
            border: h(2),
            row: h(3),
            sel: h(4),
            text: h(5),
            text2: h(6),
            muted: h(7),
            accent: h(8),
            on_accent: h(9),
            ok: h(10),
            warn: h(11),
        }
    }
}

fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let l = |x: f32, y: f32| x + (y - x) * t;
    Hsla::from(Rgba {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: l(a.a, b.a),
    })
}

impl Interpolate for Palette {
    fn interpolate(&self, to: &Self, t: f32) -> Self {
        Self {
            bg: mix(self.bg, to.bg, t),
            panel: mix(self.panel, to.panel, t),
            border: mix(self.border, to.border, t),
            row: mix(self.row, to.row, t),
            sel: mix(self.sel, to.sel, t),
            text: mix(self.text, to.text, t),
            text2: mix(self.text2, to.text2, t),
            muted: mix(self.muted, to.muted, t),
            accent: mix(self.accent, to.accent, t),
            on_accent: mix(self.on_accent, to.on_accent, t),
            ok: mix(self.ok, to.ok, t),
            warn: mix(self.warn, to.warn, t),
        }
    }
}

/// A built-in theme: its settings id, display name and colors in [`Palette`] order.
pub struct Spec {
    pub id: &'static str,
    pub name: &'static str,
    colors: [u32; 12],
}

pub const DARK: &[Spec] = &[
    Spec {
        id: "graphite",
        name: "Graphite",
        colors: [
            0x17181b, 0x121316, 0x26282d, 0x1f2125, 0x2a2d33, 0xe4e6eb, 0xb9bec7, 0x8e939c,
            0x6ea8fe, 0x0b1526, 0x7ccf8a, 0xe5b567,
        ],
    },
    Spec {
        id: "night",
        name: "Night",
        colors: [
            0x1a1b26, 0x16161e, 0x2a2c3d, 0x23253a, 0x292e42, 0xc0caf5, 0xa9b1d6, 0x8b93b8,
            0x7aa2f7, 0x10142a, 0x9ece6a, 0xe0af68,
        ],
    },
    Spec {
        id: "moss",
        name: "Moss",
        colors: [
            0x161a17, 0x121513, 0x252b26, 0x1d221e, 0x253027, 0xe2e8e1, 0xbcc6bc, 0x8f9a90,
            0x7fd18b, 0x0d1f11, 0xa6d189, 0xe0b86a,
        ],
    },
    Spec {
        id: "ember",
        name: "Ember",
        colors: [
            0x1c1a17, 0x171513, 0x2e2a25, 0x24211d, 0x332e28, 0xece4d8, 0xc9bfb0, 0xa39888,
            0xe8a15a, 0x241405, 0xa7c080, 0xe6c06b,
        ],
    },
];

pub const LIGHT: &[Spec] = &[
    Spec {
        id: "day",
        name: "Day",
        colors: [
            0xf6f7fb, 0xeceef5, 0xd8dbe7, 0xe5e7f0, 0xdde3f5, 0x1f2335, 0x3b4261, 0x5c6382,
            0x2e5bd8, 0xffffff, 0x2f7d32, 0x9a6200,
        ],
    },
    Spec {
        id: "paper",
        name: "Paper",
        colors: [
            0xfbfaf8, 0xf3f1ec, 0xe2ded6, 0xece9e2, 0xe7e3da, 0x22201c, 0x3d3a33, 0x6b665c,
            0x2f6f4f, 0xffffff, 0x2f7d32, 0x9a5b00,
        ],
    },
];

impl Spec {
    pub fn palette(&self) -> Palette {
        Palette::from_hex(self.colors)
    }
}

fn find(list: &'static [Spec], id: &str) -> &'static Spec {
    list.iter().find(|s| s.id == id).unwrap_or(&list[0])
}

/// The active colors and fonts.
pub struct Theme {
    pub colors: Palette,
    pub font: SharedString,
    pub mono: SharedString,
    /// Whether the palette in use is one of the dark ones.
    pub dark: bool,
    fade: Option<Fade>,
}

/// A switch between two palettes in progress.
struct Fade {
    from: Palette,
    to: Palette,
    dark: bool,
    started: Instant,
}

const FADE: Duration = Duration::from_millis(260);

impl Global for Theme {}

pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

/// The first family of `wanted` that is installed, else `fallback`.
fn installed(cx: &App, wanted: &[&str], fallback: &str) -> SharedString {
    let names = cx.text_system().all_font_names();
    wanted
        .iter()
        .find(|w| names.iter().any(|n| n == *w))
        .copied()
        .unwrap_or(fallback)
        .to_owned()
        .into()
}

/// The design asks for the system UI font; on Linux gpui's own pick is often missing, so look.
fn ui_font(cx: &App) -> SharedString {
    if cfg!(target_os = "linux") {
        installed(
            cx,
            &[
                "Inter",
                "Adwaita Sans",
                "Cantarell",
                "Noto Sans",
                "Ubuntu",
                "DejaVu Sans",
            ],
            ".SystemUIFont",
        )
    } else if cfg!(target_os = "windows") {
        installed(cx, &["Segoe UI Variable Text", "Segoe UI"], ".SystemUIFont")
    } else {
        ".SystemUIFont".into()
    }
}

fn mono_font(cx: &App) -> SharedString {
    installed(
        cx,
        &[
            "SF Mono",
            "JetBrains Mono",
            "JetBrainsMono Nerd Font",
            "Cascadia Mono",
            "Menlo",
            "Consolas",
            "DejaVu Sans Mono",
        ],
        "monospace",
    )
}

pub fn init(cx: &mut App) {
    let theme = Theme {
        colors: Palette::from_hex(DARK[0].colors),
        font: ui_font(cx),
        mono: mono_font(cx),
        dark: true,
        fade: None,
    };
    tracing::debug!("fonts: ui {}, mono {}", theme.font, theme.mono);
    cx.set_global(theme);
}

/// Switches to the theme the settings pick for the current light or dark mode.
pub fn apply(appearance: &Appearance, window: Option<&mut Window>, cx: &mut App) {
    let system = window
        .as_ref()
        .map(|w| w.appearance())
        .unwrap_or_else(|| cx.window_appearance());
    let dark = match appearance.mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => matches!(
            system,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    let spec = if dark {
        find(DARK, &appearance.dark)
    } else {
        find(LIGHT, &appearance.light)
    };
    let to = Palette::from_hex(spec.colors);
    let animate = !cx.reduce_motion() && !cx.windows().is_empty();
    let theme = cx.global_mut::<Theme>();
    theme.dark = dark;
    if animate {
        theme.fade = Some(Fade {
            from: theme.colors,
            to,
            dark,
            started: Instant::now(),
        });
    } else {
        theme.fade = None;
        theme.colors = to;
        sync_base(&to, dark, cx);
    }
    cx.refresh_windows();
}

/// The red of destructive buttons and the window's close button, the same in every theme.
pub fn danger() -> Hsla {
    gpui_kit::hsla(355. / 360., 0.72, 0.52, 1.)
}

/// Whether the palette is changing; transitions jump straight to their targets meanwhile.
pub fn fading(cx: &App) -> bool {
    cx.theme().fade.is_some()
}

/// Advances a palette change; the root view calls it first thing every frame.
pub fn tick(window: &mut Window, cx: &mut App) {
    let Some(fade) = &cx.theme().fade else {
        return;
    };
    let t = (fade.started.elapsed().as_secs_f32() / FADE.as_secs_f32()).min(1.);
    let eased = 1. - (1. - t).powi(3);
    let (colors, dark) = (fade.from.interpolate(&fade.to, eased), fade.dark);
    let theme = cx.global_mut::<Theme>();
    theme.colors = colors;
    if t >= 1. {
        theme.fade = None;
    }
    sync_base(&colors, dark, cx);
    // A refresh during drawing is ignored, and an animation frame alone keeps cached screens in
    // the old colors: ask for a full redraw from the next frame instead.
    window.on_next_frame(|window, _| window.refresh());
}

/// gpui-base paints carets, selections, placeholders and scrollbars from its own tokens.
fn sync_base(c: &Palette, dark: bool, cx: &mut App) {
    let base = gpui_kit::base::Theme::global_mut(cx);
    base.appearance = if dark {
        ThemeAppearance::Dark
    } else {
        ThemeAppearance::Light
    };
    let t = &mut base.tokens.colors;
    t.background = c.bg;
    t.foreground = c.text;
    t.surface = c.bg;
    t.surface_foreground = c.text;
    t.primary = c.accent;
    t.primary_foreground = c.on_accent;
    t.secondary = c.sel;
    t.secondary_foreground = c.text;
    t.muted = c.row;
    t.muted_foreground = c.muted;
    t.accent = c.accent;
    t.accent_foreground = c.on_accent;
    t.border = c.border;
    t.input = c.border;
    t.ring = c.accent;
    t.selection = c.accent.opacity(0.35);
    base.scrollbar = ScrollbarTheme::new()
        .with_mode(ScrollbarMode::Hover)
        .with_styles(
            ScrollbarStyles::default()
                .thumb(|s| {
                    s.bg(c.muted.opacity(0.35))
                        .width(gpui_kit::px(6.))
                        .radius(gpui_kit::px(3.))
                })
                .thumb_hover(|s| s.bg(c.muted.opacity(0.6))),
        );
}
