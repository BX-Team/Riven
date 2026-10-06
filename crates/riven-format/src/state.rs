use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{InstallSide, sorted};
use crate::path::PackPath;
use crate::project::Entry;

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StateFile {
    pub sha512: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preserve: bool,
}

/// `.riven/own.json` — content the player added to an instance; pack updates leave it alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OwnContent {
    #[serde(default, serialize_with = "sorted")]
    pub content: Vec<Entry>,
}

/// `<config>/riven/trusted.json` — public keys pinned per pack URL on first install (TOFU).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Trusted {
    /// Channel pointer URL → `ed25519:` public key.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}
