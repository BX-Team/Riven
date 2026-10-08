mod add_mods;
mod app;
mod assets;
mod chrome;
mod dev;
mod dialogs;
mod instance;
mod instance_settings;
mod java_picker;
mod launch_bar;
mod logs;
mod markdown;
mod mods;
mod new_instance;
mod runtime;
mod session;
mod settings;
mod state;
mod theme;
mod time;
mod toast;
mod ui;

use std::process::ExitCode;

use gpui_kit::{App, Bounds, WindowBounds, WindowOptions, px, size};

use state::AppState;

pub fn run() -> ExitCode {
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            let state = AppState::init(cx);
            let settings = state.read(cx).settings.clone();
            set_language(settings.language.as_deref());
            theme::apply(&settings.appearance, None, cx);
            if settings.reduce_motion {
                cx.set_reduce_motion(true);
            }
            open_main_window(cx);
            cx.activate(true);
        });
    ExitCode::SUCCESS
}

fn open_main_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(960.), px(600.))),
        app_id: Some("riven".into()),
        ..chrome::window_options()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title("Riven Launcher");
        app::RivenApp::view(window, cx)
    });
    if let Err(e) = opened {
        tracing::error!("cannot open the main window: {e}");
        cx.quit();
    }
}

/// Picks the UI language: the saved one, else the system's when we have it, else English.
pub fn set_language(saved: Option<&str>) {
    let system = sys_locale::get_locale().unwrap_or_default();
    let lang = match saved.unwrap_or(&system) {
        l if l.starts_with("ru") => "ru-RU",
        _ => "en-US",
    };
    rust_i18n::set_locale(lang);
}
