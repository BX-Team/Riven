use gpui_kit::{Hsla, Styled as _, Svg, px, svg};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconName {
    ArrowLeft,
    Check,
    ChevronDown,
    ChevronUp,
    Close,
    Code,
    Compass,
    Loader,
    Package,
    Play,
    Plus,
    Search,
    Settings,
    Trash,
    WindowClose,
    WindowMaximize,
    WindowMinimize,
    WindowRestore,
}

impl IconName {
    fn path(self) -> &'static str {
        match self {
            Self::ArrowLeft => "icons/arrow-left.svg",
            Self::Check => "icons/check.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronUp => "icons/chevron-up.svg",
            Self::Close => "icons/close.svg",
            Self::Code => "icons/code.svg",
            Self::Compass => "icons/compass.svg",
            Self::Loader => "icons/loader.svg",
            Self::Package => "icons/package.svg",
            Self::Play => "icons/play.svg",
            Self::Plus => "icons/plus.svg",
            Self::Search => "icons/search.svg",
            Self::Settings => "icons/settings.svg",
            Self::Trash => "icons/trash.svg",
            Self::WindowClose => "icons/window-close.svg",
            Self::WindowMaximize => "icons/window-maximize.svg",
            Self::WindowMinimize => "icons/window-minimize.svg",
            Self::WindowRestore => "icons/window-restore.svg",
        }
    }
}

/// A 15 px icon; svg elements do not inherit the text color, so it is passed in.
pub fn icon(name: IconName, color: Hsla) -> Svg {
    svg()
        .path(name.path())
        .size(px(15.))
        .flex_none()
        .text_color(color)
}
