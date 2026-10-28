use plume_source::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io::{Cursor, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[derive(Clone)]
struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    delay: Duration,
    declared: Option<usize>,
    body_delay: bool,
}
impl Response {
    fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: body.into(),
            delay: Duration::ZERO,
            declared: None,
            body_delay: false,
        }
    }
    fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::ok(vec![])
        }
    }
    fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    fn delay(mut self) -> Self {
        self.delay = Duration::from_secs(2);
        self
    }
}
struct Mock {
    url: Url,
    requests: Arc<Mutex<Vec<String>>>,
    cancel: CancellationToken,
}
impl Mock {
    async fn new(responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
        let requests = Arc::new(Mutex::new(vec![]));
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let captured = requests.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {_=token.cancelled()=>break,r=listener.accept()=>r};
                let Ok((mut socket, _)) = accepted else {
                    break;
                };
                let response = responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| Response::status(500));
                let requests = captured.clone();
                let stop = token.clone();
                tokio::spawn(async move {
                    let mut bytes = vec![];
                    let mut buf = [0u8; 4096];
                    while !bytes.windows(4).any(|s| s == b"\r\n\r\n") {
                        let n = tokio::select! {_=stop.cancelled()=>return,r=socket.read(&mut buf)=>r.unwrap_or(0)};
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..n]);
                    }
                    requests
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&bytes).into_owned());
                    tokio::select! {_=stop.cancelled()=>return,_=tokio::time::sleep(response.delay)=>{}}
                    let mut head = format!(
                        "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                        response.status,
                        response.declared.unwrap_or(response.body.len())
                    );
                    for (k, v) in response.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = socket.write_all(head.as_bytes()).await;
                    if response.body_delay {
                        let split = response.body.len().min(64);
                        let _ = socket.write_all(&response.body[..split]).await;
                        tokio::select! {_=stop.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
                        let _ = socket.write_all(&response.body[split..]).await;
                        let _ = socket.shutdown().await;
                        return;
                    }
                    let _ = socket.write_all(&response.body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            url: Url::parse(&format!("http://{addr}/feed")).unwrap(),
            requests,
            cancel,
        }
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
fn feed() -> Vec<u8> {
    serde_json::to_vec(&json!({"name":"Mock","identifier":"collision","sourceURL":"https://evil.test/identity","apps":[{"name":"App","bundleIdentifier":"test.app","versions":[{"version":"1.0","buildVersion":"2","downloadURL":"/artifact","minOSVersion":"16"}]}]})).unwrap()
}
fn device() -> DeviceProfile {
    DeviceProfile {
        os_version: "17.0".into(),
        platform: "ios".into(),
        family: 1,
    }
}
fn ipa_with(bundle: &str, platform: &str, os: &str) -> Vec<u8> {
    let mut output = zip::ZipWriter::new(Cursor::new(vec![]));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    output
        .start_file("Payload/Example.app/Info.plist", options)
        .unwrap();
    write!(output,r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{bundle}</string><key>CFBundleShortVersionString</key><string>1.0</string><key>CFBundleVersion</key><string>2</string><key>MinimumOSVersion</key><string>{os}</string><key>CFBundleSupportedPlatforms</key><array><string>{platform}</string></array><key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array></dict></plist>"#).unwrap();
    output
        .start_file("Payload/Example.app/Example", options)
        .unwrap();
    output
        .write_all(b"synthetic payload - no third party binary")
        .unwrap();
    output.finish().unwrap().into_inner()
}
async fn download(
    mock: &Mock,
    mutator: impl FnOnce(&mut Release),
) -> Result<(ValidatedArtifact, tempfile::TempDir)> {
    let client = SourceClient::new().unwrap();
    let s = client
        .add(mock.url.as_str(), &CancellationToken::new())
        .await
        .unwrap();
    let mut r = s.catalog.apps[0].releases[0].clone();
    mutator(&mut r);
    let dir = tempfile::tempdir().unwrap();
    let result = client
        .download(
            &r,
            "test.app",
            &device(),
            dir.path(),
            &CancellationToken::new(),
            &DownloadProgress::default(),
        )
        .await;
    if result.is_err() {
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    result.map(|artifact| (artifact, dir))
}
#[tokio::test]
async fn fetch_redirect_conditionals_cached_failure_and_no_credentials() {
    let mock = Mock::new(vec![
        Response::status(302).header("Location", "/actual"),
        Response::ok(feed())
            .header("ETag", "\"v1\"")
            .header("Last-Modified", "Wed, 01 Jan 2025 00:00:00 GMT"),
        Response::status(304),
        Response::status(503),
    ])
    .await;
    let client = SourceClient::new().unwrap();
    let stop = CancellationToken::new();
    let first = client.add(mock.url.as_str(), &stop).await.unwrap();
    assert_eq!(first.url, mock.url);
    assert_eq!(
        first.catalog.apps[0].releases[0].download_url.path(),
        "/artifact"
    );
    let second = client.refresh(&first, &stop).await.unwrap();
    assert_eq!(second.catalog.name, first.catalog.name);
    assert!(second.last_success >= first.last_success);
    assert!(
        client
            .refresh(&second, &stop)
            .await
            .unwrap_err()
            .to_string()
            .contains("503")
    );
    assert_eq!(second.catalog.apps.len(), 1);
    let requests = mock.requests.lock().unwrap();
    assert!(requests[2].to_lowercase().contains("if-none-match: \"v1\""));
    assert!(requests[2].to_lowercase().contains("if-modified-since:"));
    for request in requests.iter() {
        let lower = request.to_lowercase();
        for forbidden in ["authorization:", "cookie:", "x-apple", "xcode", "password"] {
            assert!(!lower.contains(forbidden), "{request}");
        }
    }
}
#[tokio::test]
async fn rejects_http_errors_html_json_masquerades_and_oversized_feed() {
    for response in [
        Response::status(404),
        Response::ok(b"<html>Login</html>".to_vec()),
        Response::ok(feed()).header("Content-Type", "text/html"),
        Response::ok(b"rate limit".to_vec()),
        Response::ok(feed()).header("Content-Length-Ignore", "unused"),
    ] {
        // The final ordinary response is accepted even without a JSON MIME type.
        let expected_ok = response
            .headers
            .iter()
            .any(|(k, _)| k == "Content-Length-Ignore");
        let mock = Mock::new(vec![response]).await;
        assert_eq!(
            SourceClient::new()
                .unwrap()
                .add(mock.url.as_str(), &CancellationToken::new())
                .await
                .is_ok(),
            expected_ok
        );
    }
    let mut oversized = Response::ok(feed());
    oversized.declared = Some(MAX_FEED_BYTES as usize + 1);
    let mock = Mock::new(vec![oversized]).await;
    assert!(
        SourceClient::new()
            .unwrap()
            .add(mock.url.as_str(), &CancellationToken::new())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn fetch_timeout_and_cancellation() {
    let mock = Mock::new(vec![
        Response::ok(feed()).delay(),
        Response::ok(feed()).delay(),
    ])
    .await;
    let client = SourceClient::with_timeout(Duration::from_millis(30)).unwrap();
    assert!(
        client
            .add(mock.url.as_str(), &CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .to_lowercase()
            .contains("tim")
    );
    let stop = CancellationToken::new();
    let cloned = stop.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        cloned.cancel();
    });
    assert!(
        client
            .add(mock.url.as_str(), &stop)
            .await
            .unwrap_err()
            .to_string()
            .contains("Cancelled")
    );
}
#[tokio::test]
async fn successful_octet_stream_suffixless_download_checks_hash_metadata_and_cleanup() {
    let ipa = ipa_with("test.app", "iPhoneOS", "16.0");
    let sha = hex::encode(Sha256::digest(&ipa));
    let size = ipa.len() as u64;
    let mock = Mock::new(vec![
        Response::ok(feed()),
        Response::ok(ipa).header("Content-Type", "application/octet-stream"),
    ])
    .await;
    let (artifact, _directory) = download(&mock, |r| {
        r.sha256 = Some(sha.clone());
        r.size = Some(size);
    })
    .await
    .unwrap();
    assert_eq!(artifact.sha256, sha);
    assert_eq!(artifact.metadata.bundle_id, "test.app");
    let path = artifact.path().to_owned();
    assert!(path.exists());
    drop(artifact);
    assert!(!path.exists());
}
#[tokio::test]
async fn github_style_release_redirect_works_without_ipa_suffix() {
    let mock = Mock::new(vec![
        Response::ok(feed()),
        Response::status(302).header("Location", "/signed?signature=retain"),
        Response::ok(ipa_with("test.app", "iPhoneOS", "16")),
    ])
    .await;
    assert!(download(&mock, |_| {}).await.is_ok());
    assert!(mock.requests.lock().unwrap()[2].starts_with("GET /signed?signature=retain "));
}
#[tokio::test]
async fn checksum_bundle_version_build_platform_os_and_size_mismatches() {
    for (ipa, change, expected) in [
        (
            ipa_with("wrong.bundle", "iPhoneOS", "16"),
            0,
            "Bundle ID mismatch: expected test.app, actual wrong.bundle",
        ),
        (ipa_with("test.app", "AppleTVOS", "16"), 0, "platforms"),
        (ipa_with("test.app", "iPhoneOS", "18"), 0, "requires OS 18"),
        (
            ipa_with("test.app", "iPhoneOS", "16"),
            1,
            "SHA-256 mismatch",
        ),
        (
            ipa_with("test.app", "iPhoneOS", "16"),
            2,
            "Version mismatch",
        ),
        (ipa_with("test.app", "iPhoneOS", "16"), 3, "Build mismatch"),
        (ipa_with("test.app", "iPhoneOS", "16"), 4, "Size mismatch"),
    ] {
        let mock = Mock::new(vec![Response::ok(feed()), Response::ok(ipa)]).await;
        let err = download(&mock, |r| match change {
            1 => r.sha256 = Some("0".repeat(64)),
            2 => r.version = "2".into(),
            3 => r.build = Some("3".into()),
            4 => r.size = Some(1),
            _ => {}
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");
    }
}
#[tokio::test]
async fn download_http_html_invalid_and_incomplete_archives_are_rejected() {
    let ipa = ipa_with("test.app", "iPhoneOS", "16");
    let mut truncated = Response::ok(ipa.clone());
    truncated.declared = Some(ipa.len() + 100);
    for response in [
        Response::status(403),
        Response::ok(b"<html>login</html>".to_vec()),
        Response::ok(ipa.clone()).header("Content-Type", "text/html"),
        Response::ok(ipa[..ipa.len() / 2].to_vec()),
        truncated,
    ] {
        let mock = Mock::new(vec![Response::ok(feed()), response]).await;
        assert!(download(&mock, |_| {}).await.is_err());
    }
}
#[tokio::test]
async fn cancelled_download_leaves_no_partial_file_and_can_retry() {
    let mock = Mock::new(vec![
        Response::ok(feed()),
        Response::ok(ipa_with("test.app", "iPhoneOS", "16")).delay(),
        Response::ok(ipa_with("test.app", "iPhoneOS", "16")),
    ])
    .await;
    let client = SourceClient::new().unwrap();
    let s = client
        .add(mock.url.as_str(), &CancellationToken::new())
        .await
        .unwrap();
    let r = &s.catalog.apps[0].releases[0];
    let dir = tempfile::tempdir().unwrap();
    let stop = CancellationToken::new();
    let cloned = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cloned.cancel();
    });
    assert!(
        client
            .download(
                r,
                "test.app",
                &device(),
                dir.path(),
                &stop,
                &DownloadProgress::default()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("Cancelled")
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(
        client
            .download(
                r,
                "test.app",
                &device(),
                dir.path(),
                &CancellationToken::new(),
                &DownloadProgress::default()
            )
            .await
            .is_ok()
    );
}
#[test]
fn rejects_traversal_and_crc_corruption_before_extracting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.ipa");
    let mut zip = zip::ZipWriter::new(Cursor::new(vec![]));
    zip.start_file("../escape", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"bad").unwrap();
    std::fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
    assert!(inspect_ipa(&path, &CancellationToken::new()).is_err());
    let mut bytes = ipa_with("test.app", "iPhoneOS", "16");
    let offset = bytes.windows(9).position(|s| s == b"synthetic").unwrap();
    bytes[offset] = b'X';
    std::fs::write(&path, bytes).unwrap();
    assert!(
        inspect_ipa(&path, &CancellationToken::new())
            .unwrap_err()
            .to_string()
            .contains("Corrupt")
    );
}
#[tokio::test]
async fn bounded_redirects_fail_instead_of_looping() {
    let mock = Mock::new(vec![Response::status(302).header("Location", "/loop"); 10]).await;
    assert!(
        SourceClient::new()
            .unwrap()
            .add(mock.url.as_str(), &CancellationToken::new())
            .await
            .is_err()
    );
    assert!(mock.requests.lock().unwrap().len() <= 9);
}

#[tokio::test]
async fn cancels_a_partially_written_file_and_times_out_stalled_body() {
    let mut stalled = Response::ok(ipa_with("test.app", "iPhoneOS", "16"));
    stalled.body_delay = true;
    let mock = Mock::new(vec![Response::ok(feed()), stalled.clone(), stalled]).await;
    let client = SourceClient::with_timeout(Duration::from_millis(300)).unwrap();
    let source = client
        .add(mock.url.as_str(), &CancellationToken::new())
        .await
        .unwrap();
    let release = source.catalog.apps[0].releases[0].clone();
    let directory = tempfile::tempdir().unwrap();
    let progress = Arc::new(DownloadProgress::default());
    let cancel = CancellationToken::new();
    let observed = progress.clone();
    let stop = cancel.clone();
    let cancelled = async {
        tokio::time::timeout(Duration::from_secs(1), async {
            while observed.snapshot().0 == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        stop.cancel();
    };
    let profile = device();
    let (result, ()) = tokio::join!(
        client.download(
            &release,
            "test.app",
            &profile,
            directory.path(),
            &cancel,
            &progress
        ),
        cancelled
    );
    assert!(result.unwrap_err().to_string().contains("Cancelled"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    let error = client
        .download(
            &release,
            "test.app",
            &device(),
            directory.path(),
            &CancellationToken::new(),
            &DownloadProgress::default(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn newer_mock_catalog_offers_update_but_retains_installed_release_and_identity() {
    let mut newer: serde_json::Value = serde_json::from_slice(&feed()).unwrap();
    newer["apps"][0]["versions"][0]["version"] = json!("2.0");
    newer["apps"][0]["versions"][0]["downloadURL"] = json!("/new-artifact");
    let mock = Mock::new(vec![
        Response::ok(feed()),
        Response::ok(serde_json::to_vec(&newer).unwrap()),
    ])
    .await;
    let client = SourceClient::new().unwrap();
    let original = client
        .add(mock.url.as_str(), &CancellationToken::new())
        .await
        .unwrap();
    let record = InstallRecord {
        source_key: original.key.clone(),
        original_bundle_id: "test.app".into(),
        release: original.catalog.apps[0].releases[0].clone(),
        artifact_sha256: "installed-hash".into(),
        installed_bundle_id: "test.app.source.TEAM".into(),
        device_udid: "device".into(),
        account_key: "account".into(),
        team_id: "TEAM".into(),
        settings: json!({"refresh":true}),
        installed_at: chrono::Utc::now(),
    };
    let mut installs = InstallStore::default();
    installs.record_success(record.clone());
    let refreshed = client
        .refresh(&original, &CancellationToken::new())
        .await
        .unwrap();
    let app = &refreshed.catalog.apps[0];
    assert_eq!(
        release_change(
            app,
            app.newest_eligible(Some(&device())).unwrap(),
            &record.release
        ),
        ReleaseChange::Newer
    );
    assert_eq!(installs.installs[0].release.version, "1.0");
    assert_eq!(
        installs.installs[0].installed_bundle_id,
        "test.app.source.TEAM"
    );
    assert_eq!(
        installs.installs[0].release.download_url.path(),
        "/artifact"
    );
    assert_eq!(mock.requests.lock().unwrap().len(), 2); // Catalog checks never download/install an IPA.
}
