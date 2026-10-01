use std::collections::HashSet;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Group, Hashes, Java, Keyed, Loader, Side, sorted, sorted_strings};
use crate::path::PackPath;

/// `riven.json` — the pack project in the author's repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    pub name: String,
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<PackPath>,
    pub minecraft: String,
    pub loader: Loader,
    pub java: Java,
    #[serde(default, serialize_with = "sorted")]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub files: FileRules,
    #[serde(default, serialize_with = "sorted")]
    pub content: Vec<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Modrinth,
    GitHub,
    Url,
    Local,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileRules {
    /// Globs the installer never overwrites once the user changed them.
    #[serde(default)]
    pub preserve: Vec<String>,
    /// Globs under `overrides/` left out of releases.
    #[serde(default)]
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub source: Source,
    pub file: EntryFile,
    pub side: Side,
    #[serde(default)]
    pub group: Option<String>,
    pub update: UpdatePolicy,
    pub reason: Reason,
    #[serde(default, serialize_with = "sorted_strings")]
    pub requires: Vec<String>,
}

impl Keyed for Entry {
    fn key(&self) -> &str {
        &self.id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Mod,
    ResourcePack,
    ShaderPack,
    DataPack,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    Modrinth {
        project: String,
        version: String,
    },
    GitHub {
        repo: String,
        tag: String,
        asset: String,
    },
    Url {
        url: String,
    },
    Local {
        path: PackPath,
    },
}

impl Source {
    pub fn kind(&self) -> SourceKind {
        match self {
            Source::Modrinth { .. } => SourceKind::Modrinth,
            Source::GitHub { .. } => SourceKind::GitHub,
            Source::Url { .. } => SourceKind::Url,
            Source::Local { .. } => SourceKind::Local,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EntryFile {
    pub path: PackPath,
    pub size: u64,
    pub hashes: Hashes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum UpdatePolicy {
    Follow,
    Pinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    Explicit,
    Dependency,
}

/// A structural problem in `riven.json` that serde alone does not catch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    DuplicateEntry(String),
    DuplicateGroup(String),
    DuplicatePath(PackPath),
    UnknownGroup { entry: String, group: String },
    UnknownRequire { entry: String, require: String },
    MissingHash(String),
    MalformedHash { entry: String, algo: &'static str },
    MissingUrl(String),
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issue::DuplicateEntry(id) => write!(f, "duplicate content id `{id}`"),
            Issue::DuplicateGroup(id) => write!(f, "duplicate group id `{id}`"),
            Issue::DuplicatePath(path) => write!(f, "several entries install to `{path}`"),
            Issue::UnknownGroup { entry, group } => {
                write!(f, "`{entry}` belongs to undefined group `{group}`")
            }
            Issue::UnknownRequire { entry, require } => {
                write!(f, "`{entry}` requires unknown entry `{require}`")
            }
            Issue::MissingHash(id) => write!(f, "`{id}` has no file hash"),
            Issue::MalformedHash { entry, algo } => write!(f, "`{entry}` has a malformed {algo}"),
            Issue::MissingUrl(id) => write!(f, "`{id}` has no download url"),
        }
    }
}

impl Project {
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.content.iter().find(|e| e.id == id)
    }

    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = Vec::new();

        let mut groups = HashSet::new();
        for group in &self.groups {
            if !groups.insert(group.id.as_str()) {
                issues.push(Issue::DuplicateGroup(group.id.clone()));
            }
        }

        let mut ids = HashSet::new();
        let mut paths = HashSet::new();
        for entry in &self.content {
            if !ids.insert(entry.id.as_str()) {
                issues.push(Issue::DuplicateEntry(entry.id.clone()));
            }
            if !paths.insert(&entry.file.path) {
                issues.push(Issue::DuplicatePath(entry.file.path.clone()));
            }
        }

        for entry in &self.content {
            if let Some(group) = &entry.group
                && !groups.contains(group.as_str())
            {
                issues.push(Issue::UnknownGroup {
                    entry: entry.id.clone(),
                    group: group.clone(),
                });
            }
            for require in &entry.requires {
                if !ids.contains(require.as_str()) {
                    issues.push(Issue::UnknownRequire {
                        entry: entry.id.clone(),
                        require: require.clone(),
                    });
                }
            }
            if entry.file.hashes.is_empty() {
                issues.push(Issue::MissingHash(entry.id.clone()));
            } else if let Some(algo) = entry.file.hashes.malformed() {
                issues.push(Issue::MalformedHash {
                    entry: entry.id.clone(),
                    algo,
                });
            }
            let needs_url = !matches!(entry.source, Source::Local { .. });
            if needs_url && entry.file.url.is_none() {
                issues.push(Issue::MissingUrl(entry.id.clone()));
            }
        }

        issues
    }
}
