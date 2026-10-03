use std::collections::HashMap;

use crate::{Modrinth, ProjectInfo, Result, Source, Version};

/// What is known about a file before asking Modrinth which project it belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub sha512: Option<String>,
    pub sha1: Option<String>,
}

/// A file identified as a version of a Modrinth project.
#[derive(Debug, Clone)]
pub struct Matched {
    pub source: riven_format::Source,
    pub version: Version,
    pub info: ProjectInfo,
}

/// Finds each file on Modrinth by its sha512, falling back to sha1.
pub async fn identify(modrinth: &Modrinth, files: &[Known]) -> Result<Vec<Option<Matched>>> {
    let sha512s: Vec<String> = files.iter().filter_map(|f| f.sha512.clone()).collect();
    let by_sha512 = modrinth.versions_by_hash("sha512", &sha512s).await?;
    let sha1s: Vec<String> = files
        .iter()
        .filter(|f| f.sha512.as_ref().is_none_or(|h| !by_sha512.contains_key(h)))
        .filter_map(|f| f.sha1.clone())
        .collect();
    let by_sha1 = modrinth.versions_by_hash("sha1", &sha1s).await?;
    let found: Vec<Option<&Version>> = files
        .iter()
        .map(|f| {
            f.sha512
                .as_ref()
                .and_then(|h| by_sha512.get(h))
                .or_else(|| f.sha1.as_ref().and_then(|h| by_sha1.get(h)))
        })
        .collect();

    let mut ids: Vec<String> = found.iter().flatten().map(|v| v.project.clone()).collect();
    ids.sort();
    ids.dedup();
    let projects: HashMap<String, ProjectInfo> = if ids.is_empty() {
        HashMap::new()
    } else {
        modrinth
            .projects(&ids)
            .await?
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect()
    };
    Ok(found
        .into_iter()
        .map(|v| {
            let v = v?;
            Some(Matched {
                source: riven_format::Source::Modrinth {
                    project: v.project.clone(),
                    version: v.id.clone(),
                },
                version: v.clone(),
                info: projects.get(&v.project)?.clone(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::*;
    use crate::Cache;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[tokio::test]
    async fn files_are_found_by_sha1_when_sha512_is_unknown() {
        let dir = std::env::temp_dir().join(format!("riven-test-identify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::new(&dir, Duration::from_secs(3600));
        let modrinth = Modrinth::new(crate::client())
            .with_cache(cache.clone())
            .with_base("http://127.0.0.1:9/v2");

        let sha512 = "f".repeat(128);
        let sha1 = "4c1f4d0a5b5f6e0f1ff3e1e3c6a3a0b2d48d0c11";
        let sodium: serde_json::Value =
            serde_json::from_str(&fixture("modrinth/versions-ids-sodium.json")).unwrap();
        let body = serde_json::json!({ "algorithm": "sha512", "hashes": [sha512] });
        cache.put(
            &format!("POST {}\n{body}", modrinth.api_url("version_files", &[])),
            "{}",
        );
        let body = serde_json::json!({ "algorithm": "sha1", "hashes": [sha1] });
        cache.put(
            &format!("POST {}\n{body}", modrinth.api_url("version_files", &[])),
            &serde_json::json!({ sha1: sodium[0] }).to_string(),
        );
        cache.put(
            &modrinth.api_url("projects", &[("ids", r#"["AANobbMI"]"#.into())]),
            &format!("[{}]", fixture("modrinth/project-sodium.json")),
        );

        let files = [
            Known {
                sha512: Some(sha512),
                sha1: Some(sha1.into()),
            },
            Known::default(),
        ];
        let found = identify(&modrinth, &files).await.unwrap();
        let matched = found[0].as_ref().unwrap();
        assert!(
            matches!(&matched.source, riven_format::Source::Modrinth { project, .. } if project == "AANobbMI")
        );
        assert_eq!(matched.info.slug, "sodium");
        assert!(found[1].is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
