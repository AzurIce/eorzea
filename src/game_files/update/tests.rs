use super::backend::Plan;
use super::*;
use crate::game_files::patch_manager::patch_cache_path;
use sha1::{Digest, Sha1};
use std::fs;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "eorzea-update-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend((body.len() as u32).to_be_bytes());
    out.extend(kind);
    out.extend(body);
    out.extend(0u32.to_be_bytes());
}

fn patch(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for magic in crate::game_files::zpatch::ZIPATCH_MAGIC {
        out.extend(magic.to_le_bytes());
    }
    let path = b"result.bin";
    let mut command = vec![b'A', 0, 0];
    command.extend(0i64.to_be_bytes());
    command.extend((data.len() as i64).to_be_bytes());
    command.extend((path.len() as u32).to_be_bytes());
    command.extend(0u16.to_be_bytes());
    command.extend([0, 0]);
    command.extend(path);
    let mut block = Vec::new();
    block.extend(16i32.to_le_bytes());
    block.extend(0i32.to_le_bytes());
    block.extend(0x7d00i32.to_le_bytes());
    block.extend((data.len() as i32).to_le_bytes());
    block.extend(data);
    block.resize((block.len() + 127) & !127, 0);
    command.extend(block);
    let mut body = Vec::from(((command.len() + 5) as i32).to_be_bytes());
    body.push(b'F');
    body.extend(command);
    chunk(&mut out, b"SQPK", &body);
    chunk(&mut out, b"EOF_", &[0; 32]);
    out
}

fn entry(url: String, index: usize, bytes: &[u8]) -> PatchListEntry {
    PatchListEntry {
        version: format!("version-{index}"),
        url,
        length: bytes.len() as u64,
        hash_type: "sha1".into(),
        hash_block_size: 64,
        hashes: bytes
            .chunks(64)
            .map(|block| format!("{:x}", Sha1::digest(block)))
            .collect(),
    }
}

fn manager() -> GameFileManager {
    GameFileManager {
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
    }
}

fn persist(root: &Path, entries: &[PatchListEntry], next: usize, cache: &Path) {
    fs::create_dir_all(root.join(".eorzea")).unwrap();
    fs::create_dir_all(cache).unwrap();
    let plan = Plan {
        patches: entries.into(),
        next,
        patch_dir: cache.to_path_buf(),
    };
    fs::write(plan_path(root), serde_json::to_vec(&plan).unwrap()).unwrap();
}

/// 一个请求可被测试回调暂停，避免靠睡眠猜测流水线是否重叠。
async fn server(
    bodies: Vec<Vec<u8>>,
    blocked: usize,
    release: Arc<AtomicBool>,
) -> (
    String,
    Arc<Mutex<Vec<(usize, bool)>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/game", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let record = requests.clone();
    let task = tokio::spawn(async move {
        let mut clients = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut socket, _) = accepted.unwrap();
                    let bodies = bodies.clone();
                    let record = record.clone();
                    let release = release.clone();
                    clients.spawn(async move {
                        let mut request = Vec::new();
                        let mut buffer = [0; 1024];
                        while !request.windows(4).any(|b| b == b"\r\n\r\n") {
                            let n = socket.read(&mut buffer).await.unwrap();
                            if n == 0 { return; }
                            request.extend_from_slice(&buffer[..n]);
                        }
                        let request = String::from_utf8_lossy(&request);
                        let index: usize = request.split_whitespace().nth(1).unwrap()
                            .rsplit('/').next().unwrap().trim_end_matches(".patch").parse().unwrap();
                        record.lock().unwrap().push((index, release.load(Ordering::SeqCst)));
                        if index == blocked {
                            tokio::time::timeout(Duration::from_secs(8), async {
                                while !release.load(Ordering::SeqCst) {
                                    tokio::time::sleep(Duration::from_millis(5)).await;
                                }
                            }).await.unwrap();
                        }
                        let body = &bodies[index];
                        let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        socket.write_all(header.as_bytes()).await.unwrap();
                        socket.write_all(body).await.unwrap();
                    });
                },
                _ = clients.join_next(), if !clients.is_empty() => {},
            }
        }
    });
    (url, requests, task)
}

#[tokio::test(flavor = "current_thread")]
async fn download_and_apply_overlap_with_bounded_lookahead() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    let cache = root.join("cache");
    let bodies: Vec<_> = (0..4)
        .map(|n| patch(format!("patch-{n}").as_bytes()))
        .collect();
    let release = Arc::new(AtomicBool::new(false));
    let (url, requests, server) = server(bodies.clone(), 1, release.clone()).await;
    let entries: Vec<_> = bodies
        .iter()
        .enumerate()
        .map(|(n, body)| entry(format!("{url}/{n}.patch"), n, body))
        .collect();
    let mut overlapped = false;
    let mut last = UpdateProgress::default();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        manager().update(&entries, &cache, root, 2, |p| {
            if p.install.completed >= 1 && !release.load(Ordering::SeqCst) {
                assert!(p.download.completed + p.download.skipped < entries.len());
                overlapped = true;
                release.store(true, Ordering::SeqCst);
            }
            assert!(
                p.download
                    .active
                    .iter()
                    .filter(|f| f.phase == super::super::patch_manager::PatchPhase::Downloading)
                    .count()
                    <= 2
            );
            last = p;
        }),
    )
    .await
    .unwrap()
    .unwrap();
    server.abort();
    assert!(overlapped);
    assert_eq!(result.installed.len(), 4);
    assert_eq!(fs::read(root.join("game/result.bin")).unwrap(), b"patch-3");
    assert_eq!(
        fs::read_to_string(root.join("game/ffxivgame.ver")).unwrap(),
        "version-3"
    );
    assert_eq!(
        fs::read_to_string(root.join("game/ffxivgame.bck")).unwrap(),
        "version-3"
    );
    assert_eq!(last.install.completed, 4);
    assert_eq!(last.install.done, last.install.total);
    assert_eq!(last.download.done, last.download.total);
    assert!(last.download.active.is_empty());
    assert!(!has_pending_update(root));
    for entry in &entries {
        assert!(!patch_cache_path(&cache, entry).exists());
    }
    // 预取窗口为正在应用的一份 + 两份预取，不能提前请求第四份。
    assert!(requests
        .lock()
        .unwrap()
        .iter()
        .any(|&(n, committed)| n == 3 && committed));
}

#[tokio::test(flavor = "current_thread")]
async fn later_download_finishes_first_but_install_waits_for_head() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    let bodies = vec![patch(b"first"), patch(b"second")];
    let release = Arc::new(AtomicBool::new(false));
    let (url, _, server) = server(bodies.clone(), 0, release.clone()).await;
    let entries: Vec<_> = bodies
        .iter()
        .enumerate()
        .map(|(n, b)| entry(format!("{url}/{n}.patch"), n, b))
        .collect();
    let mut saw_wait = false;
    tokio::time::timeout(
        Duration::from_secs(10),
        manager().update(&entries, &root.join("cache"), root, 2, |p| {
            if p.download.completed == 1 && !release.load(Ordering::SeqCst) {
                assert_eq!(p.install.completed, 0);
                assert!(!root.join("game/result.bin").exists());
                assert_eq!(p.ready.len(), 1);
                assert_eq!(p.ready[0].index, 1);
                assert_eq!(p.ready[0].done, entries[1].length);
                saw_wait = p.waiting_for_download;
                release.store(true, Ordering::SeqCst);
            }
            if let Some(current) = &p.install.current {
                assert!(p.ready.iter().all(|file| file.index != current.index));
            }
            assert!(p.ready.iter().all(|file| file.index >= p.install.completed));
        }),
    )
    .await
    .unwrap()
    .unwrap();
    server.abort();
    assert!(saw_wait);
    assert_eq!(fs::read(root.join("game/result.bin")).unwrap(), b"second");
}

#[tokio::test]
async fn resumes_original_queue_with_missing_version_and_partial_game_files() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    let cache = root.join("cache");
    let old = patch(b"old");
    let new = patch(b"complete");
    let entries = vec![
        entry("http://127.0.0.1:9/game/ex1/0.patch".into(), 0, &old),
        entry("http://127.0.0.1:9/game/ex1/1.patch".into(), 1, &new),
    ];
    persist(root, &entries, 1, &cache);
    fs::create_dir_all(root.join("game")).unwrap();
    fs::write(root.join("game/result.bin"), b"part").unwrap();
    fs::write(patch_cache_path(&cache, &entries[1]), new).unwrap();
    let manager = manager();
    let pending = manager.pending_update(root).await.unwrap().unwrap();
    assert_eq!(pending.patches, entries[1..]);
    // 使用记录中的缓存目录；不能因缺失 .ver 重新请求全量补丁。
    manager
        .update(
            &pending.patches,
            &root.join("ignored-cache"),
            root,
            4,
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(fs::read(root.join("game/result.bin")).unwrap(), b"complete");
    assert_eq!(
        fs::read_to_string(root.join("game/sqpack/ex1/ex1.ver")).unwrap(),
        "version-1"
    );
    assert!(!has_pending_update(root));
}

#[tokio::test(flavor = "current_thread")]
async fn minimum_concurrency_downloads_beyond_lookahead_while_head_is_blocked() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    let bodies: Vec<_> = (0..5).map(|_| patch(b"data")).collect();
    let release = Arc::new(AtomicBool::new(false));
    let (url, _, server) = server(bodies.clone(), 0, release.clone()).await;
    let entries: Vec<_> = bodies
        .iter()
        .enumerate()
        .map(|(n, body)| entry(format!("{url}/{n}.patch"), n, body))
        .collect();
    let mut passed_window = false;
    tokio::time::timeout(
        Duration::from_secs(10),
        manager().update_with_min_concurrency(&entries, &root.join("cache"), root, 2, 2, |p| {
            assert!(
                p.download
                    .active
                    .iter()
                    .filter(
                        |file| file.phase == super::super::patch_manager::PatchPhase::Downloading
                    )
                    .count()
                    <= 2
            );
            if !release.load(Ordering::SeqCst) && p.ready.iter().any(|file| file.index >= 3) {
                assert_eq!(p.install.completed, 0);
                passed_window = true;
                release.store(true, Ordering::SeqCst);
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    server.abort();
    assert!(passed_window);
}

#[tokio::test]
async fn minimum_concurrency_cannot_exceed_maximum() {
    let scratch = Scratch::new();
    assert!(manager()
        .update_with_min_concurrency(&[], &scratch.0.join("cache"), &scratch.0, 2, 3, |_| {},)
        .await
        .is_err());
    assert!(!has_pending_update(&scratch.0));
}

#[tokio::test]
async fn truncated_patch_retains_queue_and_does_not_install_successor() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    let cache = root.join("cache");
    let mut bad = patch(b"first");
    bad.truncate(bad.len() - 44); // 缺完整 EOF_ chunk
    let good = patch(b"second");
    let entries = vec![
        entry("http://127.0.0.1:9/game/0.patch".into(), 0, &bad),
        entry("http://127.0.0.1:9/game/1.patch".into(), 1, &good),
    ];
    persist(root, &entries, 0, &cache);
    fs::write(patch_cache_path(&cache, &entries[0]), bad).unwrap();
    fs::write(patch_cache_path(&cache, &entries[1]), good).unwrap();
    assert!(manager()
        .update(&entries, &cache, root, 2, |_| {})
        .await
        .is_err());
    assert!(has_pending_update(root));
    assert!(!root.join("game/ffxivgame.ver").exists());
    assert_eq!(fs::read(root.join("game/result.bin")).unwrap(), b"first");
    assert!(entries.iter().all(|e| patch_cache_path(&cache, e).exists()));
    assert_eq!(
        manager()
            .pending_update(root)
            .await
            .unwrap()
            .unwrap()
            .patches,
        entries
    );
}

#[tokio::test]
async fn a_missing_file_after_commit_is_redownloaded_before_advancing() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    let cache = root.join("cache");
    let body = patch(b"same-patch");
    let release = Arc::new(AtomicBool::new(true));
    let (url, requests, server) = server(vec![body.clone()], usize::MAX, release).await;
    let entries = vec![entry(format!("{url}/0.patch"), 0, &body)];
    persist(root, &entries, 0, &cache);
    fs::create_dir_all(root.join("game")).unwrap();
    // 模拟已写 .ver 并删除补丁，但队列游标尚未持久化。
    fs::write(root.join("game/ffxivgame.ver"), b"version-0").unwrap();
    manager()
        .update(&entries, &cache, root, 1, |_| {})
        .await
        .unwrap();
    server.abort();
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(
        fs::read(root.join("game/result.bin")).unwrap(),
        b"same-patch"
    );
    assert!(!has_pending_update(root));
}

#[tokio::test]
async fn second_updater_is_rejected_and_finished_plan_can_complete_cleanup() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    fs::create_dir_all(root.join(".eorzea")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".eorzea/update.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let result = manager()
        .update(&[], &root.join("cache"), root, 1, |_| {})
        .await;
    assert!(result.unwrap_err().to_string().contains("已有更新任务"));
    drop(lock);
    let body = patch(b"x");
    let entries = vec![entry("http://127.0.0.1:9/game/0.patch".into(), 0, &body)];
    persist(root, &entries, 1, &root.join("cache"));
    fs::create_dir_all(root.join("game")).unwrap();
    fs::write(root.join("game/ffxivgame.ver"), b"version-0").unwrap();
    manager()
        .update(&[], &root.join("cache"), root, 1, |_| {})
        .await
        .unwrap();
    assert!(!has_pending_update(root));
    assert_eq!(
        fs::read_to_string(root.join("game/ffxivgame.bck")).unwrap(),
        "version-0"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn deletion_failure_does_not_advance_queue() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    let root = &scratch.0;
    let cache = root.join("cache");
    let bodies = [patch(b"first"), patch(b"second")];
    let entries: Vec<_> = bodies
        .iter()
        .enumerate()
        .map(|(n, b)| entry(format!("http://127.0.0.1:9/game/{n}.patch"), n, b))
        .collect();
    persist(root, &entries, 0, &cache);
    for (e, b) in entries.iter().zip(bodies) {
        fs::write(patch_cache_path(&cache, e), b).unwrap();
    }
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(cache.join("permission-check"), b"").is_ok() {
        // root 等特权用户不受目录权限约束。
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let result = manager().update(&entries, &cache, root, 2, |_| {}).await;
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(root.join("game/result.bin")).unwrap(), b"first");
    assert_eq!(
        fs::read_to_string(root.join("game/ffxivgame.ver")).unwrap(),
        "version-0"
    );
    assert_eq!(
        manager()
            .pending_update(root)
            .await
            .unwrap()
            .unwrap()
            .patches,
        entries
    );
    manager()
        .update(&entries, &cache, root, 2, |_| {})
        .await
        .unwrap();
    assert_eq!(fs::read(root.join("game/result.bin")).unwrap(), b"second");
}
