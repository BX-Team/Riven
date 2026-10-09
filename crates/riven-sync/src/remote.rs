use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_NONE_MATCH};
use riven_format::{Channel, PublicKey, Release, Trusted};
use sha2::{Digest, Sha512};
use url::Url;

use crate::SyncError;

/// Where a pack lives: its channel pointer and, if the link carried one, the expected key.
#[derive(Debug, Clone)]
pub struct Link {
    pub pointer: Url,
    pub key: Option<PublicKey>,
}

/// Expands `gh:owner/repo` to the repository's GitHub Pages address.
fn github_pages(link: &str) -> Option<String> {
    let rest = link.strip_prefix("gh:")?;
    let (path, fragment) = match rest.split_once('#') {
        Some((path, fragment)) => (path, format!("#{fragment}")),
        None => (rest, String::new()),
    };
    let (owner, repo) = path.trim_end_matches('/').split_once('/')?;
    let valid = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !valid(owner) || !valid(repo) || repo.starts_with('.') {
        return None;
    }
    let host = format!("{}.github.io", owner.to_ascii_lowercase());
    if repo.eq_ignore_ascii_case(&host) {
        Some(format!("https://{host}/{fragment}"))
    } else {
        Some(format!("https://{host}/{repo}/{fragment}"))
    }
}

/// Parses an install link: a pack's base URL, channel pointer or `gh:owner/repo`, optionally with `#key=ed25519:…`.
pub fn parse_link(link: &str, channel: &str) -> Result<Link, SyncError> {
    let expanded = github_pages(link);
    let mut url = Url::parse(expanded.as_deref().unwrap_or(link))
        .map_err(|_| SyncError::BadLink(link.to_owned()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SyncError::BadLink(link.to_owned()));
    }
    let key = match url.fragment() {
        Some(fragment) => match fragment.strip_prefix("key=") {
            Some(key) => Some(PublicKey::parse(key)?),
            None => None,
        },
        None => None,
    };
    url.set_fragment(None);
    let pointer = if url.path().ends_with(".json") {
        url
    } else {
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        url.join(&format!("channels/{channel}.json"))
            .map_err(|_| SyncError::BadLink(link.to_owned()))?
    };
    Ok(Link { pointer, key })
}

/// The channel a pointer URL names: `stable` for `…/channels/stable.json`.
pub fn channel_of(pointer: &str) -> Option<&str> {
    let (base, file) = pointer.rsplit_once('/')?;
    base.ends_with("/channels").then_some(())?;
    file.strip_suffix(".json").filter(|c| !c.is_empty())
}

/// The same pack's pointer for another channel.
pub fn with_channel(pointer: &str, channel: &str) -> Option<String> {
    channel_of(pointer)?;
    let valid = !channel.is_empty()
        && channel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !channel.starts_with('.');
    let (base, _) = pointer.rsplit_once('/')?;
    valid.then(|| format!("{base}/{channel}.json"))
}

/// Whether two pointers are channels of one pack.
pub fn same_pack(a: &str, b: &str) -> bool {
    a == b
        || (channel_of(a).is_some()
            && channel_of(b).is_some()
            && a.rsplit_once('/').map(|p| p.0) == b.rsplit_once('/').map(|p| p.0))
}

/// A verified release and what to remember about it.
#[derive(Debug, Clone)]
pub struct Remote {
    pub pointer: Channel,
    pub release: Release,
    pub manifest_url: Url,
    pub key: Option<PublicKey>,
    pub etag: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Fetched {
    /// The pointer did not change since `etag`.
    NotModified,
    Release(Box<Remote>),
}

/// Which key a pointer must be signed with, following §3.4: link, then pinned, then first use.
pub fn expected_key(
    link: Option<&PublicKey>,
    pinned: Option<&PublicKey>,
    declared: Option<&str>,
) -> Result<Option<PublicKey>, SyncError> {
    let declared = declared.map(PublicKey::parse).transpose()?;
    if let (Some(link), Some(pinned)) = (link, pinned)
        && link != pinned
    {
        return Err(SyncError::KeyChanged);
    }
    let expected = link.or(pinned).cloned();
    if let (Some(expected), Some(declared)) = (&expected, &declared)
        && expected != declared
    {
        return Err(SyncError::KeyChanged);
    }
    Ok(expected.or(declared))
}

/// Checks a signed document; a pack that is expected to be signed must stay signed.
pub fn check_signature(
    key: Option<&PublicKey>,
    bytes: &[u8],
    sig: Option<&str>,
    what: &str,
) -> Result<(), SyncError> {
    match (key, sig) {
        (Some(key), Some(sig)) => key
            .verify(bytes, sig)
            .map_err(|_| SyncError::BadSignature(what.to_owned())),
        (Some(_), None) => Err(SyncError::Unsigned(what.to_owned())),
        (None, _) => Ok(()),
    }
}

async fn get(
    http: &reqwest::Client,
    url: &Url,
    etag: Option<&str>,
) -> Result<Option<(Vec<u8>, Option<String>)>, SyncError> {
    let mut request = http.get(url.clone());
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let http_err = |source| SyncError::Http {
        url: url.to_string(),
        source,
    };
    let response = request.send().await.map_err(http_err)?;
    match response.status() {
        StatusCode::NOT_MODIFIED => return Ok(None),
        StatusCode::NOT_FOUND => return Err(SyncError::NotFound(url.to_string())),
        status if !status.is_success() => {
            return Err(SyncError::Status {
                url: url.to_string(),
                status,
            });
        }
        _ => {}
    }
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response.bytes().await.map_err(http_err)?;
    Ok(Some((bytes.to_vec(), etag)))
}

async fn get_sig(http: &reqwest::Client, url: &Url) -> Result<Option<String>, SyncError> {
    let sig_url = Url::parse(&format!("{url}.sig")).expect("appending to a URL stays valid");
    match get(http, &sig_url, None).await {
        Ok(Some((bytes, _))) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Ok(None) | Err(SyncError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Fetches and verifies the release a link points at, pinning its key on first use.
pub async fn fetch(
    http: &reqwest::Client,
    link: &Link,
    etag: Option<&str>,
    trusted: &mut Trusted,
) -> Result<Fetched, SyncError> {
    let Some((pointer_bytes, etag)) = get(http, &link.pointer, etag).await? else {
        return Ok(Fetched::NotModified);
    };
    let text = String::from_utf8_lossy(&pointer_bytes);
    let pointer: Channel = riven_format::from_str(&text)?;
    let source = link.pointer.to_string();
    let pinned = crate::trust::pinned(trusted, &source);
    let key = expected_key(link.key.as_ref(), pinned.as_ref(), pointer.key.as_deref())?;
    let sig = get_sig(http, &link.pointer).await?;
    check_signature(
        key.as_ref(),
        &pointer_bytes,
        sig.as_deref(),
        "channel pointer",
    )?;

    let manifest_url = link
        .pointer
        .join(&pointer.manifest)
        .map_err(|_| SyncError::BadLink(pointer.manifest.clone()))?;
    let (manifest, _) = get(http, &manifest_url, None)
        .await?
        .ok_or_else(|| SyncError::NotFound(manifest_url.to_string()))?;
    let actual = hex::encode(Sha512::digest(&manifest));
    if !actual.eq_ignore_ascii_case(&pointer.sha512) {
        return Err(SyncError::ManifestMismatch(manifest_url.to_string()));
    }
    let sig = get_sig(http, &manifest_url).await?;
    check_signature(key.as_ref(), &manifest, sig.as_deref(), "release manifest")?;
    let release: Release = riven_format::from_str(&String::from_utf8_lossy(&manifest))?;

    if let Some(key) = &key {
        trusted.keys.insert(source, key.to_string());
    }
    Ok(Fetched::Release(Box::new(Remote {
        pointer,
        release,
        manifest_url,
        key,
        etag,
    })))
}

/// Reads a `.riven` archive: its manifest, with every embedded blob added to the store.
pub fn read_archive(archive: &[u8], store: &crate::Store) -> Result<Release, SyncError> {
    use std::io::{Cursor, Read};
    let bad = |e: zip::result::ZipError| SyncError::Archive(e.to_string());
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(bad)?;
    let mut text = String::new();
    zip.by_name("manifest.json")
        .map_err(bad)?
        .read_to_string(&mut text)
        .map_err(|e| SyncError::Archive(e.to_string()))?;
    let release: Release = riven_format::from_str(&text)?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(bad)?;
        let name = file.name().map_err(bad)?.into_owned();
        let Some(sha512) = name
            .strip_prefix("blobs/")
            .and_then(|r| r.split('/').nth(1))
        else {
            continue;
        };
        let expected = riven_format::Hashes {
            sha512: Some(sha512.to_owned()),
            ..Default::default()
        };
        let mut bytes = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut bytes)
            .map_err(|e| SyncError::Archive(e.to_string()))?;
        store.insert(&bytes, &expected, &name)?;
    }
    Ok(release)
}

#[cfg(test)]
mod tests {
    use riven_format::KeyPair;

    use super::*;

    #[test]
    fn channels_switch_within_one_pack() {
        let stable = "https://bx-team.github.io/vc/channels/stable.json";
        assert_eq!(channel_of(stable), Some("stable"));
        assert_eq!(
            with_channel(stable, "beta").as_deref(),
            Some("https://bx-team.github.io/vc/channels/beta.json")
        );
        assert!(same_pack(
            stable,
            "https://bx-team.github.io/vc/channels/beta.json"
        ));
        assert!(!same_pack(
            stable,
            "https://other.github.io/vc/channels/beta.json"
        ));
        assert_eq!(with_channel(stable, "../x"), None);
        assert_eq!(channel_of("/home/me/pack.riven"), None);
    }

    #[test]
    fn links_resolve_to_channel_pointers() {
        let link = parse_link("https://pack.videcraft.net", "stable").unwrap();
        assert_eq!(
            link.pointer.as_str(),
            "https://pack.videcraft.net/channels/stable.json"
        );
        let key = KeyPair::generate().unwrap().public();
        let raw = format!("https://x.test/dist/channels/beta.json#key={key}");
        let link = parse_link(&raw, "stable").unwrap();
        assert_eq!(
            link.pointer.as_str(),
            "https://x.test/dist/channels/beta.json"
        );
        assert_eq!(link.key, Some(key.clone()));
        let link = parse_link(&format!("gh:BX-Team/videcraft#key={key}"), "beta").unwrap();
        assert_eq!(
            link.pointer.as_str(),
            "https://bx-team.github.io/videcraft/channels/beta.json"
        );
        assert_eq!(link.key, Some(key));
        let link = parse_link("gh:BX-Team/bx-team.github.io", "stable").unwrap();
        assert_eq!(
            link.pointer.as_str(),
            "https://bx-team.github.io/channels/stable.json"
        );
        assert!(parse_link("gh:owner/../x", "stable").is_err());
        assert!(parse_link("gh:owner", "stable").is_err());
        assert!(parse_link("file:///etc/passwd", "stable").is_err());
        assert!(parse_link("https://x.test/#key=ed25519:bad", "stable").is_err());
    }

    #[test]
    fn keys_are_pinned_and_never_silently_replaced() {
        let a = KeyPair::generate().unwrap();
        let b = KeyPair::generate().unwrap();
        let (pa, pb) = (a.public(), b.public());
        let declared_b = pb.to_string();

        assert_eq!(
            expected_key(None, None, Some(&declared_b)).unwrap(),
            Some(pb.clone())
        );
        assert!(matches!(
            expected_key(None, Some(&pa), Some(&declared_b)),
            Err(SyncError::KeyChanged)
        ));
        assert!(matches!(
            expected_key(Some(&pb), Some(&pa), None),
            Err(SyncError::KeyChanged)
        ));
        assert!(matches!(
            expected_key(Some(&pa), None, Some(&declared_b)),
            Err(SyncError::KeyChanged)
        ));
        assert_eq!(
            expected_key(Some(&pa), None, None).unwrap(),
            Some(pa.clone())
        );
        assert_eq!(expected_key(None, None, None).unwrap(), None);

        let bytes = b"pointer";
        let sig = a.sign(bytes);
        assert!(check_signature(Some(&pa), bytes, Some(&sig), "x").is_ok());
        assert!(matches!(
            check_signature(Some(&pa), bytes, None, "x"),
            Err(SyncError::Unsigned(_))
        ));
        assert!(matches!(
            check_signature(Some(&pb), bytes, Some(&sig), "x"),
            Err(SyncError::BadSignature(_))
        ));
        assert!(check_signature(None, bytes, None, "x").is_ok());
    }
}
