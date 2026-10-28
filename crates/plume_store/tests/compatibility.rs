use plume_store::AccountStore;
#[test]
fn old_accounts_and_refresh_records_survive_atomic_saves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("accounts.json");
    std::fs::write(&path,r#"{"selected_account":null,"accounts":{},"refreshes":{"device":{"udid":"device","name":"Phone","account":"existing@example.test","is_mac":false,"apps":[{"path":"/existing/refresh.app","scheduled_refresh":"2025-01-01T00:00:00Z"}]}}}"#).unwrap();
    let mut store = AccountStore::load_sync(&Some(path.clone())).unwrap();
    assert!(store.refreshes()["device"].apps[0].bundle_id.is_none());
    store.set_locale_sync(Some("en".into())).unwrap();
    let restored = AccountStore::load_sync(&Some(path)).unwrap();
    assert_eq!(
        restored.refreshes()["device"].apps[0].path.to_str(),
        Some("/existing/refresh.app")
    );
    assert_eq!(restored.locale(), Some("en"));
    let device = &restored.refreshes()["device"];
    assert_eq!(device.apps[0].account_for(device), "existing@example.test");
    let mut other = device.apps[0].clone();
    other.account_email = Some("second@example.test".into());
    other.team_id = Some("ORIGINAL_TEAM".into());
    assert_eq!(other.account_for(device), "second@example.test");
    let bytes = serde_json::to_vec(&other).unwrap();
    let roundtrip: plume_store::RefreshApp = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(roundtrip.team_id.as_deref(), Some("ORIGINAL_TEAM"));
}
