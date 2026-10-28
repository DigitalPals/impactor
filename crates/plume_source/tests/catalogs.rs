use plume_source::*;
use serde_json::json;

const BLOB: &str =
    "https://github.com/mrdrvt99/Altstore-Repository/blob/main/youmod.json?utm_source=chatgpt.com";
const RAW: &str = "https://raw.githubusercontent.com/mrdrvt99/Altstore-Repository/main/youmod.json";
fn base() -> Url {
    Url::parse("https://example.test/catalog/feed.json?catalog=public").unwrap()
}
fn catalog(value: serde_json::Value) -> Catalog {
    parse_catalog(&serde_json::to_vec(&value).unwrap(), &base()).unwrap()
}
fn app(releases: serde_json::Value) -> App {
    catalog(json!({"name":"Test","apps":[{"name":"App","bundleIdentifier":"test.app","versions":releases}]})).apps.remove(0)
}
fn release(version: &str, build: Option<&str>) -> Release {
    app(json!([{"version":version,"buildVersion":build,"downloadURL":"/artifact"}]))
        .releases
        .remove(0)
}

#[test]
fn exact_github_input_deduplicates_with_raw_and_tracking() {
    let SourceInput::Canonical(blob) = source_input(BLOB).unwrap() else {
        panic!()
    };
    let SourceInput::Canonical(raw) = source_input(RAW).unwrap() else {
        panic!()
    };
    assert_eq!(blob, raw);
    assert_eq!(source_key(&blob), source_key(&raw));
    assert_eq!(
        source_input(&format!("{RAW}?utm_source=tracker&raw=true#foo")).unwrap(),
        SourceInput::Canonical(raw)
    );
}
#[test]
fn preserves_meaningful_query_parameters_and_rejects_credentials_and_schemes() {
    let url = "https://catalog.example.test/list?token=signed%2Bvalue&utm_source=meaningful";
    assert_eq!(
        source_input(url).unwrap(),
        SourceInput::Canonical(Url::parse(url).unwrap())
    );
    let raw = format!("{RAW}?token=signed%2Bvalue&utm_source=tracker");
    assert!(
        matches!(source_input(&raw).unwrap(),SourceInput::Canonical(u) if u.query()==Some("token=signed%2Bvalue"))
    );
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://user:pass@example.test/feed",
        "http://example.test/feed",
    ] {
        assert!(source_input(url).is_err());
    }
    assert!(network_url("https://cdn.example.test/artifact?signature=keep-me").is_ok());
}
#[test]
fn encoded_paths_and_ambiguous_refs_are_not_guessed() {
    assert_eq!(
        source_input("https://github.com/org/repo/blob/feature%2Fbranch/a%20b.json?utm_source=x")
            .unwrap(),
        SourceInput::Canonical(
            Url::parse("https://raw.githubusercontent.com/org/repo/feature%2Fbranch/a%20b.json")
                .unwrap()
        )
    );
    assert!(matches!(
        source_input("https://github.com/org/repo/blob/feature/branch/a.json").unwrap(),
        SourceInput::Github { .. }
    ));
    assert!(matches!(
        source_input("https://github.com/org/repo/blob/main/catalog/a.json").unwrap(),
        SourceInput::Github { .. }
    ));
}
#[test]
fn pinned_publisher_feeds_prefer_modern_records_and_remain_separate() {
    let you = parse_catalog(
        include_bytes!("fixtures/youmod.json"),
        &Url::parse(RAW).unwrap(),
    )
    .unwrap();
    let music = parse_catalog(
        include_bytes!("fixtures/ytmusic.json"),
        &Url::parse(RAW).unwrap(),
    )
    .unwrap();
    assert_eq!(you.name, "YouMod");
    assert_eq!(music.name, "YTMusicUltimate");
    for source in [&you, &music] {
        assert_eq!(source.apps.len(), 1);
        assert_eq!(source.apps[0].releases[0].version, "1.3.0");
        assert_eq!(source.apps[0].releases[0].min_os.as_deref(), Some("17.0"));
    }
    assert_eq!(you.identifier, music.identifier); // Publisher collisions are metadata, not store keys.
    assert_ne!(
        source_key(&Url::parse(RAW).unwrap()),
        source_key(&Url::parse(&RAW.replace("youmod", "ytmusic")).unwrap())
    );
}
#[test]
fn json5_legacy_fallback_optional_forms_and_diagnostics() {
    let input=br#"{name:'JSON5',apps:[{name:'App',bundleIdentifier:'test.app',version:'legacy',downloadURL:'../download?signature=yes',
      versions:[{version:'broken'}],screenshots:{iphone:['one.png',{imageURL:'two.png',width:300}],ipad:[{imageURL:'ipad.png'}]},
      iconURL:'javascript:bad',appPermissions:{future:true},apiVersion:'whatever'}, {name:'Broken'},],news:[],}"#;
    let catalog = parse_catalog(input, &base()).unwrap();
    let app = &catalog.apps[0];
    assert_eq!(app.releases[0].version, "legacy");
    assert_eq!(
        app.releases[0].download_url.as_str(),
        "https://example.test/download?signature=yes"
    );
    assert_eq!(app.screenshots.len(), 3);
    assert!(app.icon_url.is_none());
    assert!(catalog.diagnostics.len() >= 3);
    assert!(app.description.is_none());
}
#[test]
fn validates_required_records_and_urls_but_allows_empty_and_unavailable_catalogs() {
    for invalid in [
        br#"<html>login</html>"#.as_slice(),
        br#"{"error":"API limit"}"#,
        br#"{"name":"bad","apps":[{}]}"#,
    ] {
        assert!(parse_catalog(invalid, &base()).is_err());
    }
    let empty = catalog(json!({"name":"Empty","apps":[]}));
    assert!(empty.apps.is_empty());
    assert!(!empty.diagnostics.is_empty());
    let unavailable = catalog(
        json!({"name":"Test","apps":[{"name":"PAL","bundleIdentifier":"test.pal","marketplaceID":"123"}]}),
    );
    assert!(
        unavailable.apps[0]
            .unavailable
            .as_deref()
            .unwrap()
            .contains("PAL")
    );
    let app = app(
        json!([{"version":"1","downloadURL":"file:///tmp/bad"},{"version":"2","downloadURL":"/good","sha256":"bad"},{"version":"3","downloadURL":"/good"}]),
    );
    assert_eq!(app.releases.len(), 1);
    assert_eq!(app.releases[0].version, "3");
}
#[test]
fn bundle_collisions_only_within_the_same_catalog_are_rejected() {
    let item =
        json!({"name":"App","bundleIdentifier":"test.app","version":"1","downloadURL":"/ipa"});
    let first = catalog(json!({"name":"A","apps":[item.clone(),item.clone()]}));
    let second = catalog(json!({"name":"B","apps":[item]}));
    assert_eq!(first.apps.len(), 1);
    assert_eq!(second.apps.len(), 1);
    assert!(first.diagnostics.iter().any(|d| d.contains("duplicate")));
}
#[test]
fn publisher_order_and_eligible_versions_are_deterministic() {
    let app = app(json!([
        {"version":"release-c","downloadURL":"/c","minOSVersion":"18"},
        {"version":"release-b","downloadURL":"/b","minOSVersion":"16","maxOSVersion":"17.9"},
        {"version":"release-a","downloadURL":"/a","platform":"tvos"}
    ]));
    assert_eq!(app.newest_eligible(None), Some(0));
    let d = DeviceProfile {
        os_version: "17.0.1".into(),
        platform: "ios".into(),
        family: 1,
    };
    assert_eq!(app.newest_eligible(Some(&d)), Some(1));
    assert_eq!(
        release_change(&app, 0, &app.releases[1]),
        ReleaseChange::Newer
    );
    assert_eq!(
        release_change(&app, 1, &app.releases[0]),
        ReleaseChange::Older
    );
    assert_eq!(
        compare_numeric("1.10", "1.9"),
        Some(std::cmp::Ordering::Greater)
    );
    assert_eq!(
        compare_numeric("1.0", "1.0.0"),
        Some(std::cmp::Ordering::Equal)
    );
}
#[test]
fn avoids_false_updates_downgrades_and_endless_artifact_changes() {
    let mut app = app(
        json!([{"version":"1.10","buildVersion":"10","downloadURL":"/ipa","date":"2025-01-01"}]),
    );
    let installed = app.releases[0].clone();
    app.releases[0].date = Some("2030-01-01".into());
    app.releases[0].notes = Some("changed notes".into());
    assert_eq!(release_change(&app, 0, &installed), ReleaseChange::Same);
    app.releases[0].download_url = Url::parse("https://example.test/other").unwrap();
    assert_eq!(
        release_change(&app, 0, &installed),
        ReleaseChange::Ambiguous
    );
    assert_eq!(
        release_change(&app, 0, &release("1.9", Some("999"))),
        ReleaseChange::Newer
    );
    assert_eq!(
        release_change(&app, 0, &release("2.0", Some("1"))),
        ReleaseChange::Older
    );
    assert_eq!(
        release_change(&app, 0, &release("1.10", Some("9"))),
        ReleaseChange::Newer
    );
    assert_eq!(
        release_change(&app, 0, &release("1.10", Some("11"))),
        ReleaseChange::Older
    );
    assert_eq!(
        release_change(&app, 0, &release("uncomparable", None)),
        ReleaseChange::Ambiguous
    );
}
#[test]
fn searches_names_developers_and_bundle_ids_without_account_or_device() {
    let catalog = parse_catalog(include_bytes!("fixtures/ytmusic.json"), &base()).unwrap();
    let app = &catalog.apps[0];
    for q in ["ytmusic", "DAYANCH96", "com.google.ios"] {
        assert!(app.matches(q));
    }
    assert!(!app.matches("unrelated"));
}
