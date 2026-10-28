mod view;

use crate::{defaults, source_install::SourceInstall};
use iced::{Subscription, Task, widget::image};

use plume_source::{
    App, CancellationToken, DeviceProfile, DownloadProgress, InstallRecord, InstallStore,
    ReleaseChange, Source, SourceClient, SourceStore, account_key, installs_path, release_change,
};
use plume_utils::{Device, Package};
use rust_i18n::t;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

type Loaded = Result<(SourceStore, InstallStore), String>;
type DeviceResult = Result<(DeviceProfile, Result<Vec<String>, String>), String>;
#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Loaded),
    UrlChanged(String),
    Add,
    AddDone(u64, Result<Source, String>),
    CancelAdd,
    RefreshAll,
    Refresh(String),
    Refreshed(String, u64, u64, Result<Source, String>),
    Toggle(String, bool),
    Remove(String),
    SelectSource(Option<String>),
    ShowUpdates,
    Search(String),
    Scroll(f32),
    FocusUrl,
    FocusSearch,
    More,
    Details(String, String),
    Back,
    SelectRelease(usize),
    Target(Option<Device>, Option<String>),
    DeviceReady(u64, DeviceResult),
    Install,
    CancelDownload,
    DownloadTick,
    DownloadDone(u64, Result<(Package, SourceInstall), String>),
    OpenInstaller(Package, SourceInstall),
    Icon(String, Option<image::Handle>),
    Saved(Result<(), String>),
    ReloadInstalls,
    InstallsLoaded(Result<InstallStore, String>),
    Shutdown(Result<(), String>),
}

struct Request {
    id: u64,
    cancel: CancellationToken,
}
struct Download {
    request: Request,
    source: String,
    progress: Arc<DownloadProgress>,
}

pub struct SourcesScreen {
    pub store: SourceStore,
    installs: InstallStore,
    client: Option<SourceClient>,
    loading: bool,
    storage_ok: bool,
    input: String,
    search: String,
    selected_source: Option<String>,
    updates_only: bool,
    detail: Option<(String, String)>,
    release: usize,
    limit: usize,
    error: Option<String>,
    add: Option<Request>,
    requests: HashMap<String, Request>,
    download: Option<Download>,
    retry_download: bool,
    serial: u64,
    save_revision: u64,
    writer: Arc<Mutex<u64>>,
    device: Option<Device>,
    account: Option<String>,
    profile: Option<DeviceProfile>,
    device_error: Option<String>,
    inventory: Option<HashSet<String>>,
    device_generation: u64,
    icons: HashMap<String, Option<image::Handle>>,
}

impl SourcesScreen {
    pub fn new() -> (Self, Task<Message>) {
        let (client, error) = match SourceClient::new() {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let task = Task::perform(
            async {
                tokio::task::spawn_blocking(|| {
                    let data = defaults::get_data_path();
                    Ok((
                        SourceStore::load(&data.join("sources.json")).map_err(|e| e.to_string())?,
                        InstallStore::load(&installs_path(&data)).map_err(|e| e.to_string())?,
                    ))
                })
                .await
                .map_err(|e| e.to_string())?
            },
            Message::Loaded,
        );
        (
            Self {
                store: SourceStore::default(),
                installs: InstallStore::default(),
                client,
                loading: true,
                storage_ok: false,
                input: String::new(),
                search: String::new(),
                selected_source: None,
                updates_only: false,
                detail: None,
                release: 0,
                limit: 100,
                error,
                add: None,
                requests: HashMap::new(),
                download: None,
                retry_download: false,
                serial: 0,
                save_revision: 0,
                writer: Arc::new(Mutex::new(0)),
                device: None,
                account: None,
                profile: None,
                device_error: None,
                inventory: None,
                device_generation: 0,
                icons: HashMap::new(),
            },
            task,
        )
    }
    fn next_id(&mut self) -> u64 {
        self.serial = self.serial.wrapping_add(1);
        self.serial
    }
    fn save(&mut self) -> Task<Message> {
        if !self.storage_ok {
            return Task::none();
        }
        self.save_revision += 1;
        let revision = self.save_revision;
        let store = self.store.clone();
        let writer = self.writer.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut latest = writer.lock().map_err(|e| e.to_string())?;
                    if revision <= *latest {
                        return Ok(());
                    }
                    store
                        .save(&defaults::get_data_path().join("sources.json"))
                        .map_err(|e| e.to_string())?;
                    *latest = revision;
                    Ok(())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            Message::Saved,
        )
    }
    fn refresh(&mut self, key: &str) -> Task<Message> {
        let Some(source) = self.store.source(key).filter(|s| s.enabled).cloned() else {
            return Task::none();
        };
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        if self.requests.contains_key(key) {
            return Task::none();
        }
        let request = Request {
            id: self.next_id(),
            cancel: CancellationToken::new(),
        };
        let id = request.id;
        let cancel = request.cancel.clone();
        let generation = source.generation;
        let key = key.to_owned();
        self.requests.insert(key.clone(), request);
        Task::perform(
            async move {
                client
                    .refresh(&source, &cancel)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| Message::Refreshed(key.clone(), generation, id, result),
        )
    }
    fn cancel_source(&mut self, key: &str) {
        if let Some(request) = self.requests.remove(key) {
            request.cancel.cancel();
        }
        if self.download.as_ref().is_some_and(|d| d.source == key) {
            self.cancel_download();
        }
    }
    fn cancel_download(&mut self) {
        if let Some(d) = self.download.take() {
            d.request.cancel.cancel();
        }
    }

    fn reconcile_device(&mut self) -> Task<Message> {
        self.inventory = None;
        self.device_error = None;
        self.device_generation += 1;
        let generation = self.device_generation;
        let Some(device) = self.device.clone() else {
            self.select_newest();
            return Task::none();
        };
        Task::perform(
            async move {
                let (os_version, platform, family) =
                    tokio::time::timeout(Duration::from_secs(15), device.source_device_info())
                        .await
                        .map_err(|_| "Device compatibility request timed out".to_string())?
                        .map_err(|e| e.to_string())?;
                let inventory =
                    tokio::time::timeout(Duration::from_secs(15), device.source_installed_ids())
                        .await
                        .map_err(|_| "Installed app reconciliation timed out".to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()));
                Ok((
                    DeviceProfile {
                        os_version,
                        platform,
                        family,
                    },
                    inventory,
                ))
            },
            move |r| Message::DeviceReady(generation, r),
        )
    }

    pub fn shutdown(&mut self) -> Task<Message> {
        if let Some(request) = self.add.take() {
            request.cancel.cancel();
        }
        for (_, request) in self.requests.drain() {
            request.cancel.cancel();
        }
        self.cancel_download();
        if !self.storage_ok {
            return Task::done(Message::Shutdown(Ok(())));
        }
        self.save().map(|message| match message {
            Message::Saved(result) => Message::Shutdown(result),
            _ => Message::Shutdown(Ok(())),
        })
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Loaded(result) => {
                self.loading = false;
                match result {
                    Ok((store, installs)) => {
                        self.store = store;
                        self.installs = installs;
                        self.storage_ok = true;
                    }
                    Err(e) => {
                        self.error = Some(e);
                        return Task::none();
                    }
                }
                // One modest startup check; no network polling or background updater.
                let keys: Vec<_> = self
                    .store
                    .sources
                    .iter()
                    .filter(|s| {
                        s.enabled
                            && s.last_success
                                .is_none_or(|t| chrono::Utc::now() - t > chrono::Duration::hours(6))
                    })
                    .map(|s| s.key.clone())
                    .collect();
                Task::batch(keys.iter().map(|k| self.refresh(k)))
            }
            Message::UrlChanged(s) => {
                self.input = s;
                Task::none()
            }
            Message::Add => {
                if self.add.is_some() || !self.storage_ok {
                    return Task::none();
                }
                if let Ok(plume_source::SourceInput::Canonical(url)) =
                    plume_source::source_input(&self.input)
                    && self.store.source(&plume_source::source_key(&url)).is_some()
                {
                    self.error = Some(t!("sources_duplicate").to_string());
                    return Task::none();
                }
                let Some(client) = self.client.clone() else {
                    return Task::none();
                };
                let request = Request {
                    id: self.next_id(),
                    cancel: CancellationToken::new(),
                };
                let id = request.id;
                let cancel = request.cancel.clone();
                let input = self.input.clone();
                self.add = Some(request);
                self.error = None;
                Task::perform(
                    async move { client.add(&input, &cancel).await.map_err(|e| e.to_string()) },
                    move |result| Message::AddDone(id, result),
                )
            }
            Message::AddDone(id, result) => {
                if self.add.as_ref().map(|r| r.id) != Some(id) {
                    return Task::none();
                }
                self.add = None;
                match result {
                    Ok(source) => {
                        self.selected_source = Some(source.key.clone());
                        self.updates_only = false;
                        self.detail = None;
                        if let Err(e) = self.store.add(source) {
                            self.error = Some(e.to_string());
                            return Task::none();
                        }
                        self.input.clear();
                        self.save()
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }
            Message::CancelAdd => {
                if let Some(r) = self.add.take() {
                    r.cancel.cancel();
                }
                Task::none()
            }
            Message::RefreshAll => {
                let keys: Vec<_> = self
                    .store
                    .sources
                    .iter()
                    .filter(|s| s.enabled)
                    .map(|s| s.key.clone())
                    .collect();
                Task::batch(keys.iter().map(|k| self.refresh(k)))
            }
            Message::Refresh(key) => self.refresh(&key),
            Message::Refreshed(key, generation, id, result) => {
                if self.requests.get(&key).map(|r| r.id) != Some(id)
                    || !self.store.accepts(&key, generation)
                {
                    return Task::none();
                }
                self.requests.remove(&key);
                if let Some(source) = self.store.sources.iter_mut().find(|s| s.key == key) {
                    match result {
                        Ok(fresh) => *source = fresh,
                        Err(e) => source.error = Some(e),
                    }
                }
                // A history index can refer to a different release after a feed refresh.
                if self.detail.as_ref().is_some_and(|(k, _)| k == &key) {
                    self.select_newest();
                }
                self.save()
            }
            Message::Toggle(key, enabled) => {
                self.cancel_source(&key);
                self.store.set_enabled(&key, enabled);
                let save = self.save();
                if enabled {
                    Task::batch([save, self.refresh(&key)])
                } else {
                    save
                }
            }
            Message::Remove(key) => {
                // An add may still be resolving an ambiguous blob URL that names this source.
                // Removing it must invalidate that completion as well as refresh/download work.
                if let Some(request) = self.add.take() {
                    request.cancel.cancel();
                    self.error = Some(t!("sources_add_cancelled").to_string());
                }
                self.cancel_source(&key);
                self.store.remove(&key);
                if self.selected_source.as_ref() == Some(&key) {
                    self.selected_source = None;
                }
                if self.detail.as_ref().is_some_and(|(k, _)| k == &key) {
                    self.detail = None;
                }
                self.save()
            }
            Message::SelectSource(key) => {
                self.selected_source = key;
                self.detail = None;
                self.updates_only = false;
                self.limit = 100;
                Task::none()
            }
            Message::ShowUpdates => {
                self.selected_source = None;
                self.detail = None;
                self.updates_only = true;
                self.limit = 100;
                Task::none()
            }
            Message::Search(q) => {
                self.search = q;
                self.limit = 100;
                Task::none()
            }
            Message::Scroll(y) => iced::widget::operation::scroll_by(
                "sources-body",
                iced::widget::operation::AbsoluteOffset { x: 0.0, y },
            ),
            Message::FocusUrl => iced::widget::operation::focus("source-url"),
            Message::FocusSearch => iced::widget::operation::focus("source-search"),
            Message::More => {
                self.limit += 100;
                Task::none()
            }
            Message::Details(key, bundle) => {
                self.retry_download = false;
                self.detail = Some((key, bundle));
                self.select_newest();
                self.load_icon()
            }
            Message::Back => {
                self.detail = None;
                Task::none()
            }
            Message::SelectRelease(i) => {
                self.retry_download = false;
                self.release = i;
                Task::none()
            }
            Message::Target(device, account) => {
                if self.device.as_ref().map(|d| &d.udid) == device.as_ref().map(|d| &d.udid)
                    && self.account == account
                {
                    return Task::none();
                }
                self.cancel_download();
                self.device = device;
                self.account = account;
                self.profile = None;
                self.reconcile_device()
            }
            Message::DeviceReady(generation, result) => {
                if generation != self.device_generation {
                    return Task::none();
                }
                match result {
                    Ok((profile, inventory)) => {
                        self.profile = Some(profile);
                        self.inventory = inventory.ok().map(|v| v.into_iter().collect());
                    }
                    Err(e) => self.device_error = Some(e),
                }
                self.select_newest();
                Task::none()
            }
            Message::Install => self.download(),
            Message::CancelDownload => {
                self.cancel_download();
                Task::none()
            }
            Message::DownloadTick => Task::none(),
            Message::DownloadDone(id, result) => {
                if self.download.as_ref().map(|d| d.request.id) != Some(id) {
                    if let Ok((package, _)) = result {
                        std::thread::spawn(move || package.remove_package_stage());
                    }
                    return Task::none();
                }
                self.download = None;
                match result {
                    Ok((package, context)) => Task::done(Message::OpenInstaller(package, context)),
                    Err(e) => {
                        self.retry_download = true;
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }
            Message::Icon(key, handle) => {
                self.icons.insert(key, handle);
                Task::none()
            }
            Message::Saved(result) => {
                if let Err(e) = result {
                    self.error = Some(e);
                }
                Task::none()
            }
            Message::ReloadInstalls => Task::perform(
                async {
                    tokio::task::spawn_blocking(|| {
                        InstallStore::load(&installs_path(&defaults::get_data_path()))
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .map_err(|e| e.to_string())?
                },
                Message::InstallsLoaded,
            ),
            Message::InstallsLoaded(result) => {
                match result {
                    Ok(store) => {
                        self.installs = store;
                        // The inventory collected before installation cannot establish whether
                        // the just-installed identity is now present, including partial failures.
                        self.reconcile_device()
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }
            Message::OpenInstaller(_, _) => Task::none(),
            Message::Shutdown(_) => Task::none(),
        }
    }

    fn detail_app(&self) -> Option<(&Source, &App)> {
        let (source, bundle) = self.detail.as_ref()?;
        let source = self.store.source(source)?;
        Some((
            source,
            source
                .catalog
                .apps
                .iter()
                .find(|a| &a.bundle_id == bundle)?,
        ))
    }
    fn select_newest(&mut self) {
        self.release = self
            .detail_app()
            .and_then(|(_, a)| a.newest_eligible(self.profile.as_ref()))
            .unwrap_or(0);
    }
    fn record(&self, source: &Source, app: &App) -> Option<&InstallRecord> {
        self.installs.find(
            &source.key,
            &app.bundle_id,
            &self.device.as_ref()?.udid,
            self.account.as_deref()?,
        )
    }
    fn change(&self, source: &Source, app: &App, index: usize) -> Option<ReleaseChange> {
        self.record(source, app).map(|r| {
            let mut installed = r.release.clone();
            installed.sha256 = Some(r.artifact_sha256.clone());
            release_change(app, index, &installed)
        })
    }
    fn update_available(&self, source: &Source, app: &App) -> bool {
        if self.device.is_none() || self.profile.is_none() {
            return false;
        }
        let Some(record) = self.record(source, app) else {
            return false;
        };
        if self
            .inventory
            .as_ref()
            .is_some_and(|ids| !ids.contains(&record.installed_bundle_id))
        {
            return false;
        }
        app.newest_eligible(self.profile.as_ref())
            .is_some_and(|i| self.change(source, app, i) == Some(ReleaseChange::Newer))
    }
    pub fn updates_count(&self) -> usize {
        self.store
            .sources
            .iter()
            .filter(|s| s.enabled)
            .map(|s| {
                s.catalog
                    .apps
                    .iter()
                    .filter(|a| self.update_available(s, a))
                    .count()
            })
            .sum()
    }

    fn load_icon(&mut self) -> Task<Message> {
        let Some((_, app)) = self.detail_app() else {
            return Task::none();
        };
        let Some(url) = app.icon_url.clone() else {
            return Task::none();
        };
        let key = url.to_string();
        if self.icons.contains_key(&key) {
            return Task::none();
        }
        self.icons.insert(key.clone(), None);
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let bytes = client.image(&url, &CancellationToken::new()).await.ok()?;
                tokio::task::spawn_blocking(move || {
                    let mut limits = image_crate_limits();
                    limits.max_image_width = Some(1024);
                    limits.max_image_height = Some(1024);
                    let mut reader = ::image::ImageReader::new(std::io::Cursor::new(bytes))
                        .with_guessed_format()
                        .ok()?;
                    reader.limits(limits);
                    let decoded = reader.decode().ok()?.thumbnail(128, 128).to_rgba8();
                    let (w, h) = decoded.dimensions();
                    Some(image::Handle::from_rgba(w, h, decoded.into_raw()))
                })
                .await
                .ok()
                .flatten()
            },
            move |handle| Message::Icon(key.clone(), handle),
        )
    }
    fn download(&mut self) -> Task<Message> {
        if self.download.is_some() {
            return Task::none();
        }
        let Some((source, app)) = self.detail_app() else {
            return Task::none();
        };
        if !source.enabled || app.unavailable.is_some() {
            return Task::none();
        }
        let Some(release) = app.releases.get(self.release).cloned() else {
            return Task::none();
        };
        let (Some(device), Some(profile), Some(client)) = (
            self.device.clone(),
            self.profile.clone(),
            self.client.clone(),
        ) else {
            self.error = Some(t!("sources_select_device").to_string());
            return Task::none();
        };
        let context = SourceInstall {
            source_key: source.key.clone(),
            source_name: source.catalog.name.clone(),
            app_name: app.name.clone(),
            original_bundle_id: app.bundle_id.clone(),
            release: release.clone(),
            artifact_sha256: String::new(),
            target_udid: device.udid,
            profile: profile.clone(),
            account_key: self.account.clone(),
            previous: self.record(source, app).cloned(),
        };
        let request = Request {
            id: self.next_id(),
            cancel: CancellationToken::new(),
        };
        let id = request.id;
        let cancel = request.cancel.clone();
        let progress = Arc::new(DownloadProgress::default());
        self.download = Some(Download {
            request,
            source: context.source_key.clone(),
            progress: progress.clone(),
        });
        self.error = None;
        let description = context.describe();
        self.retry_download = false;
        Task::perform(
            async move {
                async {
                    let artifact = client
                        .download(
                            &release,
                            &context.original_bundle_id,
                            &profile,
                            &defaults::get_data_path().join("source-downloads"),
                            &cancel,
                            &progress,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    let mut context = context;
                    context.artifact_sha256 = artifact.sha256.clone();
                    tokio::task::spawn_blocking(move || {
                        let package = Package::new_without_artwork(artifact.path().to_owned())
                            .map_err(|e| format!("IPA staging: {e}"))?;
                        if cancel.is_cancelled() {
                            package.remove_package_stage();
                            return Err("Download cancelled".into());
                        }
                        // Package::new copied the immutable original; signing uses only that private stage.
                        Ok((package, context))
                    })
                    .await
                    .map_err(|e| e.to_string())?
                }
                .await
                .map_err(|error: String| format!("{description}: {error}"))
            },
            move |r| Message::DownloadDone(id, r),
        )
    }
    pub fn subscription(&self) -> Subscription<Message> {
        if self.download.is_some() {
            iced::time::every(Duration::from_millis(150)).map(|_| Message::DownloadTick)
        } else {
            Subscription::none()
        }
    }
}

fn image_crate_limits() -> ::image::Limits {
    let mut limits = ::image::Limits::default();
    limits.max_alloc = Some(8 * 1024 * 1024);
    limits
}

pub fn selected_account_key(store: Option<&plume_store::AccountStore>) -> Option<String> {
    store
        .and_then(|s| s.selected_account())
        .map(|a| account_key(a.email(), a.team_id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> Source {
        let url = plume_source::Url::parse("https://source.test/feed").unwrap();
        let catalog = plume_source::parse_catalog(
            include_bytes!("../../../../crates/plume_source/tests/fixtures/youmod.json"),
            &url,
        )
        .unwrap();
        Source {
            key: plume_source::source_key(&url),
            url,
            enabled: true,
            catalog,
            etag: None,
            last_modified: None,
            last_success: Some(chrono::Utc::now()),
            error: None,
            generation: 0,
        }
    }
    #[test]
    fn refresh_failure_retains_catalog_and_remove_disable_ignore_inflight_completion() {
        let (mut screen, _) = SourcesScreen::new();
        let source = source();
        let key = source.key.clone();
        screen.store.add(source.clone()).unwrap();
        screen.requests.insert(
            key.clone(),
            Request {
                id: 5,
                cancel: CancellationToken::new(),
            },
        );
        let _ = screen.update(Message::Refreshed(key.clone(), 0, 5, Err("offline".into())));
        assert_eq!(screen.store.sources[0].catalog.name, "YouMod");
        assert_eq!(screen.store.sources[0].error.as_deref(), Some("offline"));
        screen.requests.insert(
            key.clone(),
            Request {
                id: 6,
                cancel: CancellationToken::new(),
            },
        );
        let _ = screen.update(Message::Toggle(key.clone(), false));
        let _ = screen.update(Message::Refreshed(key.clone(), 0, 6, Ok(source.clone())));
        assert!(!screen.store.sources[0].enabled);
        let add_cancel = CancellationToken::new();
        screen.add = Some(Request {
            id: 7,
            cancel: add_cancel.clone(),
        });
        let _ = screen.update(Message::Remove(key.clone()));
        assert!(add_cancel.is_cancelled());
        let _ = screen.update(Message::AddDone(7, Ok(source.clone())));
        let _ = screen.update(Message::Refreshed(key, 0, 6, Ok(source)));
        assert!(screen.store.sources.is_empty());
    }
    #[test]
    fn device_changes_discard_profile_completions_and_cancel_pending_handoffs() {
        let (mut screen, _) = SourcesScreen::new();
        screen.device_generation = 2;
        let cancel = CancellationToken::new();
        screen.download = Some(Download {
            request: Request {
                id: 1,
                cancel: cancel.clone(),
            },
            source: "source".into(),
            progress: Arc::new(DownloadProgress::default()),
        });
        screen.device = Some(Device {
            name: "Old".into(),
            udid: "old".into(),
            device_id: 1,
            usbmuxd_device: None,
            is_mac: false,
        });
        let _ = screen.update(Message::Target(None, None));
        assert!(cancel.is_cancelled());
        assert!(screen.download.is_none());
        let _ = screen.update(Message::DeviceReady(
            2,
            Ok((
                DeviceProfile {
                    os_version: "17".into(),
                    platform: "ios".into(),
                    family: 1,
                },
                Ok(vec![]),
            )),
        ));
        assert!(screen.profile.is_none());
        screen.device = Some(Device {
            name: "Current".into(),
            udid: "current".into(),
            device_id: 2,
            usbmuxd_device: None,
            is_mac: false,
        });
        screen.inventory = Some(HashSet::new());
        let _ = screen.update(Message::InstallsLoaded(Ok(InstallStore::default())));
        assert!(screen.inventory.is_none());
        assert_eq!(screen.device_generation, 4);
        let profile = DeviceProfile {
            os_version: "17".into(),
            platform: "ios".into(),
            family: 1,
        };
        let _ = screen.update(Message::DeviceReady(3, Ok((profile.clone(), Ok(vec![])))));
        assert!(screen.inventory.is_none());
        let _ = screen.update(Message::DeviceReady(
            4,
            Ok((profile, Ok(vec!["installed.identity".into()]))),
        ));
        assert!(
            screen
                .inventory
                .as_ref()
                .unwrap()
                .contains("installed.identity")
        );
    }
    #[test]
    fn provisioning_records_are_not_catalog_installs_and_update_checks_are_scoped() {
        let (mut screen, _) = SourcesScreen::new();
        let source = source();
        let app = &source.catalog.apps[0];
        let record = InstallRecord {
            source_key: source.key.clone(),
            original_bundle_id: app.bundle_id.clone(),
            release: app.releases[0].clone(),
            artifact_sha256: "hash".into(),
            installed_bundle_id: "signed.variant.TEAM".into(),
            device_udid: "device".into(),
            account_key: "account".into(),
            team_id: "TEAM".into(),
            settings: serde_json::json!({}),
            installed_at: chrono::Utc::now(),
        };
        screen.device = Some(Device {
            name: "Test".into(),
            udid: "device".into(),
            device_id: 1,
            usbmuxd_device: None,
            is_mac: false,
        });
        screen.account = Some("account".into());
        screen.profile = Some(DeviceProfile {
            os_version: "17".into(),
            platform: "ios".into(),
            family: 1,
        });
        screen.inventory = Some(HashSet::from([
            app.bundle_id.clone(),
            record.installed_bundle_id.clone(),
        ]));
        screen.store.add(source).unwrap();
        assert_eq!(screen.updates_count(), 0);
        screen.installs.record_success(record.clone());
        screen.store.sources[0].catalog.apps[0].releases[0].version = "2.0".into();
        assert_eq!(screen.updates_count(), 1);
        assert_eq!(
            screen.installs.installs[0].release.version,
            record.release.version
        );
        screen.account = Some("other".into());
        assert_eq!(screen.updates_count(), 0);
        screen.account = Some("account".into());
        screen.inventory = Some(HashSet::new());
        assert_eq!(screen.updates_count(), 0);
        screen.store.set_enabled(&record.source_key, false);
        assert_eq!(screen.installs.installs.len(), 1);
    }
}
