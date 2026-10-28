use crate::{Catalog, Error, Release, Result, Url};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn source_key(url: &Url) -> String {
    hex::encode(Sha256::digest(url.as_str().as_bytes()))
}
/// Opaque account/team scope. No Apple tokens, passwords, or cookies enter these files.
pub fn account_key(email: &str, team: &str) -> String {
    hex::encode(Sha256::digest(
        format!("{}\n{team}", email.to_lowercase()).as_bytes(),
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub key: String,
    pub url: Url,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub catalog: Catalog,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub last_success: Option<DateTime<Utc>>,
    #[serde(default)]
    pub error: Option<String>,
    /// Invalidates async completions on disable/remove/re-enable.
    #[serde(default)]
    pub generation: u64,
}
fn yes() -> bool {
    true
}
fn schema() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStore {
    #[serde(default = "schema")]
    pub schema: u32,
    #[serde(default)]
    pub sources: Vec<Source>,
}
impl Default for SourceStore {
    fn default() -> Self {
        Self {
            schema: 1,
            sources: vec![],
        }
    }
}

impl SourceStore {
    pub fn load(path: &Path) -> Result<Self> {
        let store: Self = load_json(path)?;
        if store.schema > 1 {
            return Err(Error::new(
                "Source storage",
                "This sources file was created by a newer Impactor",
            ));
        }
        Ok(store)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_json(path, self)
    }
    pub fn add(&mut self, source: Source) -> Result<()> {
        if self.sources.iter().any(|s| s.key == source.key) {
            return Err(Error::new("Source", "This source URL is already added"));
        }
        self.sources.push(source);
        Ok(())
    }
    pub fn source(&self, key: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.key == key)
    }
    pub fn accepts(&self, key: &str, generation: u64) -> bool {
        self.source(key)
            .is_some_and(|s| s.enabled && s.generation == generation)
    }
    pub fn set_enabled(&mut self, key: &str, enabled: bool) {
        if let Some(s) = self.sources.iter_mut().find(|s| s.key == key) {
            s.enabled = enabled;
            s.generation = s.generation.wrapping_add(1);
        }
    }
    pub fn remove(&mut self, key: &str) {
        self.sources.retain(|s| s.key != key);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRecord {
    pub source_key: String,
    pub original_bundle_id: String,
    pub release: Release,
    pub artifact_sha256: String,
    pub installed_bundle_id: String,
    pub device_udid: String,
    pub account_key: String,
    #[serde(default)]
    pub team_id: String,
    /// Serialized existing SignerOptions; never authentication material.
    pub settings: serde_json::Value,
    pub installed_at: DateTime<Utc>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InstallStore {
    #[serde(default)]
    pub installs: Vec<InstallRecord>,
}
impl InstallStore {
    pub fn load(path: &Path) -> Result<Self> {
        load_json(path)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_json(path, self)
    }
    pub fn find(
        &self,
        source: &str,
        bundle: &str,
        device: &str,
        account: &str,
    ) -> Option<&InstallRecord> {
        self.installs.iter().find(|r| {
            r.source_key == source
                && r.original_bundle_id == bundle
                && r.device_udid == device
                && r.account_key == account
        })
    }
    pub fn identity_conflict(
        &self,
        source: &str,
        bundle: &str,
        device: &str,
        account: &str,
        identity: &str,
    ) -> bool {
        self.installs.iter().any(|r| {
            r.device_udid == device
                && r.installed_bundle_id == identity
                && (r.source_key != source
                    || r.original_bundle_id != bundle
                    || r.account_key != account)
        })
    }
    /// Call only after the device installer confirms success. Replacement is scoped to all four keys.
    pub fn record_success(&mut self, record: InstallRecord) {
        self.installs.retain(|r| {
            !(r.source_key == record.source_key
                && r.original_bundle_id == record.original_bundle_id
                && r.device_udid == record.device_udid
                && r.account_key == record.account_key)
        });
        self.installs.push(record);
    }
}

pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::read(path) {
        Ok(data) => serde_json::from_slice(&data)
            .map_err(|e| Error::new("Storage", format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(Error::new("Storage", format!("{}: {e}", path.display()))),
    }
}
pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| Error::new("Storage", e))?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| Error::new("Storage", e))?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|e| Error::new("Storage", e))?;
    file.flush()
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| Error::new("Storage", e))?;
    file.persist(path).map_err(|e| Error::new("Storage", e))?;
    #[cfg(unix)]
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| Error::new("Storage", e))?;
    Ok(())
}

/// A download never writes here: only confirmed installations create provenance.
pub fn installs_path(data_dir: &Path) -> PathBuf {
    data_dir.join("source-installs.json")
}
