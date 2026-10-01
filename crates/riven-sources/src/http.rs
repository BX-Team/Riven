use reqwest::StatusCode;
use serde::de::DeserializeOwned;

use crate::Cache;

const USER_AGENT: &str = concat!(
    "BX-Team/Riven/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/BX-Team/Riven)"
);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request to {url} failed: {source}")]
    Http { url: String, source: reqwest::Error },
    #[error("{url} returned {status}")]
    Status { url: String, status: StatusCode },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unexpected response from {url}: {source}")]
    Json {
        url: String,
        source: serde_json::Error,
    },
    #[error("{0}")]
    Unsupported(String),
}

/// The shared HTTP client every source uses.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .expect("TLS backend initializes")
}

/// GETs JSON, serving fresh responses from `cache` when one is given.
pub(crate) async fn get_json<T: DeserializeOwned>(
    http: &reqwest::Client,
    cache: Option<&Cache>,
    url: &str,
) -> Result<T, Error> {
    let parse = |body: &str| {
        serde_json::from_str(body).map_err(|source| Error::Json {
            url: url.to_owned(),
            source,
        })
    };
    if let Some(body) = cache.and_then(|c| c.get(url)) {
        return parse(&body);
    }
    tracing::debug!("GET {url}");
    let http_err = |source| Error::Http {
        url: url.to_owned(),
        source,
    };
    let response = http.get(url).send().await.map_err(http_err)?;
    match response.status() {
        StatusCode::NOT_FOUND => return Err(Error::NotFound(url.to_owned())),
        status if !status.is_success() => {
            return Err(Error::Status {
                url: url.to_owned(),
                status,
            });
        }
        _ => {}
    }
    let body = response.text().await.map_err(http_err)?;
    let value = parse(&body)?;
    if let Some(cache) = cache {
        cache.put(url, &body);
    }
    Ok(value)
}
