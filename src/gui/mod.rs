mod add_mods;
mod app;
mod assets;
mod changes;
mod clipboard;
mod code_editor;
mod configs;
mod crash;
mod dev;
mod dialogs;
mod file_tree;
mod help;
mod instance;
mod instance_settings;
mod java_picker;
mod launch_bar;
mod links;
mod logs;
mod markdown;
mod memory_slider;
mod mods;
mod new_instance;
mod pick;
mod runtime;
mod screenshots;
mod session;
mod settings;
mod state;
mod time;
mod updater;
mod welcome;

use std::process::ExitCode;

use gpui_kit::{App, Bounds, Window, WindowBounds, WindowOptions, px, size};
use riven_format::{Appearance, ThemeMode};

use state::AppState;

/// Whether a lone argument is something for the launcher to open rather than a CLI command.
pub fn opens(arg: &str) -> bool {
    links::Open::parse(arg).is_some()
}

pub fn run(arg: Option<String>) -> ExitCode {
    let open = arg.as_deref().and_then(links::Open::parse);
    let (tx, rx) = std::sync::mpsc::channel();
    match links::claim(open.as_ref()) {
        links::Claim::Forwarded => return ExitCode::SUCCESS,
        links::Claim::Primary(listener) => links::serve(listener, tx.clone()),
        links::Claim::Alone => {}
    }
    if let Some(open) = open {
        let _ = tx.send(open);
    }
    let app = gpui_kit::application().with_assets(assets::Assets);
    app.on_open_urls(move |urls| {
        for open in urls.iter().filter_map(|u| links::Open::parse(u)) {
            let _ = tx.send(open);
        }
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        moonglass::init(cx);
        dev::init(cx);
        app::init(cx);
        let state = AppState::init(cx);
        let settings = state.read(cx).settings.clone();
        set_language(settings.language.as_deref(), cx);
        apply_theme(&settings.appearance, None, cx);
        if settings.reduce_motion {
            cx.set_reduce_motion(true);
        }
        open_main_window(cx);
        cx.activate(true);
        links::listen(rx, cx);
        links::register();
    });
    ExitCode::SUCCESS
}

fn open_main_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(960.), px(600.))),
        app_id: Some("riven".into()),
        ..moonglass::chrome::window_options()
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
pub fn set_language(saved: Option<&str>, cx: &mut App) {
    let system = sys_locale::get_locale().unwrap_or_default();
    let lang = match saved.unwrap_or(&system) {
        l if l.starts_with("ru") => "ru-RU",
        _ => "en-US",
    };
    rust_i18n::set_locale(lang);
    moonglass::set_labels(
        moonglass::Labels {
            clear: rust_i18n::t!("common.clear").into(),
        },
        cx,
    );
}

/// Switches to the theme the settings pick for the current light or dark mode.
pub fn apply_theme(appearance: &Appearance, window: Option<&mut Window>, cx: &mut App) {
    let mode = match appearance.mode {
        ThemeMode::System => moonglass::ThemeMode::System,
        ThemeMode::Light => moonglass::ThemeMode::Light,
        ThemeMode::Dark => moonglass::ThemeMode::Dark,
    };
    moonglass::theme::apply(mode, &appearance.dark, &appearance.light, window, cx);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    fn keys(locale: &str) -> BTreeSet<String> {
        crate::_rust_i18n_backend()
            .messages_for_locale(locale)
            .unwrap_or_else(|| panic!("no locale {locale}"))
            .into_iter()
            .map(|(k, _)| k.into_owned())
            .filter(|k| k != "_version")
            .collect()
    }

    fn sources(dir: &Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push_str(&std::fs::read_to_string(&path).unwrap());
            }
        }
    }

    /// The literal keys of translation calls; `format!` ends in the same three characters.
    fn used_keys(src: &str) -> BTreeSet<&str> {
        src.match_indices("t\x21(")
            .filter(|(i, _)| {
                !src[..*i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
            .filter_map(|(i, m)| src[i + m.len()..].trim_start().strip_prefix('"'))
            .filter_map(|rest| rest.split_once('"').map(|(key, _)| key))
            .collect()
    }

    #[test]
    fn locales_match_the_code() {
        let en = keys("en-US");
        let ru = keys("ru-RU");
        assert_eq!(
            en.symmetric_difference(&ru).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "en-US and ru-RU have different keys"
        );

        let mut src = String::new();
        sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut src);
        let missing: Vec<_> = used_keys(&src)
            .into_iter()
            .filter(|k| !en.contains(*k))
            .collect();
        assert!(
            missing.is_empty(),
            "keys missing from the locales: {missing:?}"
        );
        let unused: Vec<_> = en
            .iter()
            .filter(|k| !src.contains(&format!("\"{k}\"")))
            .collect();
        assert!(unused.is_empty(), "keys no code uses: {unused:?}");
    }
}
