use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::InstallSide;
use crate::path::PackPath;

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
