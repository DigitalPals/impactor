use crate::{
    CancellationToken, Error, Result, Source, SourceInput, Url, github_raw, network_url,
    parse_catalog, source_input, source_key,
};
use reqwest::{
    Client, StatusCode,
    header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

pub(crate) fn request_error(stage: &'static str, error: reqwest::Error) -> Error {
    let detail = if error.is_timeout() {
        "Request timed out. Check the connection and retry."
    } else if error.is_connect() {
        "Could not connect. Check the connection, proxy, and source address."
    } else if error.is_redirect() {
        "Redirect failed or exceeded the redirect limit."
    } else if error.is_body() || error.is_decode() {
        "Incomplete or invalid HTTP response body. Retry the download."
    } else {
        "HTTP request failed. Check the source address and retry."
    };
    Error::new(stage, detail)
}

pub const MAX_FEED_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_IPA_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct SourceClient {
    pub(crate) client: Client,
    pub(crate) permits: Arc<Semaphore>,
}
impl SourceClient {
    pub fn new() -> Result<Self> {
        Self::with_timeout(Duration::from_secs(30))
    }
    pub fn with_timeout(timeout: Duration) -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("Impactor-Sources/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(timeout)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 8 {
                    attempt.error("Too many redirects")
                } else if network_url(attempt.url().as_str()).is_err() {
                    attempt.error("Unsafe redirect URL")
                } else if attempt
                    .previous()
                    .last()
                    .is_some_and(|u| u.scheme() == "https")
                    && attempt.url().scheme() != "https"
                {
                    attempt.error("HTTPS downgrade redirect rejected")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|e| Error::new("Network", e.without_url()))?;
        Ok(Self {
            client,
            permits: Arc::new(Semaphore::new(4)),
        })
    }
    pub async fn canonical_url(&self, input: &str, cancel: &CancellationToken) -> Result<Url> {
        match source_input(input)? {
            SourceInput::Canonical(u) => Ok(u),
            SourceInput::Github { owner, repo, tail } => {
                let mut matches = vec![];
                for split in 1..tail.len() {
                    let reference = tail[..split].join("/");
                    let mut api = Url::parse("https://api.github.com/").unwrap();
                    api.path_segments_mut()
                        .unwrap()
                        .clear()
                        .extend(["repos", &owner, &repo, "contents"])
                        .extend(&tail[split..]);
                    api.query_pairs_mut().append_pair("ref", &reference);
                    let response = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Source fetch","Cancelled")),
                    r=self.client.get(api).timeout(Duration::from_secs(45)).send()=>r.map_err(|e|crate::network::request_error("Source fetch",e))? };
                    if response.status() == StatusCode::NOT_FOUND {
                        continue;
                    }
                    if !response.status().is_success() {
                        return Err(Error::new(
                            "Source fetch",
                            format!(
                                "GitHub ref resolution returned HTTP {}. Use the raw URL, or retry later.",
                                response.status()
                            ),
                        ));
                    }
                    let bytes =
                        limited_body(response, MAX_FEED_BYTES, cancel, "Source fetch").await?;
                    let v: serde_json::Value = serde_json::from_slice(&bytes)
                        .map_err(|e| Error::new("Source fetch", e))?;
                    if v["type"] == "file" {
                        matches.push(github_raw(&owner, &repo, &reference, &tail[split..])?);
                    }
                }
                if matches.len() != 1 {
                    return Err(Error::new(
                        "Source URL",
                        "GitHub branch/path is ambiguous or unavailable. Copy the file's Raw URL.",
                    ));
                }
                Ok(matches.remove(0))
            }
        }
    }
    pub async fn add(&self, input: &str, cancel: &CancellationToken) -> Result<Source> {
        let url = self.canonical_url(input, cancel).await?;
        self.fetch(url, None, cancel)
            .await?
            .ok_or_else(|| Error::new("Source fetch", "Unexpected 304 for a new source"))
    }
    pub async fn refresh(&self, source: &Source, cancel: &CancellationToken) -> Result<Source> {
        match self.fetch(source.url.clone(), Some(source), cancel).await? {
            Some(mut fresh) => {
                fresh.enabled = source.enabled;
                fresh.generation = source.generation;
                Ok(fresh)
            }
            None => {
                let mut s = source.clone();
                s.last_success = Some(chrono::Utc::now());
                s.error = None;
                Ok(s)
            }
        }
    }
    async fn fetch(
        &self,
        url: Url,
        previous: Option<&Source>,
        cancel: &CancellationToken,
    ) -> Result<Option<Source>> {
        let _permit = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Source fetch","Cancelled")),p=self.permits.acquire()=>p.map_err(|e|Error::new("Source fetch",e))? };
        let mut request = self
            .client
            .get(url.clone())
            .timeout(Duration::from_secs(60));
        if let Some(old) = previous {
            if let Some(etag) = &old.etag {
                request = request.header(IF_NONE_MATCH, etag);
            }
            if let Some(last) = &old.last_modified {
                request = request.header(IF_MODIFIED_SINCE, last);
            }
        }
        let response = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Source fetch","Cancelled")),r=request.send()=>r.map_err(|e|crate::network::request_error("Source fetch",e))? };
        if response.status() == StatusCode::NOT_MODIFIED && previous.is_some() {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Error::new(
                "Source fetch",
                format!(
                    "HTTP {}. Check the source address or retry later.",
                    response.status()
                ),
            ));
        }
        if is_html(&response) {
            return Err(Error::new(
                "Source fetch",
                "Server returned HTML, not JSON. Check the raw URL or whether the source requires login.",
            ));
        }
        let fetched_url = response.url().clone();
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|s| s.to_str().ok())
                .map(str::to_owned)
        };
        let etag = header(ETAG);
        let last_modified = header(LAST_MODIFIED);
        let bytes = limited_body(response, MAX_FEED_BYTES, cancel, "Source fetch").await?;
        let catalog = tokio::task::spawn_blocking(move || parse_catalog(&bytes, &fetched_url))
            .await
            .map_err(|e| Error::new("Source parse", e))??;
        if cancel.is_cancelled() {
            return Err(Error::new("Source fetch", "Cancelled"));
        }
        Ok(Some(Source {
            key: source_key(&url),
            url,
            enabled: true,
            catalog,
            etag,
            last_modified,
            last_success: Some(chrono::Utc::now()),
            error: None,
            generation: 0,
        }))
    }
    pub async fn image(&self, url: &Url, cancel: &CancellationToken) -> Result<Vec<u8>> {
        network_url(url.as_str())?;
        let _permit = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Image","Cancelled")),p=self.permits.acquire()=>p.map_err(|e|Error::new("Image",e))? };
        let response = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Image","Cancelled")),r=self.client.get(url.clone()).timeout(Duration::from_secs(30)).send()=>r.map_err(|e|crate::network::request_error("Image",e))? };
        if !response.status().is_success() || is_html(&response) {
            return Err(Error::new("Image", "Artwork unavailable"));
        }
        limited_body(response, MAX_IMAGE_BYTES, cancel, "Image").await
    }
}
pub(crate) fn is_html(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.contains("text/html") || s.contains("application/xhtml"))
}
pub(crate) async fn limited_body(
    mut response: reqwest::Response,
    max: u64,
    cancel: &CancellationToken,
    stage: &'static str,
) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > max) {
        return Err(Error::new(stage, "Response exceeds size limit"));
    }
    let mut bytes = vec![];
    loop {
        let chunk = tokio::select! { _=cancel.cancelled()=>return Err(Error::new(stage,"Cancelled")),r=response.chunk()=>r.map_err(|e|request_error(stage,e))? };
        let Some(chunk) = chunk else {
            break;
        };
        if bytes.len() as u64 + chunk.len() as u64 > max {
            return Err(Error::new(stage, "Response exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
