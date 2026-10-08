use futures_util::future::try_join_all;
use riven_format::{Hashes, Kind, LoaderKind, SourceKind};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use url::Url;

use std::collections::HashMap;

use crate::http::{get_json, post_json};
use crate::{
    Cache, Channel, Dependency, DependencyKind, Hit, ProjectInfo, Result, Source, Support, Target,
    Version, VersionFile,
};

pub const API: &str = "https://api.modrinth.com/v2";
/// Ids per batch request, keeping URLs well under server limits.
const BATCH: usize = 100;

/// Modrinth API v2 client.
#[derive(Debug, Clone)]
pub struct Modrinth {
    http: reqwest::Client,
    base: String,
    cache: Option<Cache>,
}

impl Modrinth {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            base: API.into(),
            cache: None,
        }
    }

    pub fn with_cache(mut self, cache: Cache) -> Self {
        self.cache = Some(cache);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_base(mut self, base: &str) -> Self {
        self.base = base.into();
        self
    }

    #[cfg(test)]
    pub(crate) fn api_url(&self, path: &str, query: &[(&str, String)]) -> String {
        self.url(path, query)
    }

    fn url(&self, path: &str, query: &[(&str, String)]) -> String {
        let mut url = Url::parse(&format!("{}/{path}", self.base)).expect("valid Modrinth url");
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        url.into()
    }

    async fn get<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        get_json(&self.http, self.cache.as_ref(), url).await
    }

    /// GETs `path?ids=[…]` in parallel batches and concatenates the replies.
    async fn get_batched<T: DeserializeOwned>(&self, path: &str, ids: &[String]) -> Result<Vec<T>> {
        let pages = try_join_all(ids.chunks(BATCH).map(|chunk| {
            let url = self.url(path, &[("ids", json(chunk))]);
            async move { self.get::<Vec<T>>(&url).await }
        }))
        .await?;
        Ok(pages.into_iter().flatten().collect())
    }

    /// POSTs `body` once per batch of `hashes`, in parallel, merging the hash-keyed replies.
    async fn post_hashes<H: serde::Serialize + Sync>(
        &self,
        path: &str,
        hashes: &[H],
        body: serde_json::Value,
    ) -> Result<HashMap<String, ApiVersion>> {
        let url = self.url(path, &[]);
        let pages = try_join_all(hashes.chunks(BATCH).map(|chunk| {
            let mut body = body.clone();
            body["hashes"] = serde_json::json!(chunk);
            let url = &url;
            async move {
                post_json::<HashMap<String, ApiVersion>>(
                    &self.http,
                    self.cache.as_ref(),
                    url,
                    &body,
                )
                .await
            }
        }))
        .await?;
        Ok(pages.into_iter().flatten().collect())
    }

    /// Modpacks matching `query`, for `minecraft` when given, most relevant first.
    pub async fn search_modpacks(
        &self,
        query: &str,
        minecraft: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Hit>> {
        let mut facets = vec![vec!["project_type:modpack".to_owned()]];
        if let Some(mc) = minecraft {
            facets.push(vec![format!("versions:{mc}")]);
        }
        let url = self.url(
            "search",
            &[
                ("query", query.to_owned()),
                ("limit", limit.to_string()),
                ("facets", json(&facets)),
            ],
        );
        let response: ApiSearch = self.get(&url).await?;
        Ok(response.hits.into_iter().map(Hit::from).collect())
    }

    /// Every version of a project, newest first.
    pub async fn project_versions(&self, project: &str) -> Result<Vec<Version>> {
        let url = self.url(&format!("project/{project}/version"), &[]);
        let versions: Vec<ApiVersion> = self.get(&url).await?;
        Ok(versions.into_iter().map(Into::into).collect())
    }

    /// A project's page: its categories, license, stats and links.
    pub async fn page(&self, id: &str) -> Result<ProjectPage> {
        let page: ApiPage = self.get(&self.url(&format!("project/{id}"), &[])).await?;
        Ok(page.into())
    }

    /// Versions owning files with the given `sha1` or `sha512` hashes, keyed by hash.
    pub async fn versions_by_hash(
        &self,
        algorithm: &str,
        hashes: &[String],
    ) -> Result<HashMap<String, Version>> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }
        let body = serde_json::json!({ "algorithm": algorithm });
        let found = self.post_hashes("version_files", hashes, body).await?;
        Ok(found
            .into_iter()
            .map(|(hash, v)| (hash, v.into()))
            .collect())
    }

    pub(crate) fn search_url(&self, query: &str, target: &Target, limit: u32) -> String {
        let mut facets = vec![vec![format!("versions:{}", target.minecraft)]];
        if let Some(kind) = project_type(target.kind) {
            facets.push(vec![format!("project_type:{kind}")]);
        }
        if let Some(loaders) = loaders(target) {
            facets.push(loaders.iter().map(|l| format!("categories:{l}")).collect());
        }
        self.url(
            "search",
            &[
                ("query", query.to_owned()),
                ("limit", limit.to_string()),
                ("facets", json(&facets)),
            ],
        )
    }

    pub(crate) fn versions_url(&self, project: &str, target: &Target) -> String {
        let mut query = vec![
            ("game_versions", json(&[&target.minecraft])),
            ("include_changelog", "false".to_owned()),
        ];
        if let Some(loaders) = loaders(target) {
            query.push(("loaders", json(&loaders)));
        }
        self.url(&format!("project/{project}/version"), &query)
    }
}

fn json<T: serde::Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).expect("plain values serialize")
}

fn project_type(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Mod => Some("mod"),
        Kind::ResourcePack => Some("resourcepack"),
        Kind::ShaderPack => Some("shader"),
        Kind::DataPack => Some("datapack"),
        Kind::File => None,
    }
}

/// Modrinth loader names for `target`; `None` means "don't filter by loader".
fn loaders(target: &Target) -> Option<Vec<&'static str>> {
    match target.kind {
        Kind::Mod => Some(match target.loader {
            LoaderKind::Fabric => vec!["fabric"],
            // Quilt loads Fabric mods.
            LoaderKind::Quilt => vec!["quilt", "fabric"],
            LoaderKind::Forge => vec!["forge"],
            LoaderKind::NeoForge => vec!["neoforge"],
        }),
        Kind::DataPack => Some(vec!["datapack"]),
        Kind::ResourcePack | Kind::ShaderPack | Kind::File => None,
    }
}

impl Source for Modrinth {
    fn kind(&self) -> SourceKind {
        SourceKind::Modrinth
    }

    async fn search(&self, query: &str, target: &Target, limit: u32) -> Result<Vec<Hit>> {
        let response: ApiSearch = self.get(&self.search_url(query, target, limit)).await?;
        Ok(response.hits.into_iter().map(Hit::from).collect())
    }

    async fn project(&self, id: &str) -> Result<ProjectInfo> {
        let project: ApiProject = self.get(&self.url(&format!("project/{id}"), &[])).await?;
        Ok(project.into())
    }

    async fn resolve(&self, project: &str, target: &Target) -> Result<Vec<Version>> {
        let versions: Vec<ApiVersion> = self.get(&self.versions_url(project, target)).await?;
        let mut versions: Vec<Version> = versions.into_iter().map(Version::from).collect();
        versions.sort_by(|a, b| b.published.cmp(&a.published));
        Ok(versions)
    }

    async fn version(&self, id: &str) -> Result<Version> {
        let version: ApiVersion = self.get(&self.url(&format!("version/{id}"), &[])).await?;
        Ok(version.into())
    }

    async fn projects(&self, ids: &[String]) -> Result<Vec<ProjectInfo>> {
        let found: Vec<ApiProject> = self.get_batched("projects", ids).await?;
        Ok(found.into_iter().map(ProjectInfo::from).collect())
    }

    async fn versions(&self, ids: &[String]) -> Result<Vec<Version>> {
        let found: Vec<ApiVersion> = self.get_batched("versions", ids).await?;
        Ok(found.into_iter().map(Version::from).collect())
    }

    async fn latest(
        &self,
        current: &[Version],
        target: &Target,
    ) -> Result<HashMap<String, Version>> {
        let by_hash: HashMap<&str, &str> = current
            .iter()
            .filter_map(|v| Some((v.primary_file()?.hashes.sha512.as_deref()?, v.id.as_str())))
            .collect();
        let mut hashes: Vec<&str> = by_hash.keys().copied().collect();
        hashes.sort_unstable();
        let mut body = serde_json::json!({
            "algorithm": "sha512",
            "game_versions": [target.minecraft],
        });
        if let Some(loaders) = loaders(target) {
            body["loaders"] = serde_json::json!(loaders);
        }
        let found = self
            .post_hashes("version_files/update", &hashes, body)
            .await?;
        Ok(found
            .into_iter()
            .filter_map(|(hash, v)| Some((by_hash.get(hash.as_str())?.to_string(), v.into())))
            .collect())
    }
}

#[derive(Deserialize)]
struct ApiSearch {
    hits: Vec<ApiHit>,
}

#[derive(Deserialize)]
struct ApiHit {
    project_id: String,
    slug: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    downloads: u64,
    icon_url: Option<String>,
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
}

const LOADER_CATEGORIES: [&str; 4] = ["fabric", "quilt", "forge", "neoforge"];

impl From<ApiHit> for Hit {
    fn from(hit: ApiHit) -> Self {
        Self {
            game_versions: hit.versions,
            loaders: hit
                .categories
                .into_iter()
                .filter(|c| LOADER_CATEGORIES.contains(&c.as_str()))
                .collect(),
            id: hit.project_id,
            slug: hit.slug,
            title: hit.title,
            description: hit.description,
            author: hit.author,
            downloads: hit.downloads,
            icon_url: hit.icon_url.filter(|u| !u.is_empty()),
        }
    }
}

/// What a project page shows about it beyond a search hit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectPage {
    pub slug: String,
    pub categories: Vec<String>,
    pub license: Option<String>,
    pub downloads: u64,
    pub followers: u64,
    /// RFC 3339.
    pub updated: String,
    pub client: Support,
    pub server: Support,
    /// `(label, url)` of the source, issue tracker, wiki and Discord, where given.
    pub links: Vec<(&'static str, String)>,
    /// The long description, in Markdown.
    pub body: String,
}

#[derive(Deserialize)]
struct ApiLicense {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct ApiPage {
    slug: String,
    #[serde(default)]
    categories: Vec<String>,
    license: Option<ApiLicense>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    followers: u64,
    #[serde(default)]
    updated: String,
    client_side: String,
    server_side: String,
    source_url: Option<String>,
    issues_url: Option<String>,
    wiki_url: Option<String>,
    discord_url: Option<String>,
    #[serde(default)]
    body: String,
}

impl From<ApiPage> for ProjectPage {
    fn from(page: ApiPage) -> Self {
        let links = [
            ("source", page.source_url),
            ("issues", page.issues_url),
            ("wiki", page.wiki_url),
            ("discord", page.discord_url),
        ]
        .into_iter()
        .filter_map(|(label, url)| Some((label, url.filter(|u| !u.is_empty())?)))
        .collect();
        let license = page.license.and_then(|l| {
            let text = if l.name.is_empty() { l.id } else { l.name };
            (!text.is_empty()).then_some(text)
        });
        Self {
            slug: page.slug,
            categories: page.categories,
            license,
            downloads: page.downloads,
            followers: page.followers,
            updated: page.updated,
            client: support(&page.client_side),
            server: support(&page.server_side),
            links,
            body: page.body,
        }
    }
}

#[derive(Deserialize)]
struct ApiProject {
    id: String,
    slug: String,
    title: String,
    #[serde(default)]
    description: String,
    project_type: String,
    client_side: String,
    server_side: String,
    team: String,
    organization: Option<String>,
    #[serde(default)]
    icon_url: Option<String>,
}

fn support(value: &str) -> Support {
    match value {
        "required" => Support::Required,
        "optional" => Support::Optional,
        "unsupported" => Support::Unsupported,
        _ => Support::Unknown,
    }
}

impl From<ApiProject> for ProjectInfo {
    fn from(project: ApiProject) -> Self {
        Self {
            kind: match project.project_type.as_str() {
                "mod" => Some(Kind::Mod),
                "resourcepack" => Some(Kind::ResourcePack),
                "shader" => Some(Kind::ShaderPack),
                "datapack" => Some(Kind::DataPack),
                _ => None,
            },
            client: support(&project.client_side),
            server: support(&project.server_side),
            owner: project.organization.unwrap_or(project.team),
            icon_url: project.icon_url.filter(|u| !u.is_empty()),
            id: project.id,
            slug: project.slug,
            title: project.title,
            description: project.description,
        }
    }
}

#[derive(Deserialize)]
struct ApiVersion {
    id: String,
    project_id: String,
    name: String,
    version_number: String,
    version_type: String,
    game_versions: Vec<String>,
    loaders: Vec<String>,
    date_published: String,
    files: Vec<ApiFile>,
    #[serde(default)]
    dependencies: Vec<ApiDependency>,
}

#[derive(Deserialize)]
struct ApiFile {
    hashes: ApiHashes,
    url: String,
    filename: String,
    primary: bool,
    size: u64,
}

#[derive(Deserialize)]
struct ApiHashes {
    sha512: Option<String>,
    sha1: Option<String>,
}

#[derive(Deserialize)]
struct ApiDependency {
    version_id: Option<String>,
    project_id: Option<String>,
    dependency_type: String,
}

impl From<ApiVersion> for Version {
    fn from(version: ApiVersion) -> Self {
        Self {
            id: version.id,
            project: version.project_id,
            name: version.name,
            number: version.version_number,
            channel: match version.version_type.as_str() {
                "beta" => Channel::Beta,
                "alpha" => Channel::Alpha,
                _ => Channel::Release,
            },
            game_versions: version.game_versions,
            loaders: version.loaders,
            published: version.date_published,
            files: version
                .files
                .into_iter()
                .map(|f| VersionFile {
                    filename: f.filename,
                    url: Some(f.url),
                    size: f.size,
                    hashes: Hashes {
                        sha512: f.hashes.sha512,
                        sha1: f.hashes.sha1,
                    },
                    primary: f.primary,
                })
                .collect(),
            dependencies: version
                .dependencies
                .into_iter()
                .filter_map(|d| {
                    let kind = match d.dependency_type.as_str() {
                        "required" => DependencyKind::Required,
                        "optional" => DependencyKind::Optional,
                        "incompatible" => DependencyKind::Incompatible,
                        "embedded" => DependencyKind::Embedded,
                        _ => return None,
                    };
                    Some(Dependency {
                        project: d.project_id,
                        version: d.version_id,
                        kind,
                    })
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use riven_format::Side;

    use super::*;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/modrinth")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// A client whose cache already holds the recorded response for each URL.
    fn offline(dir: &Path, responses: &[(String, &str)]) -> Modrinth {
        let cache = Cache::new(dir, Duration::from_secs(3600));
        for (url, name) in responses {
            cache.put(url, &fixture(name));
        }
        // An unroutable base: anything not pre-cached fails instead of hitting the network.
        let mut modrinth = Modrinth::new(crate::client()).with_cache(cache);
        modrinth.base = "http://127.0.0.1:9/v2".into();
        modrinth
    }

    fn target(loader: LoaderKind) -> Target {
        Target {
            minecraft: "1.21.1".into(),
            loader,
            kind: Kind::Mod,
        }
    }

    fn tempdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("riven-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn query_urls_encode_json_filters() {
        let modrinth = Modrinth::new(crate::client());
        let url =
            Url::parse(&modrinth.versions_url("AANobbMI", &target(LoaderKind::Quilt))).unwrap();
        let query: Vec<_> = url.query_pairs().collect();
        assert!(query.contains(&("loaders".into(), r#"["quilt","fabric"]"#.into())));
        assert!(query.contains(&("game_versions".into(), r#"["1.21.1"]"#.into())));

        let shaders = Target {
            kind: Kind::ShaderPack,
            ..target(LoaderKind::NeoForge)
        };
        let url = Url::parse(&modrinth.search_url("bsl", &shaders, 5)).unwrap();
        let facets = url.query_pairs().find(|(k, _)| k == "facets").unwrap().1;
        assert_eq!(facets, r#"[["versions:1.21.1"],["project_type:shader"]]"#);
    }

    #[tokio::test]
    async fn resolves_versions_and_dependencies() {
        let dir = tempdir("modrinth-resolve");
        let probe = Modrinth {
            base: "http://127.0.0.1:9/v2".into(),
            ..Modrinth::new(crate::client())
        };
        let fabric = target(LoaderKind::Fabric);
        let modrinth = offline(
            &dir,
            &[
                (
                    probe.versions_url("sodium-extra", &fabric),
                    "versions-sodium-extra-fabric-1.21.1.json",
                ),
                (
                    probe.versions_url("iris", &fabric),
                    "versions-iris-fabric-1.21.1.json",
                ),
                (
                    probe.versions_url("sodium", &target(LoaderKind::NeoForge)),
                    "versions-sodium-neoforge-1.21.1.json",
                ),
            ],
        );

        let extra = modrinth.resolve("sodium-extra", &fabric).await.unwrap();
        assert!(extra.windows(2).all(|w| w[0].published >= w[1].published));
        let required: Vec<_> = extra[0]
            .dependencies
            .iter()
            .filter(|d| d.kind == DependencyKind::Required)
            .collect();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0].project.as_deref(), Some("AANobbMI"));

        let iris = modrinth.resolve("iris", &fabric).await.unwrap();
        assert_eq!(iris[0].dependencies[0].version.as_deref(), Some("s7adptIg"));

        let sodium = modrinth
            .resolve("sodium", &target(LoaderKind::NeoForge))
            .await
            .unwrap();
        let file = sodium[0].primary_file().unwrap();
        assert!(file.filename.ends_with(".jar"));
        assert_eq!(file.hashes.sha512.as_ref().map(String::len), Some(128));
        assert!(sodium[0].loaders.contains(&"neoforge".to_owned()));

        assert!(modrinth.resolve("uncached", &fabric).await.is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn batch_update_maps_newest_back_to_current_versions() {
        let dir = tempdir("modrinth-latest");
        let probe = Modrinth {
            base: "http://127.0.0.1:9/v2".into(),
            ..Modrinth::new(crate::client())
        };
        let modrinth = offline(
            &dir,
            &[(
                probe.url("versions", &[("ids", json(&["5EEI3Guz"]))]),
                "versions-ids-sodium.json",
            )],
        );
        let current = modrinth.versions(&["5EEI3Guz".into()]).await.unwrap();
        assert_eq!(current.len(), 1);

        let neoforge = target(LoaderKind::NeoForge);
        let hash = current[0]
            .primary_file()
            .unwrap()
            .hashes
            .sha512
            .clone()
            .unwrap();
        let body = serde_json::json!({
            "algorithm": "sha512",
            "game_versions": ["1.21.1"],
            "loaders": ["neoforge"],
            "hashes": [hash],
        });
        let key = format!("POST {}\n{body}", probe.url("version_files/update", &[]));
        modrinth
            .cache
            .as_ref()
            .unwrap()
            .put(&key, &fixture("update-sodium-neoforge-1.21.1.json"));

        let newest = modrinth.latest(&current, &neoforge).await.unwrap();
        let next = &newest["5EEI3Guz"];
        assert_eq!(next.id, "uMOpc5uV");
        assert_eq!(next.project, current[0].project);
        assert!(next.published > current[0].published);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn project_side_and_owner() {
        let dir = tempdir("modrinth-project");
        let probe = Modrinth {
            base: "http://127.0.0.1:9/v2".into(),
            ..Modrinth::new(crate::client())
        };
        let modrinth = offline(
            &dir,
            &[(probe.url("project/sodium", &[]), "project-sodium.json")],
        );
        let project = modrinth.project("sodium").await.unwrap();
        assert_eq!(project.id, "AANobbMI");
        assert_eq!(project.kind, Some(Kind::Mod));
        assert_eq!(project.side(), Side::Client);
        assert_eq!(project.owner, "LjcZDkRW");
        let _ = std::fs::remove_dir_all(dir);
    }
}
