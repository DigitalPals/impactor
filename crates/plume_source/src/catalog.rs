use crate::{Error, Result, Url, relative_url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{cmp::Ordering, collections::HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub name: String,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon_url: Option<Url>,
    pub apps: Vec<App>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    pub bundle_id: String,
    #[serde(default)]
    pub developer: Option<String>,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon_url: Option<Url>,
    #[serde(default)]
    pub screenshots: Vec<Url>,
    /// Source order, newest first, as defined by the Classic specification.
    pub releases: Vec<Release>,
    #[serde(default)]
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    pub download_url: Url,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub min_os: Option<String>,
    #[serde(default)]
    pub max_os: Option<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub os_version: String,
    pub platform: String,
    /// UIDeviceFamily: 1 = iPhone/iPod, 2 = iPad, 3 = TV, 4 = Watch.
    pub family: u64,
}

fn field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}
fn required(v: &Value, key: &str) -> Result<String> {
    field(v, key).ok_or_else(|| Error::new("Source parse", format!("Missing/invalid {key}")))
}
fn optional_url(v: &Value, key: &str, base: &Url, warnings: &mut Vec<String>) -> Option<Url> {
    field(v, key).and_then(|s| match relative_url(base, &s) {
        Ok(u) => Some(u),
        Err(e) => {
            warnings.push(format!("Ignoring {key}: {e}"));
            None
        }
    })
}

pub fn parse_catalog(bytes: &[u8], base: &Url) -> Result<Catalog> {
    let input = std::str::from_utf8(bytes)
        .map_err(|e| Error::new("Source parse", e))?
        .trim_start_matches('\u{feff}');
    if input.trim_start().starts_with('<') {
        return Err(Error::new(
            "Source parse",
            "Received HTML/XML instead of a JSON source; use the raw file URL",
        ));
    }
    let value: Value = serde_json::from_str(input)
        .or_else(|_| json5::from_str(input))
        .map_err(|e| Error::new("Source parse", e))?;
    let name = required(&value, "name")?;
    let apps = value
        .get("apps")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("Source parse", "Missing/invalid apps array"))?;
    if apps.len() > 10_000 {
        return Err(Error::new("Source parse", "Catalog exceeds 10,000 apps"));
    }
    let mut diagnostics = vec![];
    let mut valid_apps = vec![];
    let mut ids = HashSet::new();
    for (i, app) in apps.iter().enumerate() {
        match parse_app(app, base, &mut diagnostics) {
            Ok(app) => {
                if ids.insert(app.bundle_id.clone()) {
                    valid_apps.push(app);
                } else {
                    diagnostics.push(format!(
                        "App {}: duplicate bundleIdentifier {} in this source; ignored",
                        i + 1,
                        app.bundle_id
                    ));
                }
            }
            Err(e) => diagnostics.push(format!("App {}: {e}", i + 1)),
        }
    }
    if !apps.is_empty() && valid_apps.is_empty() {
        return Err(Error::new(
            "Source parse",
            format!("No valid apps. {}", diagnostics.join("; ")),
        ));
    }
    if apps.is_empty() {
        diagnostics.push("This source has an empty catalog.".into());
    }
    let icon_url = optional_url(&value, "iconURL", base, &mut diagnostics);
    Ok(Catalog {
        name,
        identifier: field(&value, "identifier"),
        source_url: field(&value, "sourceURL"),
        subtitle: field(&value, "subtitle"),
        description: field(&value, "description"),
        icon_url,
        apps: valid_apps,
        diagnostics,
    })
}

fn parse_app(v: &Value, base: &Url, warnings: &mut Vec<String>) -> Result<App> {
    let name = required(v, "name")?;
    let bundle_id = required(v, "bundleIdentifier")?;
    if !valid_bundle_id(&bundle_id) {
        return Err(Error::new(
            "Source parse",
            format!("{name}: invalid bundleIdentifier {bundle_id}"),
        ));
    }
    let mut releases = vec![];
    if let Some(versions) = v.get("versions").and_then(Value::as_array) {
        if versions.len() > 2_000 {
            return Err(Error::new("Source parse", "More than 2,000 versions"));
        }
        for (i, version) in versions.iter().enumerate() {
            match parse_release(version, base) {
                Ok(r) => {
                    if !releases.iter().any(|x: &Release| {
                        x.version == r.version
                            && x.build == r.build
                            && x.download_url == r.download_url
                            && x.sha256 == r.sha256
                    }) {
                        releases.push(r);
                    }
                }
                Err(e) => warnings.push(format!("{name}, release {}: {e}", i + 1)),
            }
        }
    }
    if releases.is_empty()
        && let Ok(r) = parse_release(v, base)
    {
        warnings.push(format!(
            "{name}: using legacy flat version/downloadURL fields (no valid versions record)."
        ));
        releases.push(r);
    }
    let unavailable = if v.get("patreon").is_some_and(|p| !p.is_null()) {
        Some("Paid-source authentication is unsupported.".into())
    } else if v.get("marketplaceID").is_some() {
        Some("AltStore PAL marketplace packages are unsupported.".into())
    } else if releases.is_empty() {
        Some("No valid IPA releases. Check this source's diagnostics.".into())
    } else {
        None
    };
    if releases.is_empty() {
        warnings.push(format!("{name}: no valid IPA releases"));
    }
    let icon_url = optional_url(v, "iconURL", base, warnings);
    let mut screenshots = vec![];
    fn collect(v: &Value, base: &Url, out: &mut Vec<Url>) {
        if out.len() >= 50 {
            return;
        }
        match v {
            Value::String(s) => {
                if let Ok(u) = relative_url(base, s) {
                    out.push(u);
                }
            }
            Value::Array(a) => {
                for s in a {
                    collect(s, base, out);
                }
            }
            Value::Object(o) => {
                if let Some(s) = o.get("imageURL") {
                    collect(s, base, out);
                } else {
                    for key in ["iphone", "ipad"] {
                        if let Some(a) = o.get(key) {
                            collect(a, base, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(s) = v.get("screenshots").or_else(|| v.get("screenshotURLs")) {
        collect(s, base, &mut screenshots);
    }
    Ok(App {
        name,
        bundle_id,
        developer: field(v, "developerName"),
        subtitle: field(v, "subtitle"),
        description: field(v, "localizedDescription").or_else(|| field(v, "description")),
        icon_url,
        screenshots,
        releases,
        unavailable,
    })
}

fn parse_release(v: &Value, base: &Url) -> Result<Release> {
    let version = required(v, "version")?;
    let download_url = relative_url(base, &required(v, "downloadURL")?)?;
    let sha256 = field(v, "sha256").map(|s| s.to_ascii_lowercase());
    if sha256
        .as_ref()
        .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(Error::new(
            "Source parse",
            "Invalid sha256; expected 64 hexadecimal characters",
        ));
    }
    let min_os = field(v, "minOSVersion");
    let max_os = field(v, "maxOSVersion");
    if min_os
        .iter()
        .chain(max_os.iter())
        .any(|s| numeric_version(s).is_none())
    {
        return Err(Error::new("Source parse", "Invalid OS version requirement"));
    }
    if let (Some(min), Some(max)) = (&min_os, &max_os)
        && compare_numeric(min, max) == Some(Ordering::Greater)
    {
        return Err(Error::new(
            "Source parse",
            "minOSVersion exceeds maxOSVersion",
        ));
    }
    let platforms = match v.get("platforms").or_else(|| v.get("platform")) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };
    Ok(Release {
        version,
        build: field(v, "buildVersion"),
        date: field(v, "date").or_else(|| field(v, "versionDate")),
        notes: field(v, "localizedDescription").or_else(|| field(v, "versionDescription")),
        download_url,
        size: v.get("size").and_then(|s| {
            s.as_u64()
                .or_else(|| s.as_str().and_then(|s| s.parse().ok()))
        }),
        sha256,
        min_os,
        max_os,
        platforms,
    })
}

pub fn valid_bundle_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.contains('.')
        && s.split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
}

fn numeric_version(v: &str) -> Option<Vec<u64>> {
    if v.is_empty() {
        return None;
    }
    v.split('.')
        .map(|p| {
            if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) {
                p.parse().ok()
            } else {
                None
            }
        })
        .collect()
}
pub fn compare_numeric(a: &str, b: &str) -> Option<Ordering> {
    let a = numeric_version(a)?;
    let b = numeric_version(b)?;
    Some(
        (0..a.len().max(b.len()))
            .map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)))
            .find(|c| *c != Ordering::Equal)
            .unwrap_or(Ordering::Equal),
    )
}

pub fn platform_matches(platform: &str, device: &str) -> bool {
    fn normalized(p: &str) -> String {
        match p.to_ascii_lowercase().as_str() {
            "iphoneos" | "ios" | "ipados" => "ios".into(),
            "appletvos" | "tvos" => "tvos".into(),
            "watchos" => "watchos".into(),
            "macosx" | "macos" => "macos".into(),
            _ => p.to_ascii_lowercase(),
        }
    }
    normalized(platform) == normalized(device)
}
impl Release {
    pub fn incompatibility(&self, device: &DeviceProfile) -> Option<String> {
        if !self.platforms.is_empty()
            && !self
                .platforms
                .iter()
                .any(|p| platform_matches(p, &device.platform))
        {
            return Some(format!(
                "Requires platform {} (selected: {})",
                self.platforms.join(", "),
                device.platform
            ));
        }
        if let Some(min) = &self.min_os {
            match compare_numeric(&device.os_version, min) {
                Some(Ordering::Less) => {
                    return Some(format!(
                        "Requires OS {min}+ (selected: {})",
                        device.os_version
                    ));
                }
                None => return Some("Unable to determine device OS version".into()),
                _ => {}
            }
        }
        if let Some(max) = &self.max_os {
            match compare_numeric(&device.os_version, max) {
                Some(Ordering::Greater) => {
                    return Some(format!(
                        "Requires OS {max} or earlier (selected: {})",
                        device.os_version
                    ));
                }
                None => return Some("Unable to determine device OS version".into()),
                _ => {}
            }
        }
        None
    }
    pub fn same_release(&self, other: &Self) -> bool {
        self.version == other.version && self.build == other.build
    }
    pub fn same_artifact(&self, other: &Self) -> bool {
        match (&self.sha256, &other.sha256) {
            (Some(a), Some(b)) => a == b,
            _ => self.download_url == other.download_url,
        }
    }
}
impl App {
    pub fn newest_eligible(&self, device: Option<&DeviceProfile>) -> Option<usize> {
        if self.unavailable.is_some() {
            return None;
        }
        self.releases
            .iter()
            .position(|r| device.is_none_or(|d| r.incompatibility(d).is_none()))
    }
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        [
            &self.name,
            &self.bundle_id,
            self.developer.as_deref().unwrap_or(""),
        ]
        .iter()
        .any(|s| s.to_lowercase().contains(&q))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseChange {
    Same,
    Newer,
    Older,
    Ambiguous,
}

/// Dates never imply an update. Numeric versions/builds prevent downgrades. For nonnumeric
/// releases, use publisher ordering only while the installed release is still in history.
pub fn release_change(app: &App, candidate: usize, installed: &Release) -> ReleaseChange {
    let Some(new) = app.releases.get(candidate) else {
        return ReleaseChange::Ambiguous;
    };
    if new.same_release(installed) {
        if !new.same_artifact(installed) {
            return ReleaseChange::Ambiguous;
        }
        return ReleaseChange::Same;
    }
    let cmp = compare_numeric(&new.version, &installed.version);
    let cmp = if cmp == Some(Ordering::Equal) {
        match (&new.build, &installed.build) {
            (Some(a), Some(b)) => compare_numeric(a, b),
            _ => None,
        }
    } else {
        cmp
    };
    match cmp {
        Some(Ordering::Greater) => ReleaseChange::Newer,
        Some(Ordering::Less) => ReleaseChange::Older,
        Some(Ordering::Equal) => ReleaseChange::Same,
        None => match app
            .releases
            .iter()
            .position(|r| r.same_release(installed) && r.same_artifact(installed))
        {
            Some(i) if candidate < i => ReleaseChange::Newer,
            Some(i) if candidate > i => ReleaseChange::Older,
            _ => ReleaseChange::Ambiguous,
        },
    }
}
