use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[
            $((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../../assets/icons/", $name, ".svg")))),*
        ];
    };
}

icons!(
    "arrow-down",
    "arrow-left",
    "arrow-up",
    "check",
    "chevron-down",
    "chevron-right",
    "chevron-up",
    "close",
    "code",
    "compass",
    "copy",
    "external-link",
    "file",
    "file-plus",
    "folder",
    "folder-plus",
    "image",
    "loader",
    "lock",
    "more",
    "package",
    "play",
    "plus",
    "power",
    "refresh",
    "search",
    "settings",
    "stop",
    "terminal",
    "trash",
    "upload",
    "window-close",
    "window-maximize",
    "window-minimize",
    "window-restore",
);

/// The launcher's own picture, drawn in lists and the About page.
pub const LOGO: &str = "brand/riven-128.png";

const BRAND: &[(&str, &[u8])] = &[(LOGO, include_bytes!("../../assets/brand/riven-128.png"))];

/// The icons drawn for the design, embedded in the binary.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .chain(BRAND)
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .chain(BRAND)
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| (*p).into())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::super::ui::IconName;

    #[test]
    fn every_icon_is_embedded() {
        for icon in IconName::ALL {
            let path = icon.path();
            assert!(
                super::ICONS.iter().any(|(p, _)| *p == path),
                "{path} is missing from the icons! list"
            );
        }
    }
}
