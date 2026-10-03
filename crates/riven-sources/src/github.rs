use std::fmt;

use reqwest::StatusCode;
use serde::Deserialize;

use crate::http::get_json_with;
use crate::{Cache, Error, Result};

pub const API: &str = "https://api.github.com";
const PER_PAGE: u32 = 100;

/// GitHub Releases client; uses `GITHUB_TOKEN`/`GH_TOKEN` when set for the higher rate limit.
#[derive(Clone)]
pub struct GitHub {
    http: reqwest::Client,
    base: String,
    cache: Option<Cache>,
    token: Option<String>,
}

impl fmt::Debug for GitHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHub")
            .field("base", &self.base)
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub prerelease: bool,
    /// RFC 3339 timestamp.
    pub published: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub url: String,
}

impl GitHub {
    pub fn new(http: reqwest::Client) -> Self {
        let token = ["GITHUB_TOKEN", "GH_TOKEN"]
            .into_iter()
            .find_map(|var| std::env::var(var).ok().filter(|t| !t.trim().is_empty()));
        Self {
            http,
            base: API.into(),
            cache: None,
            token,
        }
    }

    pub fn with_cache(mut self, cache: Cache) -> Self {
        self.cache = Some(cache);
        self
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}/{path}", self.base);
        let auth = self.token.as_ref().map(|t| format!("Bearer {t}"));
        let mut headers = vec![
            ("accept", "application/vnd.github+json"),
            ("x-github-api-version", "2022-11-28"),
        ];
        if let Some(auth) = &auth {
            headers.push(("authorization", auth));
        }
        match get_json_with(&self.http, self.cache.as_ref(), &url, &headers).await {
            Err(Error::Status {
                status: StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS,
                ..
            }) => Err(Error::Unsupported(
                "GitHub API rate limit reached; set GITHUB_TOKEN to raise it".into(),
            )),
            other => other,
        }
    }

    /// Published releases of `repo` (`owner/name`), newest first.
    pub async fn releases(&self, repo: &str) -> Result<Vec<Release>> {
        let found: Vec<ApiRelease> = self
            .get(&format!("repos/{repo}/releases?per_page={PER_PAGE}"))
            .await?;
        let mut releases: Vec<Release> = found
            .into_iter()
            .filter(|r| !r.draft)
            .map(Release::from)
            .collect();
        releases.sort_by(|a, b| b.published.cmp(&a.published));
        Ok(releases)
    }

    pub async fn release(&self, repo: &str, tag: &str) -> Result<Release> {
        let found: ApiRelease = self
            .get(&format!("repos/{repo}/releases/tags/{tag}"))
            .await?;
        Ok(found.into())
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

impl From<ApiRelease> for Release {
    fn from(r: ApiRelease) -> Self {
        Self {
            name: r
                .name
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| r.tag_name.clone()),
            tag: r.tag_name,
            prerelease: r.prerelease,
            published: r.published_at.unwrap_or_default(),
            assets: r
                .assets
                .into_iter()
                .map(|a| Asset {
                    name: a.name,
                    size: a.size,
                    url: a.browser_download_url,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn releases_skip_drafts_and_sort_by_date() {
        let dir = std::env::temp_dir().join(format!("riven-test-github-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::new(&dir, Duration::from_secs(3600));
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/github/releases-modmenu.json");
        let mut github = GitHub::new(crate::client()).with_cache(cache.clone());
        github.base = "http://127.0.0.1:9".into();
        cache.put(
            "http://127.0.0.1:9/repos/TerraformersMC/ModMenu/releases?per_page=100",
            &std::fs::read_to_string(fixture).unwrap(),
        );

        let releases = github.releases("TerraformersMC/ModMenu").await.unwrap();
        assert!(
            releases
                .windows(2)
                .all(|w| w[0].published >= w[1].published)
        );
        let tags: Vec<&str> = releases.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(tags.last(), Some(&"v21.0.0"));
        assert!(releases[0].prerelease);
        assert!(
            releases[1].assets[0]
                .url
                .ends_with("/v20.0.3/modmenu-20.0.3.jar")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
