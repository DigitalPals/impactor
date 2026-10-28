use plume_source::*;
use serde_json::json;

fn source(url: &str) -> Source {
    let url = Url::parse(url).unwrap();
    Source{key:source_key(&url),url: url.clone(),enabled:true,catalog:parse_catalog(br#"{"name":"Catalog","identifier":"collision","sourceURL":"https://evil.test/replace","apps":[]}"#,&url).unwrap(),etag:Some("etag".into()),last_modified:None,last_success:Some(chrono::Utc::now()),error:None,generation:0}
}
#[test]
fn restart_atomic_persistence_metadata_collisions_and_removal_do_not_affect_installs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sources.json");
    let installed = dir.path().join("source-installs.json");
    let mut store = SourceStore::default();
    store.add(source("https://a.test/feed")).unwrap();
    store.add(source("https://b.test/feed")).unwrap();
    assert!(store.add(source("https://a.test/feed")).is_err());
    store.save(&path).unwrap();
    let reloaded = SourceStore::load(&path).unwrap();
    assert_eq!(reloaded.sources.len(), 2);
    assert_eq!(reloaded.sources[0].url.as_str(), "https://a.test/feed");
    let installs = InstallStore::default();
    installs.save(&installed).unwrap();
    let before = std::fs::read(&installed).unwrap();
    let key = store.sources[0].key.clone();
    store.set_enabled(&key, false);
    store.remove(&key);
    store.save(&path).unwrap();
    assert_eq!(std::fs::read(&installed).unwrap(), before);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}
#[test]
fn defaults_load_older_stores_and_reject_future_or_corrupt_without_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sources.json");
    assert!(SourceStore::load(&path).unwrap().sources.is_empty());
    std::fs::write(&path, "{}").unwrap();
    assert!(SourceStore::load(&path).unwrap().sources.is_empty());
    std::fs::write(&path, r#"{"schema":999,"sources":[]}"#).unwrap();
    assert!(SourceStore::load(&path).is_err());
    std::fs::write(&path, "corrupt").unwrap();
    assert!(SourceStore::load(&path).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "corrupt");
}
#[test]
fn stale_completions_cannot_resurrect_removed_or_disabled_sources() {
    let mut store = SourceStore::default();
    let s = source("https://a.test/feed");
    let key = s.key.clone();
    let generation = s.generation;
    store.add(s).unwrap();
    assert!(store.accepts(&key, generation));
    store.set_enabled(&key, false);
    assert!(!store.accepts(&key, generation));
    store.set_enabled(&key, true);
    assert!(!store.accepts(&key, generation));
    store.remove(&key);
    assert!(!store.accepts(&key, generation));
}
#[test]
fn records_are_scoped_to_source_device_account_and_signed_identity() {
    let mut store = InstallStore::default();
    let catalog=parse_catalog(br#"{"name":"A","apps":[{"name":"App","bundleIdentifier":"test.app","version":"1","downloadURL":"https://a.test/ipa"}]}"#,&Url::parse("https://a.test/feed").unwrap()).unwrap();
    let record = InstallRecord {
        source_key: "a".into(),
        original_bundle_id: "test.app".into(),
        release: catalog.apps[0].releases[0].clone(),
        artifact_sha256: "abc".into(),
        installed_bundle_id: "test.app.source.team".into(),
        device_udid: "device1".into(),
        account_key: account_key("a@example.test", "team"),
        team_id: "team".into(),
        settings: json!({}),
        installed_at: chrono::Utc::now(),
    };
    store.record_success(record.clone());
    assert_eq!(store.installs.len(), 1);
    assert!(
        store
            .find("a", "test.app", "device1", &record.account_key)
            .is_some()
    );
    assert!(
        store
            .find("b", "test.app", "device1", &record.account_key)
            .is_none()
    );
    assert!(
        store
            .find("a", "test.app", "device2", &record.account_key)
            .is_none()
    );
    assert!(store.identity_conflict(
        "b",
        "test.app",
        "device1",
        &record.account_key,
        &record.installed_bundle_id
    ));
    let mut second = record.clone();
    second.device_udid = "device2".into();
    store.record_success(second);
    assert_eq!(store.installs.len(), 2);
    let mut updated = record.clone();
    updated.release.version = "2".into();
    store.record_success(updated);
    assert_eq!(store.installs.len(), 2);
    assert_eq!(
        store
            .find("a", "test.app", "device1", &record.account_key)
            .unwrap()
            .release
            .version,
        "2"
    );
    let bytes = serde_json::to_string(&store).unwrap();
    assert!(!bytes.contains("a@example.test"));
    assert!(!bytes.contains("xcode_gs_token"));
}
