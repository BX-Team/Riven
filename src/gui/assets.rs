use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};
use moonglass::IconName;
use riven_format::LoaderKind;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[
            $((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../../assets/icons/", $name, ".svg")))),*
        ];
    };
}

icons!(
    "loader-fabric",
    "loader-forge",
    "loader-neoforge",
    "loader-quilt",
    "loader-vanilla",
);

/// The launcher's own picture, drawn in lists and the About page.
pub const LOGO: &str = "brand/riven-128.png";

const BRAND: &[(&str, &[u8])] = &[(LOGO, include_bytes!("../../assets/brand/riven-128.png"))];

/// The desktop entry and file type an AppImage or unpacked archive registers for itself.
#[cfg(target_os = "linux")]
pub const DESKTOP_ENTRY: &str = include_str!("../../packaging/linux/riven.desktop");
#[cfg(target_os = "linux")]
pub const MIME_INFO: &str = include_str!("../../packaging/linux/riven-mime.xml");
#[cfg(target_os = "linux")]
pub const ICON_512: &[u8] = include_bytes!("../../assets/brand/riven-512.png");

/// A loader's mark; `None` is vanilla.
pub fn loader_icon(kind: Option<LoaderKind>) -> IconName {
    IconName::Custom(match kind {
        None => "icons/loader-vanilla.svg",
        Some(LoaderKind::Fabric) => "icons/loader-fabric.svg",
        Some(LoaderKind::Forge) => "icons/loader-forge.svg",
        Some(LoaderKind::NeoForge) => "icons/loader-neoforge.svg",
        Some(LoaderKind::Quilt) => "icons/loader-quilt.svg",
    })
}

/// The launcher's own icons and pictures, then moonglass's.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match ICONS.iter().chain(BRAND).find(|(p, _)| *p == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(*bytes))),
            None => moonglass::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = ICONS
            .iter()
            .chain(BRAND)
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| (*p).into())
            .collect();
        out.extend(moonglass::Assets.list(path)?);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use riven_format::LoaderKind;

    #[test]
    fn every_loader_icon_is_embedded() {
        let kinds = [
            None,
            Some(LoaderKind::Fabric),
            Some(LoaderKind::Forge),
            Some(LoaderKind::NeoForge),
            Some(LoaderKind::Quilt),
        ];
        for kind in kinds {
            let path = super::loader_icon(kind).path();
            assert!(
                super::ICONS.iter().any(|(p, _)| *p == path),
                "{path} is missing from the icons! list"
            );
        }
    }
}
