use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::Loader;

/// `<config>/riven/settings.json` — launcher-wide preferences and launch defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    #[serde(default)]
    pub appearance: Appearance,
    /// UI language (`en-US`, `ru-RU`); none follows the system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub developer_mode: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reduce_motion: bool,
    #[serde(default)]
    pub launch: LaunchSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_account: Option<String>,
    /// Pack project folders opened in the developer section, the latest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_projects: Vec<String>,
    #[serde(default, skip_serializing_if = "DevPanel::is_default")]
    pub dev_panel: DevPanel,
}

/// The bottom panel of the developer section (Check, Git, Log).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevPanel {
    /// Height in logical pixels.
    pub height: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

impl DevPanel {
    pub const DEFAULT_HEIGHT: u32 = 240;

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for DevPanel {
    fn default() -> Self {
        Self {
            height: Self::DEFAULT_HEIGHT,
            hidden: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Appearance {
    pub mode: ThemeMode,
    pub dark: String,
    pub light: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            mode: ThemeMode::System,
            dark: "graphite".into(),
            light: "day".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// How the game starts; every group can be overridden per instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LaunchSettings {
    pub java: JavaChoice,
    pub memory: MemoryMb,
    #[serde(default)]
    pub jvm_args: Vec<String>,
    pub window: GameWindow,
    #[serde(default)]
    pub commands: LaunchCommands,
}

impl Default for LaunchSettings {
    fn default() -> Self {
        Self {
            java: JavaChoice::Auto,
            memory: MemoryMb {
                min: 1024,
                max: 4096,
            },
            jvm_args: vec![],
            window: GameWindow {
                width: 1280,
                height: 720,
                fullscreen: false,
            },
            commands: LaunchCommands::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JavaChoice {
    /// The major version Minecraft needs, downloaded when missing.
    Auto,
    /// A specific `java` executable.
    Path { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryMb {
    pub min: u32,
    pub max: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GameWindow {
    pub width: u32,
    pub height: u32,
    /// Starts the game fullscreen; `maximized` is the name older settings used.
    #[serde(
        default,
        alias = "maximized",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub fullscreen: bool,
}

/// Commands run around the game, as in Prism: before it, wrapping it and after it exits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LaunchCommands {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_launch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_exit: Option<String>,
}

/// `<data>/riven/instances/<id>/instance.json` — one game installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Instance {
    pub name: String,
    pub minecraft: String,
    /// None runs vanilla.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader: Option<Loader>,
    /// Launch groups this instance sets itself instead of inheriting the launcher's.
    #[serde(default)]
    pub overrides: LaunchOverrides,
    /// Lets the player add their own mods to an instance installed from a pack.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub own_mods: bool,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub play_seconds: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LaunchOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java: Option<JavaChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryMb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jvm_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<GameWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<LaunchCommands>,
}

impl LaunchOverrides {
    /// `defaults` with this instance's overrides applied.
    pub fn resolve(&self, defaults: &LaunchSettings) -> LaunchSettings {
        LaunchSettings {
            java: self.java.clone().unwrap_or_else(|| defaults.java.clone()),
            memory: self.memory.unwrap_or(defaults.memory),
            jvm_args: self
                .jvm_args
                .clone()
                .unwrap_or_else(|| defaults.jvm_args.clone()),
            window: self.window.unwrap_or(defaults.window),
            commands: self
                .commands
                .clone()
                .unwrap_or_else(|| defaults.commands.clone()),
        }
    }
}

/// `<config>/riven/accounts.json` — accounts the launcher can play with; secrets stay in the keychain.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Accounts {
    #[serde(default)]
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Account {
    /// Player UUID, hyphenated.
    pub id: String,
    pub name: String,
    pub kind: AccountKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Offline,
    Microsoft,
}
