use crate::{Error, Result, Url};
use percent_encoding::percent_decode_str;

/// Validate network URLs at the boundary, including redirects. Userinfo is never accepted.
pub fn network_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|e| Error::new("URL", e))?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::new(
            "URL",
            "Expected an HTTP(S) URL without embedded credentials",
        ));
    }
    Ok(url)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceInput {
    Canonical(Url),
    /// More than one ref/path split is possible. Resolve through GitHub's contents API.
    Github {
        owner: String,
        repo: String,
        tail: Vec<String>,
    },
}

pub fn source_input(value: &str) -> Result<SourceInput> {
    let mut url = network_url(value.trim())?;
    if url.scheme() != "https"
        && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    {
        return Err(Error::new(
            "Source URL",
            "Sources require HTTPS (HTTP is allowed only on loopback for local catalogs)",
        ));
    }
    url.set_fragment(None);
    if matches!(url.host_str(), Some("github.com" | "www.github.com")) {
        let parts: Vec<String> = url
            .path_segments()
            .into_iter()
            .flatten()
            .map(|s| percent_decode_str(s).decode_utf8().map(|s| s.into_owned()))
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| Error::new("Source URL", e))?;
        if parts.len() < 5 || parts[2] != "blob" || parts.iter().any(|s| s.is_empty()) {
            return Err(Error::new(
                "Source URL",
                "Use a GitHub file's blob URL or its raw HTTPS URL",
            ));
        }
        let owner = parts[0].clone();
        let repo = parts[1].clone();
        let tail = parts[3..].to_vec();
        // A percent-encoded slash in a ref is already an unambiguous single segment.
        if tail.len() == 2 {
            return Ok(SourceInput::Canonical(github_raw(
                &owner,
                &repo,
                &tail[0],
                &tail[1..],
            )?));
        }
        if tail.len() > 16 {
            return Err(Error::new(
                "Source URL",
                "Ambiguous GitHub path is too long; use the raw URL",
            ));
        }
        return Ok(SourceInput::Github { owner, repo, tail });
    }
    if url.host_str() == Some("raw.githubusercontent.com") {
        // Only GitHub source URLs have known presentation/tracking query parameters.
        let pairs: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| {
                !k.starts_with("utm_") && !matches!(k.as_ref(), "raw" | "fbclid" | "gclid")
            })
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        url.set_query(None);
        if !pairs.is_empty() {
            url.query_pairs_mut().extend_pairs(pairs);
        }
    }
    Ok(SourceInput::Canonical(url))
}

pub fn github_raw(owner: &str, repo: &str, reference: &str, path: &[String]) -> Result<Url> {
    let mut url = Url::parse("https://raw.githubusercontent.com/").unwrap();
    url.path_segments_mut()
        .unwrap()
        .clear()
        .extend([owner, repo, reference])
        .extend(path);
    Ok(url)
}

pub fn relative_url(base: &Url, value: &str) -> Result<Url> {
    network_url(base.join(value).map_err(|e| Error::new("URL", e))?.as_str())
}
