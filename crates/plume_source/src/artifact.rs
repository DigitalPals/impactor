use crate::{
    CancellationToken, DeviceProfile, Error, MAX_IPA_BYTES, Release, Result, SourceClient,
    compare_numeric, network_url, platform_matches,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Default)]
pub struct DownloadProgress {
    pub received: AtomicU64,
    pub total: AtomicU64,
}
impl DownloadProgress {
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.received.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

#[derive(Debug)]
pub struct ValidatedArtifact {
    // A private random filename owned by the app, removed on cancel/error/drop.
    file: tempfile::NamedTempFile,
    pub sha256: String,
    pub metadata: IpaMetadata,
}
impl ValidatedArtifact {
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaMetadata {
    pub bundle_id: String,
    pub version: String,
    pub build: Option<String>,
    pub min_os: Option<String>,
    pub platforms: Vec<String>,
    pub families: Vec<u64>,
}

impl SourceClient {
    pub async fn download(
        &self,
        release: &Release,
        bundle_id: &str,
        device: &DeviceProfile,
        dir: &Path,
        cancel: &CancellationToken,
        progress: &DownloadProgress,
    ) -> Result<ValidatedArtifact> {
        network_url(release.download_url.as_str())?;
        if let Some(reason) = release.incompatibility(device) {
            return Err(Error::new("IPA validation", reason));
        }
        let _permit = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Download","Cancelled")),p=self.permits.acquire()=>p.map_err(|e|Error::new("Download",e))? };
        let mut response = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Download","Cancelled")),r=self.client.get(release.download_url.clone()).timeout(std::time::Duration::from_secs(20*60)).send()=>r.map_err(|e|crate::network::request_error("Download",e))? };
        if !response.status().is_success() {
            return Err(Error::new(
                "Download",
                format!(
                    "HTTP {}. Retry, or check the release with its publisher.",
                    response.status()
                ),
            ));
        }
        if crate::network::is_html(&response) {
            return Err(Error::new(
                "Download",
                "Server returned HTML instead of an IPA (login page, expired link, or rate limit)",
            ));
        }
        let length = response.content_length();
        if length.is_some_and(|n| n > MAX_IPA_BYTES)
            || release.size.is_some_and(|n| n > MAX_IPA_BYTES)
        {
            return Err(Error::new("Download", "IPA exceeds 4 GiB limit"));
        }
        progress
            .total
            .store(length.or(release.size).unwrap_or(0), Ordering::Relaxed);
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| Error::new("Download", e))?;
        let directory = dir.to_owned();
        let temp = tokio::task::spawn_blocking(move || {
            tempfile::Builder::new()
                .prefix("source-")
                .suffix(".ipa")
                .tempfile_in(directory)
        })
        .await
        .map_err(|e| Error::new("Download", e))?
        .map_err(|e| Error::new("Download", e))?;
        let mut output =
            tokio::fs::File::from_std(temp.reopen().map_err(|e| Error::new("Download", e))?);
        let mut hash = Sha256::new();
        let mut size = 0u64;
        loop {
            let chunk = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Download","Cancelled")),r=response.chunk()=>r.map_err(|e|crate::network::request_error("Download",e))? };
            let Some(chunk) = chunk else {
                break;
            };
            size += chunk.len() as u64;
            if size > MAX_IPA_BYTES {
                return Err(Error::new("Download", "IPA exceeds 4 GiB limit"));
            }
            // Hashing each network chunk is bounded work. Full archive work runs below on a worker.
            hash.update(&chunk);
            output
                .write_all(&chunk)
                .await
                .map_err(|e| Error::new("Download", e))?;
            progress.received.store(size, Ordering::Relaxed);
        }
        output
            .flush()
            .await
            .map_err(|e| Error::new("Download", e))?;
        drop(output);
        if size == 0 || length.is_some_and(|n| n != size) {
            return Err(Error::new(
                "Download",
                format!("Incomplete response: received {size} bytes, expected {length:?}"),
            ));
        }
        if let Some(expected) = release.size
            && size != expected
        {
            return Err(Error::new(
                "IPA validation",
                format!("Size mismatch: expected {expected} bytes, received {size}"),
            ));
        }
        let sha256 = hex::encode(hash.finalize());
        if release
            .sha256
            .as_ref()
            .is_some_and(|expected| expected != &sha256)
        {
            return Err(Error::new(
                "IPA validation",
                format!(
                    "SHA-256 mismatch: expected {}, actual {sha256}",
                    release.sha256.as_deref().unwrap()
                ),
            ));
        }
        let expected = release.clone();
        let bundle = bundle_id.to_owned();
        let target = device.clone();
        let cancellation = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let metadata = inspect_ipa(temp.path(), &cancellation)?;
            validate_metadata(&metadata, &expected, &bundle, &target)?;
            if cancellation.is_cancelled() {
                return Err(Error::new("IPA validation", "Cancelled"));
            }
            Ok(ValidatedArtifact {
                file: temp,
                sha256,
                metadata,
            })
        })
        .await
        .map_err(|e| Error::new("IPA validation", e))?
    }
}

/// Before extraction: reject traversal, duplicate paths, symlinks, zip bombs, multiple apps,
/// and corrupt data. Reading every entry also checks ZIP CRCs, including truncated payloads.
pub fn inspect_ipa(path: &Path, cancel: &CancellationToken) -> Result<IpaMetadata> {
    let file = File::open(path).map_err(|e| Error::new("IPA validation", e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| Error::new("IPA validation", format!("Invalid IPA ZIP archive: {e}")))?;
    if zip.len() > 100_000 {
        return Err(Error::new("IPA validation", "Archive has too many entries"));
    }
    let mut total = 0u64;
    let mut names = HashSet::new();
    let mut apps = HashSet::new();
    let mut main = None;
    let mut buffer = [0u8; 64 * 1024];
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(Error::new("IPA validation", "Cancelled"));
        }
        let mut entry = zip
            .by_index(i)
            .map_err(|e| Error::new("IPA validation", e))?;
        let name = entry.name().to_owned();
        // One trailing separator denotes a directory. Repeated/interior empty
        // components alias paths during extraction (e.g. App.app//Info.plist).
        let path = name.strip_suffix('/').unwrap_or(&name);
        if entry.enclosed_name().is_none()
            || name.contains('\\')
            || path
                .split('/')
                .any(|p| p.is_empty() || p == ".." || p == "." || p.contains(':') || p.ends_with(['.',' ']))
            // Preserve the validated identity on case-insensitive Windows/macOS filesystems.
            || !names.insert(path.to_lowercase())
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err(Error::new(
                "IPA validation",
                format!("Unsafe or duplicate archive path: {name}"),
            ));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| Error::new("IPA validation", "Archive size overflow"))?;
        if total > 8 * 1024 * 1024 * 1024 || entry.size() > 2 * 1024 * 1024 * 1024 {
            return Err(Error::new(
                "IPA validation",
                "Uncompressed IPA exceeds safety limit",
            ));
        }
        let parts: Vec<_> = name.split('/').collect();
        if parts.len() >= 2 && parts[0] == "Payload" && parts[1].ends_with(".app") {
            apps.insert(parts[1].to_owned());
        }
        if apps.len() > 1 {
            return Err(Error::new(
                "IPA validation",
                "Archive contains multiple main Payload apps",
            ));
        }
        let is_info = parts.len() == 3
            && parts[0] == "Payload"
            && parts[1].ends_with(".app")
            && parts[2] == "Info.plist";
        if is_info && (main.is_some() || entry.size() > 2 * 1024 * 1024) {
            return Err(Error::new(
                "IPA validation",
                "Expected exactly one main app Info.plist, at most 2 MiB",
            ));
        }
        let mut plist = vec![];
        let mut read = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(Error::new("IPA validation", "Cancelled"));
            }
            let n = entry
                .read(&mut buffer)
                .map_err(|e| Error::new("IPA validation", format!("Corrupt entry {name}: {e}")))?;
            if n == 0 {
                break;
            }
            read += n as u64;
            if read > entry.size() {
                return Err(Error::new(
                    "IPA validation",
                    "Archive entry exceeds declared size",
                ));
            }
            if is_info {
                plist.extend_from_slice(&buffer[..n]);
            }
        }
        if read != entry.size() {
            return Err(Error::new(
                "IPA validation",
                format!("Incomplete archive entry {name}"),
            ));
        }
        if is_info {
            main = Some(plist);
        }
    }
    let main =
        main.ok_or_else(|| Error::new("IPA validation", "Missing Payload/*.app/Info.plist"))?;
    let dict: plist::Dictionary =
        plist::from_bytes(&main).map_err(|e| Error::new("IPA validation", e))?;
    let string = |key| {
        dict.get(key)
            .and_then(plist::Value::as_string)
            .map(str::to_owned)
    };
    let bundle_id = string("CFBundleIdentifier")
        .ok_or_else(|| Error::new("IPA validation", "Missing CFBundleIdentifier"))?;
    let version = string("CFBundleShortVersionString")
        .ok_or_else(|| Error::new("IPA validation", "Missing CFBundleShortVersionString"))?;
    let platforms = dict
        .get("CFBundleSupportedPlatforms")
        .and_then(plist::Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(plist::Value::as_string)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let families = dict
        .get("UIDeviceFamily")
        .and_then(plist::Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(plist::Value::as_unsigned_integer)
                .collect()
        })
        .unwrap_or_default();
    Ok(IpaMetadata {
        bundle_id,
        version,
        build: string("CFBundleVersion"),
        min_os: string("MinimumOSVersion"),
        platforms,
        families,
    })
}

pub fn validate_metadata(
    actual: &IpaMetadata,
    expected: &Release,
    bundle: &str,
    device: &DeviceProfile,
) -> Result<()> {
    if actual.bundle_id != bundle {
        return Err(Error::new(
            "IPA validation",
            format!(
                "Bundle ID mismatch: expected {bundle}, actual {}",
                actual.bundle_id
            ),
        ));
    }
    if actual.version != expected.version {
        return Err(Error::new(
            "IPA validation",
            format!(
                "Version mismatch: expected {}, actual {}. Ask the publisher to correct the feed.",
                expected.version, actual.version
            ),
        ));
    }
    if expected.build.is_some() && expected.build != actual.build {
        return Err(Error::new(
            "IPA validation",
            format!(
                "Build mismatch: expected {:?}, actual {:?}",
                expected.build, actual.build
            ),
        ));
    }
    if actual.platforms.is_empty()
        || !actual
            .platforms
            .iter()
            .any(|p| platform_matches(p, &device.platform))
    {
        return Err(Error::new(
            "IPA validation",
            format!(
                "IPA platforms {:?} do not support selected platform {}",
                actual.platforms, device.platform
            ),
        ));
    }
    if !actual.families.is_empty() && !actual.families.contains(&device.family) {
        return Err(Error::new(
            "IPA validation",
            format!(
                "IPA device families {:?} do not support selected family {}",
                actual.families, device.family
            ),
        ));
    }
    if let Some(min) = &actual.min_os {
        match compare_numeric(&device.os_version, min) {
            Some(std::cmp::Ordering::Less) | None => {
                return Err(Error::new(
                    "IPA validation",
                    format!(
                        "IPA requires OS {min}+, selected device has {}",
                        device.os_version
                    ),
                ));
            }
            _ => {}
        }
    }
    if let Some(reason) = expected.incompatibility(device) {
        return Err(Error::new("IPA validation", reason));
    }
    Ok(())
}
