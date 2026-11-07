mod add;
mod app;
mod assets;
mod data;
mod memory;
mod settings;

use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

/// The family of the font the demo ships, since a browser has none gpui can read.
const FONT: &str = "Inter Variable";

/// Opens the launcher window on made-up data: in the browser the page's canvas, natively a window.
pub fn run() {
    let (app, http) = application();
    app.with_assets(assets::Assets).run(move |cx| {
        if let Some(http) = http {
            cx.set_http_client(http);
        }
        moonglass::init(cx);
        load_font(cx);
        open(cx);
        cx.activate(true);
    });
}

/// The browser's fetch, so mod icons load from Modrinth's CDN as they do in the app.
type Http = Option<std::sync::Arc<dyn gpui::http_client::HttpClient>>;

#[cfg(target_family = "wasm")]
fn application() -> (gpui::Application, Http) {
    moonglass::platform::web_init();
    // One thread: shared memory would need cross-origin isolation headers on every page that embeds this.
    let platform = std::rc::Rc::new(moonglass::web::WebPlatform::new(false));
    let http = std::sync::Arc::new(platform.fetch_http_client());
    (gpui::Application::with_platform(platform), Some(http))
}

#[cfg(not(target_family = "wasm"))]
fn application() -> (gpui::Application, Http) {
    (moonglass::application(), None)
}

fn load_font(cx: &mut App) {
    let font = assets::font();
    if let Err(e) = cx.text_system().add_fonts(vec![font]) {
        tracing::warn!("cannot load the demo font: {e}");
        return;
    }
    let theme = cx.global_mut::<moonglass::theme::Theme>();
    theme.font = FONT.into();
    theme.mono = FONT.into();
}

fn open(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        app_id: Some("riven-demo".into()),
        ..Default::default()
    };
    let opened = moonglass::open_window(options, cx, |window, cx| {
        window.set_window_title("Riven Launcher");
        cx.new(|cx| app::Demo::new(window, cx))
    });
    if let Err(e) = opened {
        tracing::error!("cannot open the demo window: {e}");
    }
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    run();
}
