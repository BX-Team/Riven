use gpui_kit::{Hsla, Styled as _, Svg, px, svg};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconName {
    ArrowDown,
    ArrowLeft,
    ArrowUp,
    Check,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    Close,
    Code,
    Compass,
    Copy,
    ExternalLink,
    File,
    Folder,
    Image,
    Loader,
    Lock,
    More,
    Package,
    Play,
    Plus,
    Power,
    Refresh,
    Search,
    Settings,
    Stop,
    Terminal,
    Trash,
    WindowClose,
    WindowMaximize,
    WindowMinimize,
    WindowRestore,
}

impl IconName {
    #[cfg(test)]
    pub const ALL: [IconName; 32] = [
        IconName::ArrowDown,
        IconName::ArrowLeft,
        IconName::ArrowUp,
        IconName::Check,
        IconName::ChevronDown,
        IconName::ChevronRight,
        IconName::ChevronUp,
        IconName::Close,
        IconName::Code,
        IconName::Compass,
        IconName::Copy,
        IconName::ExternalLink,
        IconName::File,
        IconName::Folder,
        IconName::Image,
        IconName::Loader,
        IconName::Lock,
        IconName::More,
        IconName::Package,
        IconName::Play,
        IconName::Plus,
        IconName::Power,
        IconName::Refresh,
        IconName::Search,
        IconName::Settings,
        IconName::Stop,
        IconName::Terminal,
        IconName::Trash,
        IconName::WindowClose,
        IconName::WindowMaximize,
        IconName::WindowMinimize,
        IconName::WindowRestore,
    ];

    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::ArrowDown => "icons/arrow-down.svg",
            Self::ArrowLeft => "icons/arrow-left.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::Check => "icons/check.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::ChevronUp => "icons/chevron-up.svg",
            Self::Close => "icons/close.svg",
            Self::Code => "icons/code.svg",
            Self::Compass => "icons/compass.svg",
            Self::Copy => "icons/copy.svg",
            Self::ExternalLink => "icons/external-link.svg",
            Self::File => "icons/file.svg",
            Self::Folder => "icons/folder.svg",
            Self::Image => "icons/image.svg",
            Self::Loader => "icons/loader.svg",
            Self::Lock => "icons/lock.svg",
            Self::More => "icons/more.svg",
            Self::Package => "icons/package.svg",
            Self::Play => "icons/play.svg",
            Self::Plus => "icons/plus.svg",
            Self::Power => "icons/power.svg",
            Self::Refresh => "icons/refresh.svg",
            Self::Search => "icons/search.svg",
            Self::Settings => "icons/settings.svg",
            Self::Stop => "icons/stop.svg",
            Self::Terminal => "icons/terminal.svg",
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
