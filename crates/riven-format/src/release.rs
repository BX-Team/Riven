use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Group, Hashes, Java, Keyed, Loader, Side, sorted};
use crate::path::PackPath;

/// `releases/<version>.json` — a self-contained release manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Release {
    pub id: String,
    pub name: String,
    pub version: String,
    pub minecraft: String,
    pub loader: Loader,
    pub java: Java,
    #[serde(default, serialize_with = "sorted")]
    pub groups: Vec<Group>,
    #[serde(default, serialize_with = "sorted")]
    pub files: Vec<ReleaseFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReleaseFile {
    pub path: PackPath,
    pub size: u64,
    pub hashes: Hashes,
    /// Mirrors in order; relative ones resolve against the manifest URL.
    pub urls: Vec<String>,
    pub side: Side,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preserve: bool,
}

impl Keyed for ReleaseFile {
    fn key(&self) -> &str {
        self.path.as_str()
    }
}

/// `channels/<name>.json` — the tiny pointer polled on every launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Channel {
    pub version: String,
    /// Manifest URL, relative to the pointer.
    pub manifest: String,
    pub sha512: String,
}
