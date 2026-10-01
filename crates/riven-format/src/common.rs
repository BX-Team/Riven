use schemars::JsonSchema;
use serde::{Deserialize, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Client,
    Server,
    Both,
}

/// The side an instance is installed as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum InstallSide {
    Client,
    Server,
}

impl Side {
    pub fn includes(self, side: InstallSide) -> bool {
        matches!(
            (self, side),
            (Side::Both, _)
                | (Side::Client, InstallSide::Client)
                | (Side::Server, InstallSide::Server)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Loader {
    #[serde(rename = "type")]
    pub kind: LoaderKind,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Java {
    pub major: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<Memory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jvm_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Memory {
    pub min: String,
    pub recommended: String,
}

/// Hashes a source provided; the strongest present one is verified.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hashes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha512: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hash<'a> {
    Sha512(&'a str),
    Sha1(&'a str),
}

impl Hashes {
    pub fn strongest(&self) -> Option<Hash<'_>> {
        if let Some(h) = &self.sha512 {
            Some(Hash::Sha512(h))
        } else {
            self.sha1.as_deref().map(Hash::Sha1)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.strongest().is_none()
    }

    /// The first malformed hex digest, if any.
    pub fn malformed(&self) -> Option<&'static str> {
        let bad = |h: &Option<String>, len| {
            h.as_ref()
                .is_some_and(|h| h.len() != len || !h.bytes().all(|b| b.is_ascii_hexdigit()))
        };
        if bad(&self.sha512, 128) {
            Some("sha512")
        } else if bad(&self.sha1, 40) {
            Some("sha1")
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub default: bool,
}

/// Array elements written in key order by the deterministic writer.
pub trait Keyed {
    fn key(&self) -> &str;
}

impl Keyed for Group {
    fn key(&self) -> &str {
        &self.id
    }
}

pub(crate) fn sorted<T: Keyed + Serialize, S: Serializer>(
    items: &[T],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut refs: Vec<&T> = items.iter().collect();
    refs.sort_by(|a, b| a.key().cmp(b.key()));
    serializer.collect_seq(refs)
}

pub(crate) fn sorted_strings<S: Serializer>(
    items: &[String],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut refs: Vec<&String> = items.iter().collect();
    refs.sort();
    serializer.collect_seq(refs)
}
