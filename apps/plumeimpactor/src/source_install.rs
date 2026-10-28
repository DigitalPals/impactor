//! The only bridge between public catalogs and the established Apple-account installer.
use plume_source::{
    DeviceProfile, InstallRecord, InstallStore, Release, account_key, installs_path,
};
use plume_store::GsaAccount;
use plume_utils::{Device, Package, SignerInstallMode, SignerMode, SignerOptions};

#[derive(Debug, Clone)]
pub struct SourceInstall {
    pub source_key: String,
    pub source_name: String,
    pub app_name: String,
    pub original_bundle_id: String,
    pub release: Release,
    pub artifact_sha256: String,
    pub target_udid: String,
    pub profile: DeviceProfile,
    pub account_key: Option<String>,
    pub previous: Option<InstallRecord>,
}

impl SourceInstall {
    pub fn scope_check(
        &self,
        device: Option<&Device>,
        account: Option<&GsaAccount>,
    ) -> Result<(), String> {
        if device.map(|d| d.udid.as_str()) != Some(self.target_udid.as_str()) {
            return Err("Target changed since download. Return to Sources and select this release for the intended device.".into());
        }
        let scope = account.map(|a| account_key(a.email(), a.team_id()));
        if self.account_key.is_some() && self.account_key != scope {
            return Err("Account/team changed since download. Return to Sources to select the intended account.".into());
        }
        Ok(())
    }

    pub fn options(&self, package: &Package) -> Result<SignerOptions, String> {
        if let Some(previous) = &self.previous {
            let mut options: SignerOptions = serde_json::from_value(previous.settings.clone())
                .map_err(|e| format!("Cannot restore this install's signing options: {e}"))?;
            options.custom_identifier = Some(previous.installed_bundle_id.clone());
            options.install_mode = SignerInstallMode::Install;
            Ok(options)
        } else {
            let mut options = SignerOptions::default();
            package.load_into_signer_options(&mut options);
            Ok(options)
        }
    }

    /// Runs after resolving the real Apple team and before modifying/signing any bytes.
    pub async fn prepare(
        &self,
        device: &Device,
        account: &GsaAccount,
        team_id: &str,
        options: &mut SignerOptions,
    ) -> Result<(), String> {
        self.scope_check(Some(device), Some(account))?;
        let (os_version, platform, family) = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            device.source_device_info(),
        )
        .await
        .map_err(|_| "Device compatibility request timed out".to_string())?
        .map_err(|e| format!("Device compatibility: {e}"))?;
        let actual = DeviceProfile {
            os_version,
            platform,
            family,
        };
        if actual != self.profile {
            return Err("Device runtime changed since IPA validation. Download again to validate it for this device.".into());
        }
        self.prepare_identity(team_id, options)?;
        if let Some(previous) = &self.previous
            && !device.is_mac
            && !device
                .is_app_installed(&previous.installed_bundle_id)
                .await
                .map_err(|e| format!("Cannot confirm update target: {e}"))?
        {
            return Err(format!(
                "Tracked app {} is absent from this device. No in-place update can be established; the previous install record was retained.",
                previous.installed_bundle_id
            ));
        }
        let identity = options
            .custom_identifier
            .as_deref()
            .ok_or("Missing source install identity")?;
        if !plume_source::valid_bundle_id(identity) {
            return Err(format!("Invalid installed bundle identifier: {identity}"));
        }
        let store = InstallStore::load(&installs_path(&crate::defaults::get_data_path()))
            .map_err(|e| e.to_string())?;
        if store.identity_conflict(
            &self.source_key,
            &self.original_bundle_id,
            &device.udid,
            &account_key(account.email(), account.team_id()),
            identity,
        ) {
            return Err(format!(
                "Identity conflict: {identity} is tracked for another source/account. Choose a distinct identifier; the existing app was left intact."
            ));
        }
        if self.previous.is_none()
            && !device.is_mac
            && device
                .is_app_installed(identity)
                .await
                .map_err(|e| format!("Cannot check installed identity: {e}"))?
        {
            return Err(format!(
                "Identity conflict: {identity} is already installed without matching provenance. Choose another identifier."
            ));
        }
        for path in options
            .tweaks
            .iter()
            .flatten()
            .chain(options.custom_icon.iter())
            .chain(options.custom_entitlements.iter())
        {
            if !path.exists() {
                return Err(format!(
                    "Saved installer asset is missing: {}. Restore it or adjust the installer options.",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    fn prepare_identity(&self, team_id: &str, options: &mut SignerOptions) -> Result<(), String> {
        if let Some(previous) = &self.previous {
            if previous.team_id != team_id
                || options.mode != SignerMode::Pem
                || options.install_mode != SignerInstallMode::Install
                || options.custom_identifier.as_deref() != Some(&previous.installed_bundle_id)
            {
                return Err("Update identity conflict: keep the original Apple team, Apple ID signing, Install mode, and installed bundle identifier. No app was uninstalled.".into());
            }
        } else if options.custom_identifier.is_none() {
            options.custom_identifier = Some(format!(
                "{}.s{}.{}",
                self.original_bundle_id,
                self.source_key
                    .get(..8)
                    .ok_or("Invalid canonical source key")?,
                team_id
            ));
        }
        Ok(())
    }

    /// Invoked immediately after confirmed device success, independently of provisioning refresh.
    pub fn record_success(
        &self,
        device: &Device,
        account: &GsaAccount,
        team_id: &str,
        installed_id: String,
        options: &SignerOptions,
    ) -> Result<(), String> {
        let path = installs_path(&crate::defaults::get_data_path());
        self.record_success_at(&path, device, account, team_id, installed_id, options)
    }
    fn record_success_at(
        &self,
        path: &std::path::Path,
        device: &Device,
        account: &GsaAccount,
        team_id: &str,
        installed_id: String,
        options: &SignerOptions,
    ) -> Result<(), String> {
        let mut store = InstallStore::load(path).map_err(|e| e.to_string())?;
        store.record_success(InstallRecord {
            source_key: self.source_key.clone(),
            original_bundle_id: self.original_bundle_id.clone(),
            release: self.release.clone(),
            artifact_sha256: self.artifact_sha256.clone(),
            installed_bundle_id: installed_id,
            device_udid: device.udid.clone(),
            account_key: account_key(account.email(), account.team_id()),
            team_id: team_id.into(),
            settings: serde_json::to_value(options).map_err(|e| e.to_string())?,
            installed_at: chrono::Utc::now(),
        });
        store.save(path).map_err(|e| {
            format!("App installed successfully, but source provenance could not be saved: {e}")
        })
    }
    pub fn describe(&self) -> String {
        format!(
            "{} · {} {}",
            self.source_name, self.app_name, self.release.version
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plume_utils::PlistInfoTrait;
    use std::io::Write;

    fn device() -> Device {
        Device {
            name: "Test".into(),
            udid: "device-one".into(),
            device_id: 1,
            usbmuxd_device: None,
            is_mac: false,
        }
    }
    fn account() -> GsaAccount {
        GsaAccount::new(
            "test@example.test".into(),
            "Name".into(),
            "unused-adsid".into(),
            "secret-never-sent".into(),
            "TEAM".into(),
        )
    }
    fn context() -> SourceInstall {
        let url = plume_source::Url::parse("https://catalog.test/feed").unwrap();
        let catalog=plume_source::parse_catalog(br#"{"name":"Catalog","apps":[{"name":"App","bundleIdentifier":"test.app","versions":[{"version":"1.0","buildVersion":"2","downloadURL":"/ipa"}]}]}"#,&url).unwrap();
        SourceInstall {
            source_key: plume_source::source_key(&url),
            source_name: "Catalog".into(),
            app_name: "App".into(),
            original_bundle_id: "test.app".into(),
            release: catalog.apps[0].releases[0].clone(),
            artifact_sha256: "checksum".into(),
            target_udid: "device-one".into(),
            profile: DeviceProfile {
                os_version: "17".into(),
                platform: "ios".into(),
                family: 1,
            },
            account_key: Some(account_key(account().email(), "TEAM")),
            previous: None,
        }
    }
    fn package() -> (tempfile::TempDir, Package) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.ipa");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file(
            "Payload/Test.app/Info.plist",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(br#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>test.app</string><key>CFBundleShortVersionString</key><string>1.0</string><key>CFBundleVersion</key><string>2</string><key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array></dict></plist>"#).unwrap();
        zip.finish().unwrap();
        let package = Package::new(path).unwrap();
        (dir, package)
    }
    #[test]
    fn validated_ipa_handoff_uses_existing_package_stage_and_preserves_original() {
        let (dir, package) = package();
        let original = dir.path().join("original.ipa");
        let metadata =
            plume_source::inspect_ipa(&original, &plume_source::CancellationToken::new()).unwrap();
        let context = context();
        plume_source::validate_metadata(
            &metadata,
            &context.release,
            &context.original_bundle_id,
            &context.profile,
        )
        .unwrap();
        assert_eq!(package.get_bundle_identifier().as_deref(), Some("test.app"));
        assert_ne!(package.package_file(), &original);
        let options = context.options(&package).unwrap();
        assert_eq!(options.mode, SignerMode::Pem);
        std::fs::write(package.package_file(), b"simulate signing staging bytes").unwrap();
        assert!(std::fs::read(original).unwrap().starts_with(b"PK"));
        package.remove_package_stage();
    }
    #[test]
    fn target_account_and_team_changes_are_rejected() {
        let context = context();
        let mut device = device();
        assert!(context.scope_check(Some(&device), Some(&account())).is_ok());
        device.udid = "different-device".into();
        assert!(
            context
                .scope_check(Some(&device), Some(&account()))
                .is_err()
        );
        assert!(
            context
                .scope_check(Some(&super::tests::device()), None)
                .is_err()
        );
        let other = GsaAccount::new(
            "other@example.test".into(),
            "Other".into(),
            "adsid".into(),
            "token".into(),
            "TEAM".into(),
        );
        assert!(
            context
                .scope_check(Some(&super::tests::device()), Some(&other))
                .is_err()
        );
    }
    #[test]
    fn success_records_exact_identity_and_update_restores_options_without_second_copy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source-installs.json");
        let mut context = context();
        let (stage, package) = package();
        let mut options = context.options(&package).unwrap();
        options.refresh = true;
        options.features.support_file_sharing = true;
        context.prepare_identity("TEAM", &mut options).unwrap();
        let identity = options.custom_identifier.clone().unwrap();
        assert!(identity.starts_with("test.app.s"));
        context
            .record_success_at(
                &path,
                &device(),
                &account(),
                "TEAM",
                identity.clone(),
                &options,
            )
            .unwrap();
        let record = InstallStore::load(&path).unwrap().installs.remove(0);
        assert_eq!(record.installed_bundle_id, identity);
        assert_eq!(record.original_bundle_id, "test.app");
        assert_eq!(record.artifact_sha256, "checksum");
        context.previous = Some(record);
        let mut restored = context.options(&package).unwrap();
        assert!(restored.refresh);
        assert!(restored.features.support_file_sharing);
        assert_eq!(
            restored.custom_identifier.as_deref(),
            Some(identity.as_str())
        );
        context.prepare_identity("TEAM", &mut restored).unwrap();
        assert_eq!(
            restored.custom_identifier.as_deref(),
            Some(identity.as_str())
        );
        assert!(
            context
                .prepare_identity("OTHER_TEAM", &mut restored)
                .is_err()
        );
        restored.custom_identifier = Some("test.app.secondcopy".into());
        assert!(context.prepare_identity("TEAM", &mut restored).is_err());
        assert!(
            !std::fs::read_to_string(path)
                .unwrap()
                .contains("secret-never-sent")
        );
        drop(stage);
        package.remove_package_stage();
    }
    #[tokio::test]
    async fn failed_install_and_failed_update_do_not_record_or_replace_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source-installs.json");
        let mut context = context();
        let (_stage, package) = package();
        let options = context.options(&package).unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let result = crate::subscriptions::run_installation(
            &package,
            Some(&device()),
            &options,
            None,
            None,
            &tx,
            Some(&context),
        )
        .await;
        assert!(result.is_err());
        assert!(!path.exists());
        context
            .record_success_at(
                &path,
                &device(),
                &account(),
                "TEAM",
                "test.app.existing".into(),
                &options,
            )
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        context.previous = InstallStore::load(&path).unwrap().installs.pop();
        context.release.version = "2.0".into();
        let result = crate::subscriptions::run_installation(
            &package,
            Some(&device()),
            &options,
            None,
            None,
            &tx,
            Some(&context),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(std::fs::read(path).unwrap(), before);
        package.remove_package_stage();
    }
}
