use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

/// A relative POSIX path inside a pack or instance, validated against traversal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PackPath(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path `{0}` is absolute")]
    Absolute(String),
    #[error("path `{0}` contains a backslash")]
    Backslash(String),
    #[error("path `{0}` contains a colon")]
    Colon(String),
    #[error("path `{0}` contains a NUL byte")]
    Nul(String),
    #[error("path `{0}` has an empty, `.` or `..` component")]
    BadComponent(String),
    #[error("path `{0}` uses a reserved Windows device name")]
    Reserved(String),
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

impl PackPath {
    pub fn new(path: impl Into<String>) -> Result<Self, PathError> {
        let path = path.into();
        if path.is_empty() {
            return Err(PathError::Empty);
        }
        if path.starts_with('/') {
            return Err(PathError::Absolute(path));
        }
        if path.contains('\\') {
            return Err(PathError::Backslash(path));
        }
        if path.contains(':') {
            return Err(PathError::Colon(path));
        }
        if path.contains('\0') {
            return Err(PathError::Nul(path));
        }
        for component in path.split('/') {
            if component.is_empty() || component == "." || component == ".." {
                return Err(PathError::BadComponent(path));
            }
            // Windows resolves `nul.txt` and `CON ` to the device too.
            let stem = component.split('.').next().unwrap_or(component).trim_end();
            if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
                return Err(PathError::Reserved(path));
            }
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }
}

impl TryFrom<String> for PackPath {
    type Error = PathError;

    fn try_from(path: String) -> Result<Self, Self::Error> {
        Self::new(path)
    }
}

impl From<PackPath> for String {
    fn from(path: PackPath) -> Self {
        path.0
    }
}

impl fmt::Display for PackPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl JsonSchema for PackPath {
    fn schema_name() -> Cow<'static, str> {
        "PackPath".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "Relative POSIX path: no leading `/`, `\\`, `:`, empty, `.` or `..` components.",
            "minLength": 1
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for ok in [
            "mods/sodium.jar",
            "options.txt",
            "config/xaero/minimap.txt",
            ".hidden/file",
            "console.log",
        ] {
            assert!(PackPath::new(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn rejects_traversal() {
        for bad in [
            "",
            "/etc/passwd",
            "../outside",
            "mods/../../outside",
            "mods/./x.jar",
            "mods//x.jar",
            "mods/",
            "..",
            "C:\\Windows\\x",
            "C:/Windows/x",
            "\\\\?\\C:\\x",
            "mods\\..\\x",
            "file.txt:stream",
            "mods/a\0b",
            "NUL",
            "mods/con.txt",
            "aux ",
            "LPT1.jar",
        ] {
            assert!(PackPath::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn rejected_on_deserialize() {
        assert!(serde_json::from_str::<PackPath>(r#""../x""#).is_err());
    }
}
