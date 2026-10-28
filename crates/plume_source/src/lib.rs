//! Public, unauthenticated AltStore Classic/SideStore sources. No account client is shared here.
mod artifact;
mod catalog;
mod network;
mod store;
mod urls;

pub use artifact::*;
pub use catalog::*;
pub use network::*;
pub use store::*;
pub use tokio_util::sync::CancellationToken;
pub use url::Url;
pub use urls::*;

#[derive(Debug, thiserror::Error)]
#[error("{stage}: {detail}")]
pub struct Error {
    pub stage: &'static str,
    pub detail: String,
}

impl Error {
    pub fn new(stage: &'static str, detail: impl std::fmt::Display) -> Self {
        Self {
            stage,
            detail: detail.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
