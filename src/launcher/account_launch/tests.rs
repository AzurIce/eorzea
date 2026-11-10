use super::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    root: PathBuf,
    auth_path: PathBuf,
    wine: WineSettings,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = PathBuf::from("/tmp/opencode").join(format!(
            "eorzea-account-launch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("game")).unwrap();
        std::fs::create_dir_all(root.join("sdo/sdologin")).unwrap();
        std::fs::write(root.join("game/ffxiv_dx11.exe"), b"test").unwrap();
        std::fs::write(root.join("sdo/sdologin/sdologinentry64.dll"), b"ottercorp").unwrap();
        let auth_path = root.join("auth.toml");
        auth::save(
            &auth_path,
            &AuthConfig {
                default_account: Some("player".into()),
                accounts: vec![auth::Account {
                    snda_id: "account".into(),
                    username: Some("player".into()),
                    auto_login_session_key: Some("initial-key".into()),
                    area: Some("原大区".into()),
                }],
            },
        )
        .unwrap();
        let wine = WineSettings {
            startup_type: crate::config::WineStartupType::Custom,
            // 认证后在进程创建前报错，不启动真实游戏，也不写用户的游戏日志。
            custom_path: Some(root.join("not-installed/wine")),
            prefix: Some(root.join("prefix")),
            ..Default::default()
        };
        Self {
            root,
            auth_path,
            wine,
        }
    }

    fn request<'a>(&'a self, account: Option<&'a str>) -> LaunchRequest<'a> {
        let area = SdoArea {
            area_id: "1".into(),
            area_name: "测试大区".into(),
            area_stat: 1,
            area_order: 1,
            area_type: 1,
            area_lobby: "lobby.invalid".into(),
            area_gm: "gm.invalid".into(),
            area_patch: "patch.invalid".into(),
            area_config_upload: "config.invalid".into(),
        };
        LaunchRequest {
            game_root: &self.root,
            auth_path: &self.auth_path,
            account,
            areas: vec![area.clone()],
            area,
            wine: &self.wine,
            dalamud: Some(false),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct MockAuth {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockAuth {
    async fn new(override_response: impl Fn(&str) -> Option<Value> + Send + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            let mut attempt = 0;
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 2048];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..n]);
                }
                let text = String::from_utf8_lossy(&request);
                let path = text.split_whitespace().nth(1).unwrap().to_string();
                recorded.lock().unwrap().push(path.clone());
                let endpoint = path.split('?').next().unwrap();
                let response = override_response(&path).unwrap_or_else(|| {
                    let data = match endpoint {
                        "/getGuid.json" => json!({"guid": "test-guid"}),
                        "/staticLogin.json" => json!({"SndaId": "account", "tgt": "password-tgt"}),
                        "/getAccountGroup" => json!({"SndaIdArray": ["account"], "accountArray": ["player"]}),
                        "/accountGroupLogin" => json!({"tgt": "group-tgt", "autoLoginSessionKey": "password-key"}),
                        "/autoLogin.json" => {
                            attempt += 1;
                            json!({"SndaId": "account", "tgt": "auto-tgt", "autoLoginSessionKey": format!("key-{attempt}"), "autoLoginMaxAge": 86400})
                        }
                        "/fastLogin.json" => json!({"SndaId": "account", "tgt": "fast-tgt"}),
                        "/getPromotionInfo.json" => json!({}),
                        "/ssoLogin.json" => json!({"ticket": format!("ticket-{attempt}")}),
                        _ => panic!("unexpected mock endpoint: {endpoint}"),
                    };
                    json!({"return_code": 0, "data": data})
                });
                let body = response.to_string();
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(body.as_bytes()).await.unwrap();
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    fn launcher(&self) -> Launcher {
        let mut launcher = Launcher::new().unwrap();
        launcher.auth = launcher.auth.with_fallback_url(&self.url);
        launcher
    }

    fn auto_login_requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with("/autoLogin.json?"))
            .cloned()
            .collect()
    }
}

impl Drop for MockAuth {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn every_launch_attempt_authenticates_again_and_saves_before_starting() {
    let fixture = Fixture::new();
    let server = MockAuth::new(|_| None).await;
    let launcher = server.launcher();
    // 同一个 Launcher 连续调用，模拟 GUI 重试；同时覆盖默认账号、名称、snda_id。
    for (n, account) in [None, Some("player"), Some("account")]
        .into_iter()
        .enumerate()
    {
        let mut events = Vec::new();
        let error = launcher
            .launch_account(fixture.request(account), |event| match event {
                LaunchEvent::LoggingIn { .. } => events.push("login"),
                LaunchEvent::SessionRefreshed {
                    remaining_seconds, ..
                } => {
                    assert_eq!(remaining_seconds, Some(86400));
                    assert_eq!(
                        load_config(&fixture.auth_path).unwrap().accounts[0]
                            .auto_login_session_key
                            .as_deref(),
                        Some(format!("key-{}", n + 1).as_str())
                    );
                    events.push("saved");
                }
                LaunchEvent::Starting { .. } => events.push("start"),
                LaunchEvent::DalamudProgress(..) => panic!("Dalamud is disabled"),
            })
            .await
            .unwrap_err();
        assert!(
            matches!(error, LauncherError::Game(GameLaunchError::Wine(_))),
            "{error}"
        );
        assert_eq!(events, ["login", "saved", "start"]);
    }
    let requests = server.auto_login_requests();
    assert_eq!(requests.len(), 3);
    for (request, key) in requests.iter().zip(["initial-key", "key-1", "key-2"]) {
        assert!(
            request.contains(&format!("autoLoginSessionKey={key}&")),
            "{request}"
        );
    }
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with("/ssoLogin.json?"))
            .count(),
        3
    );
}

#[tokio::test]
async fn reloads_external_credentials_and_merges_only_the_rotated_key() {
    let fixture = Fixture::new();
    let path = fixture.auth_path.clone();
    let server = MockAuth::new(move |url| {
        if url.starts_with("/ssoLogin.json?") {
            // 认证期间修改元数据，保存轮换 key 时不得覆盖。
            let mut cfg = load_config(&path).unwrap();
            cfg.accounts[0].area = Some("外部修改的大区".into());
            cfg.accounts.push(auth::Account {
                snda_id: "other".into(),
                username: None,
                auto_login_session_key: Some("other-key".into()),
                area: None,
            });
            cfg.default_account = Some("other".into());
            auth::save(&path, &cfg).unwrap();
        }
        None
    })
    .await;
    let launcher = server.launcher();
    // Launcher 已创建后，模拟 CLI 轮换了磁盘凭据。
    let mut cfg = load_config(&fixture.auth_path).unwrap();
    cfg.accounts[0].auto_login_session_key = Some("external-key".into());
    auth::save(&fixture.auth_path, &cfg).unwrap();
    let _ = launcher
        .launch_account(fixture.request(Some("account")), |_| {})
        .await;
    assert!(server.auto_login_requests()[0].contains("autoLoginSessionKey=external-key&"));
    let saved = load_config(&fixture.auth_path).unwrap();
    assert_eq!(saved.default_account.as_deref(), Some("other"));
    assert_eq!(saved.accounts.len(), 2);
    assert_eq!(saved.accounts[0].area.as_deref(), Some("外部修改的大区"));
    assert_eq!(
        saved.accounts[0].auto_login_session_key.as_deref(),
        Some("key-1")
    );
    assert_eq!(
        saved.accounts[1].auto_login_session_key.as_deref(),
        Some("other-key")
    );
}

#[tokio::test]
async fn changed_credentials_during_authentication_are_not_overwritten() {
    let fixture = Fixture::new();
    let path = fixture.auth_path.clone();
    let server = MockAuth::new(move |url| {
        if url.starts_with("/ssoLogin.json?") {
            let mut cfg = load_config(&path).unwrap();
            cfg.accounts[0].auto_login_session_key = Some("external-new-key".into());
            auth::save(&path, &cfg).unwrap();
        }
        None
    })
    .await;
    let mut started = false;
    let error = server
        .launcher()
        .launch_account(fixture.request(None), |event| {
            if matches!(event, LaunchEvent::Starting { .. }) {
                started = true;
            }
        })
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("凭据已被其他操作更新"),
        "{error}"
    );
    assert!(!started);
    assert_eq!(
        load_config(&fixture.auth_path).unwrap().accounts[0]
            .auto_login_session_key
            .as_deref(),
        Some("external-new-key")
    );
}

#[tokio::test]
async fn expired_credentials_do_not_start_the_game_or_change_saved_config() {
    let fixture = Fixture::new();
    let original = std::fs::read(&fixture.auth_path).unwrap();
    let server = MockAuth::new(|url| {
        url.starts_with("/autoLogin.json?")
            .then(|| json!({"return_code": -10515005, "data": {}}))
    })
    .await;
    let mut started = false;
    let error = server
        .launcher()
        .launch_account(fixture.request(None), |event| {
            if matches!(event, LaunchEvent::Starting { .. }) {
                started = true;
            }
        })
        .await
        .unwrap_err();
    assert!(matches!(error, LauncherError::Auth(_)), "{error}");
    assert!(!started);
    assert_eq!(std::fs::read(&fixture.auth_path).unwrap(), original);
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_config_and_missing_accounts_fail_without_authentication() {
    let fixture = Fixture::new();
    let server = MockAuth::new(|_| None).await;
    let launcher = server.launcher();
    let original = load_config(&fixture.auth_path).unwrap();
    let error = launcher
        .launch_account(fixture.request(Some("unknown")), |_| {})
        .await
        .unwrap_err();
    assert!(error.to_string().contains("未找到已保存账号"), "{error}");

    let mut cfg = original;
    cfg.accounts[0].auto_login_session_key = None;
    auth::save(&fixture.auth_path, &cfg).unwrap();
    let error = launcher
        .launch_account(fixture.request(None), |_| {})
        .await
        .unwrap_err();
    assert!(error.to_string().contains("没有自动登录凭据"), "{error}");

    // 解析错误也不能把 TOML 中的敏感值附加到错误消息中。
    std::fs::write(
        &fixture.auth_path,
        b"auto_login_session_key = 'secret-value' invalid",
    )
    .unwrap();
    let error = launcher
        .launch_account(fixture.request(None), |_| {})
        .await
        .unwrap_err();
    assert!(error.to_string().contains("配置格式无效"), "{error}");
    assert!(!error.to_string().contains("secret-value"));
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn incomplete_or_mismatched_login_results_do_not_start_or_save() {
    for (endpoint, response, expected) in [
        (
            "/autoLogin.json?",
            json!({"return_code": 0, "data": {"SndaId": "account", "tgt": "tgt"}}),
            "未返回新的 session key",
        ),
        (
            "/fastLogin.json?",
            json!({"return_code": 0, "data": {"SndaId": "other", "tgt": "tgt"}}),
            "与所选账号不一致",
        ),
    ] {
        let fixture = Fixture::new();
        let original = std::fs::read(&fixture.auth_path).unwrap();
        let server =
            MockAuth::new(move |url| url.starts_with(endpoint).then(|| response.clone())).await;
        let mut started = false;
        let error = server
            .launcher()
            .launch_account(fixture.request(None), |event| {
                if matches!(event, LaunchEvent::Starting { .. }) {
                    started = true;
                }
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert!(!started);
        assert_eq!(std::fs::read(&fixture.auth_path).unwrap(), original);
    }
}

#[tokio::test]
async fn preflight_errors_and_launch_lock_do_not_consume_credentials() {
    let fixture = Fixture::new();
    let server = MockAuth::new(|_| None).await;
    let launcher = server.launcher();
    std::fs::create_dir_all(fixture.root.join(".eorzea")).unwrap();
    let pending = fixture.root.join(".eorzea/update.json");
    std::fs::write(&pending, b"unfinished").unwrap();
    let error = launcher
        .launch_account(fixture.request(None), |_| {})
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        LauncherError::Game(GameLaunchError::UpdateInProgress)
    ));
    std::fs::remove_file(pending).unwrap();

    let lock = std::fs::File::create(fixture.auth_path.with_extension("launch.lock")).unwrap();
    lock.try_lock().unwrap();
    let error = launcher
        .launch_account(fixture.request(None), |_| {})
        .await
        .unwrap_err();
    assert!(error.to_string().contains("另一个启动任务"), "{error}");
    drop(lock);

    let mut request = fixture.request(None);
    let missing = fixture.root.join("missing-game");
    request.game_root = &missing;
    let error = launcher.launch_account(request, |_| {}).await.unwrap_err();
    assert!(
        error.to_string().contains("未找到游戏可执行文件"),
        "{error}"
    );
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn password_login_obtains_a_reusable_key_instead_of_requiring_a_cached_ticket() {
    let server = MockAuth::new(|_| None).await;
    let token = server
        .launcher()
        .login_password("player", "test-password")
        .await
        .unwrap();
    assert_eq!(
        token.auto_login_session_key.as_deref(),
        Some("password-key")
    );
    assert_eq!(token.username.as_deref(), Some("player"));
    let requests = server.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|p| p.starts_with("/accountGroupLogin?"))
    );
    assert!(
        requests
            .iter()
            .any(|p| p.starts_with("/ssoLogin.json?tgt=group-tgt&"))
    );
}
