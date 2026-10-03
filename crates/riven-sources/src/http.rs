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
    get_json_with(http, cache, url, &[]).await
}

/// [`get_json`] with extra request headers; they are not part of the cache key.
pub(crate) async fn get_json_with<T: DeserializeOwned>(
    http: &reqwest::Client,
    cache: Option<&Cache>,
    url: &str,
    headers: &[(&str, &str)],
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
    let mut request = http.get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().await.map_err(http_err)?;
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

/// POSTs a JSON body and parses the JSON reply, caching by URL and body.
pub(crate) async fn post_json<T: DeserializeOwned>(
    http: &reqwest::Client,
    cache: Option<&Cache>,
    url: &str,
    body: &serde_json::Value,
) -> Result<T, Error> {
    let body = body.to_string();
    let key = format!("POST {url}\n{body}");
    let parse = |text: &str| {
        serde_json::from_str(text).map_err(|source| Error::Json {
            url: url.to_owned(),
            source,
        })
    };
    if let Some(text) = cache.and_then(|c| c.get(&key)) {
        return parse(&text);
    }
    tracing::debug!("POST {url}");
    let http_err = |source| Error::Http {
        url: url.to_owned(),
        source,
    };
    let response = http
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(http_err)?;
    if !response.status().is_success() {
        return Err(Error::Status {
            url: url.to_owned(),
            status: response.status(),
        });
    }
    let text = response.text().await.map_err(http_err)?;
    let value = parse(&text)?;
    if let Some(cache) = cache {
        cache.put(&key, &text);
    }
    Ok(value)
}
