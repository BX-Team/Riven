use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub const LOGO: &str = "brand/riven-128.png";
pub const FABRIC: &str = "icons/loader-fabric.svg";
pub const VANILLA: &str = "icons/loader-vanilla.svg";

const OWN: &[(&str, &[u8])] = &[
    (LOGO, include_bytes!("../../../assets/brand/riven-128.png")),
    (
        FABRIC,
        include_bytes!("../../../assets/icons/loader-fabric.svg"),
    ),
    (
        VANILLA,
        include_bytes!("../../../assets/icons/loader-vanilla.svg"),
    ),
];

pub fn font() -> Cow<'static, [u8]> {
    Cow::Borrowed(include_bytes!("../assets/fonts/InterVariable.ttf"))
}

/// The launcher's own pictures, then moonglass's icons.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match OWN.iter().find(|(p, _)| *p == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(*bytes))),
            None => moonglass::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = OWN
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| (*p).into())
            .collect();
        out.extend(moonglass::Assets.list(path)?);
        Ok(out)
    }
}
