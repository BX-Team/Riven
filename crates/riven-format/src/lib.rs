mod common;
mod path;
mod project;
mod release;
mod sign;
mod state;

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

pub use common::{Group, Hash, Hashes, InstallSide, Java, Keyed, Loader, LoaderKind, Memory, Side};
pub use path::{PackPath, PathError};
pub use project::{
    Entry, EntryFile, FileRules, Issue, Kind, Project, Reason, Source, SourceKind, UpdatePolicy,
};
pub use release::{Channel, Release, ReleaseFile};
pub use sign::{KeyPair, PublicKey, SignError};
pub use state::{State, StateFile, Trusted};

pub const SCHEMA_BASE: &str = "https://raw.githubusercontent.com/BX-Team/Riven/master/schema/v1/";

/// Upgrades a document object by one `format` version in place.
pub type Migration = fn(&mut Map<String, Value>) -> Result<(), String>;

/// A top-level JSON file Riven reads and writes.
pub trait Document: Serialize + DeserializeOwned + JsonSchema {
    /// Schema file stem under `schema/v1/`.
    const NAME: &'static str;
    /// `MIGRATIONS[i]` upgrades `format: i + 1` to `i + 2`.
    const MIGRATIONS: &'static [Migration] = &[];

    fn format() -> u64 {
        Self::MIGRATIONS.len() as u64 + 1
    }

    fn schema_url() -> String {
        format!("{SCHEMA_BASE}{}.json", Self::NAME)
    }
}

impl Document for Project {
    const NAME: &'static str = "project";
}

impl Document for Release {
    const NAME: &'static str = "release";
}

impl Document for Channel {
    const NAME: &'static str = "channel";
}

impl Document for State {
    const NAME: &'static str = "state";
}

impl Document for Trusted {
    const NAME: &'static str = "trusted";
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid {doc}: {source}")]
    Json {
        doc: &'static str,
        source: serde_json::Error,
    },
    #[error("invalid {0}: top level is not an object")]
    NotObject(&'static str),
    #[error("invalid {0}: missing `format`")]
    MissingFormat(&'static str),
    #[error("{doc} has format {found}, this riven supports up to {supported}; update riven")]
    TooNew {
        doc: &'static str,
        found: u64,
        supported: u64,
    },
    #[error("{doc} has unsupported format {found}")]
    Unsupported { doc: &'static str, found: u64 },
    #[error("cannot migrate {doc} from format {from}: {reason}")]
    Migration {
        doc: &'static str,
        from: u64,
        reason: String,
    },
}

/// Serializes deterministically: 2-space indent, fixed field order, sorted keyed arrays, trailing newline.
pub fn to_string<T: Document>(doc: &T) -> String {
    #[derive(Serialize)]
    struct Tagged<'a, T> {
        #[serde(rename = "$schema")]
        schema: String,
        format: u64,
        #[serde(flatten)]
        doc: &'a T,
    }

    let tagged = Tagged {
        schema: T::schema_url(),
        format: T::format(),
        doc,
    };
    let mut out = serde_json::to_string_pretty(&tagged).expect("documents always serialize");
    out.push('\n');
    out
}

/// Parses a document, migrating older `format` versions to the current one.
pub fn from_str<T: Document>(input: &str) -> Result<T, Error> {
    let json = |source| Error::Json {
        doc: T::NAME,
        source,
    };
    let mut value: Value = serde_json::from_str(input).map_err(json)?;
    let object = value.as_object_mut().ok_or(Error::NotObject(T::NAME))?;
    let found = object
        .get("format")
        .and_then(Value::as_u64)
        .ok_or(Error::MissingFormat(T::NAME))?;
    let supported = T::format();
    if found > supported {
        return Err(Error::TooNew {
            doc: T::NAME,
            found,
            supported,
        });
    }
    if found == 0 {
        return Err(Error::Unsupported {
            doc: T::NAME,
            found,
        });
    }
    for (from, migrate) in (found..).zip(&T::MIGRATIONS[found as usize - 1..]) {
        migrate(object).map_err(|reason| Error::Migration {
            doc: T::NAME,
            from,
            reason,
        })?;
    }
    object.remove("$schema");
    object.remove("format");
    serde_json::from_value(value).map_err(json)
}

/// JSON Schema of a document, including the `$schema` and `format` header.
pub fn schema<T: Document>() -> String {
    let mut schema = schemars::generate::SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>();
    let object = schema.ensure_object();
    object.insert("$id".into(), T::schema_url().into());
    let properties = object
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("schema properties are an object");
    properties.insert("$schema".into(), serde_json::json!({ "type": "string" }));
    properties.insert("format".into(), serde_json::json!({ "const": T::format() }));
    let required = object
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("schema required is an array");
    required.insert(0, "format".into());
    let mut out = serde_json::to_string_pretty(&schema).expect("schemas always serialize");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests;
