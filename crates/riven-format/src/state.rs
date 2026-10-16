use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{InstallSide, sorted};
use crate::path::PackPath;
use crate::project::Entry;
use crate::release::ReleaseNote;

/// `.riven/state.json` — what riven installed into an instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct State {
    /// Channel pointer URL.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub side: InstallSide,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default)]
    pub groups: BTreeMap<String, bool>,
    #[serde(default)]
    pub files: BTreeMap<PackPath, StateFile>,
    /// What the last update to another version changed, for the player.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<Changes>,
}

/// The difference between two installed versions of a pack, by content name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Changes {
    /// The version updated from.
    pub from: String,
    /// The author's notes of the versions since `from`, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<ReleaseNote>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updated: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Changed files that are not content: configs and other overrides.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub other: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StateFile {
    pub sha512: String,
    /// The content entry's name from the release; overrides have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preserve: bool,
}

/// `.riven/own.json` — content the player added to an instance; pack updates leave it alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OwnContent {
    #[serde(default, serialize_with = "sorted")]
    pub content: Vec<Entry>,
    /// Pack files the player switched off or removed; pack updates keep them that way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pack: Vec<PackChoice>,
}

/// The player's say over one file of an instance's pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackChoice {
    pub path: PackPath,
    /// The Modrinth project, which finds the file again when an update renames it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Removed rather than switched off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
}

/// `<config>/riven/trusted.json` — public keys pinned per pack URL on first install (TOFU).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Trusted {
    /// Channel pointer URL → `ed25519:` public key.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}
