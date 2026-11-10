//! 补丁下载管理。
//!
//! 对应 C# `PatchManager` 的下载部分（`PatchAcquisition` + `CheckPatchValidity`）：
//! - 并发下载（默认 4 个槽位，同 C# `MAX_DOWNLOADS_AT_ONCE`）
//! - 已存在但不完整的补丁按 `Range` 续传；服务端忽略 Range 时自动回退全量重下
//! - 下载前后都按块做 SHA1 校验（`HashType=sha1`、`HashBlockSize` 分块），流式读取
//! - 已下载且校验通过的补丁自动跳过
//! - 进度按网络块实时上报，每个在途补丁带独立子进度
//!
//! 补丁**应用**见 `crate::game_files::zpatch`。

use eorzea_auth::PatchListEntry;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, info, warn};

/// 并发下载槽位数，与 C# `PatchManager.MAX_DOWNLOADS_AT_ONCE` 一致。
pub const MAX_DOWNLOADS_AT_ONCE: usize = 4;

/// 单个失败文件的最大尝试次数（每次都会从当前已写入位置续传）。
#[cfg(not(target_arch = "wasm32"))]
const MAX_DOWNLOAD_ATTEMPTS: usize = 5;
/// 进度回调的最小间隔。
#[cfg(not(target_arch = "wasm32"))]
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

/// 补丁下载结果。
#[derive(Debug)]
pub struct DownloadSummary {
    pub downloaded: Vec<PathBuf>,
    pub skipped: usize,
    pub total_bytes: u64,
}

/// 单个补丁当前处于哪个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchPhase {
    Downloading,
    Verifying,
}

/// 单个正在处理的补丁。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchProgress {
    /// 在补丁列表中的稳定下标，CLI 用它区分同一个子进度条。
    pub index: usize,
    pub version: String,
    pub url: String,
    /// 已写入磁盘的字节数（含续传前已有的部分）。
    pub done: u64,
    /// 补丁总大小。
    pub total: u64,
    /// 本次开始下载时从文件中复用的字节数；0 表示从头下载。
    pub resumed_from: u64,
    /// 当前阶段：下载中或 SHA1 校验中。
    pub phase: PatchPhase,
}

/// 下载进度快照。
///
/// `done` 包含：已完成/跳过文件的全部字节 + 当前在途文件已写入的字节。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    pub done: u64,
    pub total: u64,
    /// 当前正在处理的补丁（最多 `concurrency` 个），按列表下标排序。
    pub active: Vec<PatchProgress>,
    /// 已成功下载的文件数（不含跳过）。
    pub completed: usize,
    /// 已存在且校验通过、跳过的文件数。
    pub skipped: usize,
}

#[derive(Debug)]
pub(super) struct ProgressState {
    total: u64,
    completed_bytes: u64,
    completed: usize,
    skipped: usize,
    active: BTreeMap<usize, PatchProgress>,
}

impl ProgressState {
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn resume(&mut self, patches: &[PatchListEntry]) {
        self.completed_bytes = patches.iter().map(|p| p.length).sum();
        self.completed = patches.len();
    }
    pub(super) fn new(total: u64) -> Self {
        Self {
            total,
            completed_bytes: 0,
            completed: 0,
            skipped: 0,
            active: BTreeMap::new(),
        }
    }

    pub(super) fn snapshot(&self) -> DownloadProgress {
        let in_flight: u64 = self.active.values().map(|p| p.done).sum();
        DownloadProgress {
            done: (self.completed_bytes + in_flight).min(self.total),
            total: self.total,
            active: self.active.values().cloned().collect(),
            completed: self.completed,
            skipped: self.skipped,
        }
    }

    fn ensure_active(&mut self, index: usize, entry: &PatchListEntry, done: u64) {
        match self.active.get_mut(&index) {
            Some(p) => p.done = done.min(p.total),
            None => {
                self.active.insert(
                    index,
                    PatchProgress {
                        index,
                        version: entry.version.clone(),
                        url: entry.url.clone(),
                        done: done.min(entry.length),
                        total: entry.length,
                        resumed_from: done,
                        phase: PatchPhase::Downloading,
                    },
                );
            }
        }
    }

    fn set_done(&mut self, index: usize, done: u64) {
        if let Some(p) = self.active.get_mut(&index) {
            p.done = done.min(p.total);
        }
    }

    fn set_phase(&mut self, index: usize, phase: PatchPhase) {
        if let Some(p) = self.active.get_mut(&index) {
            p.phase = phase;
        }
    }

    fn finish(&mut self, index: usize, skipped: bool) {
        if let Some(p) = self.active.remove(&index) {
            self.completed_bytes += p.total;
            if skipped {
                self.skipped += 1;
            } else {
                self.completed += 1;
            }
        }
    }
}

/// 把 SHA1 摘要与补丁列表里的十六进制字符串比较，避免每块 `format!` 分配。
fn sha1_matches_hex(digest: &[u8], expected: &str) -> bool {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let expected = expected.as_bytes();
    if expected.len() != digest.len() * 2 {
        return false;
    }
    digest.iter().enumerate().all(|(i, byte)| {
        expected[i * 2] == HEX[(byte >> 4) as usize]
            && expected[i * 2 + 1] == HEX[(byte & 0x0f) as usize]
    })
}

/// 校验文件是否符合补丁条目的逐块 SHA1 哈希。
///
/// 对应 C# `PatchManager.CheckPatchValidity()`。
pub fn verify_patch_sha1(path: &Path, entry: &PatchListEntry) -> Result<bool, PatchDownloadError> {
    verify_patch_sha1_with_progress(path, entry, |_| {})
}

/// 带已处理字节回调的 [`verify_patch_sha1`]。
pub fn verify_patch_sha1_with_progress(
    path: &Path,
    entry: &PatchListEntry,
    mut on_progress: impl FnMut(u64),
) -> Result<bool, PatchDownloadError> {
    let metadata = std::fs::metadata(path).map_err(|e| PatchDownloadError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    if metadata.len() != entry.length {
        return Ok(false);
    }

    if entry.hash_type != "sha1" {
        warn!(
            hash_type = %entry.hash_type,
            "unsupported hash type, skipping verification"
        );
        return Ok(true);
    }
    if entry.hashes.is_empty() || entry.hash_block_size == 0 {
        // boot 补丁无 hash 信息，只校验大小
        return Ok(true);
    }

    let block_size = entry.hash_block_size as usize;
    if block_size == 0 {
        return Ok(true);
    }

    use sha1::{Digest, Sha1};
    use std::io::{BufReader, Read};

    let file = std::fs::File::open(path).map_err(|e| PatchDownloadError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut reader = BufReader::with_capacity(block_size.min(1 << 20), file);
    let mut block = vec![0u8; block_size];
    let mut hashed = 0u64;

    for (i, expected) in entry.hashes.iter().enumerate() {
        let n = reader
            .read(&mut block)
            .map_err(|e| PatchDownloadError::Io {
                path: path.to_path_buf(),
                source: e,
            })?;
        if n == 0 {
            debug!(block = i, file = %path.display(), "patch ended early");
            return Ok(false);
        }
        hashed += n as u64;
        on_progress(hashed);
        if !sha1_matches_hex(&Sha1::digest(&block[..n]), expected) {
            debug!(
                block = i,
                file = %path.display(),
                "SHA1 mismatch at block {i}"
            );
            return Ok(false);
        }
    }

    // 文件长度虽然与 entry.length 相同，但 hash 列表可能比实际块少；
    // 读一个字节确认没有多余数据。
    let mut extra = [0u8; 1];
    if reader
        .read(&mut extra)
        .map_err(|e| PatchDownloadError::Io {
            path: path.to_path_buf(),
            source: e,
        })?
        != 0
    {
        debug!(file = %path.display(), "patch has trailing data");
        return Ok(false);
    }

    on_progress(entry.length);
    Ok(true)
}

/// 校验补丁前 `available` 字节中**完整块**的 SHA1。
///
/// 用于续传前确认已有前缀没有损坏。只校验到最后一个完整块；
/// 末尾不完整的块会在续传时补齐，最终再由 [`verify_patch_sha1`] 整体校验。
fn verify_patch_prefix(
    path: &Path,
    entry: &PatchListEntry,
    available: u64,
) -> Result<bool, PatchDownloadError> {
    if entry.hash_type != "sha1" || entry.hashes.is_empty() || entry.hash_block_size == 0 {
        return Ok(true);
    }

    let block_size = entry.hash_block_size;
    let full_blocks = (available / block_size) as usize;
    if full_blocks == 0 {
        return Ok(true);
    }

    use sha1::{Digest, Sha1};
    use std::io::{BufReader, Read};

    let file = std::fs::File::open(path).map_err(|e| PatchDownloadError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut reader = BufReader::with_capacity((block_size as usize).min(1 << 20), file);
    let mut block = vec![0u8; block_size as usize];

    for i in 0..full_blocks {
        let Some(expected) = entry.hashes.get(i) else {
            return Ok(false);
        };
        let n = reader
            .read(&mut block)
            .map_err(|e| PatchDownloadError::Io {
                path: path.to_path_buf(),
                source: e,
            })?;
        if n == 0 {
            return Ok(false);
        }
        if !sha1_matches_hex(&Sha1::digest(&block[..n]), expected) {
            debug!(
                block = i,
                file = %path.display(),
                "SHA1 mismatch in partial patch at block {i}"
            );
            return Ok(false);
        }
    }

    Ok(true)
}

/// 返回补丁条目唯一对应的缓存路径。
///
/// 不同仓库可能使用相同的 basename 和版本号，因此缓存键必须包含完整 URL 身份。
pub(crate) fn patch_cache_path(dest_dir: &Path, entry: &PatchListEntry) -> PathBuf {
    use sha1::{Digest, Sha1};

    let file_name = entry
        .url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("patch");
    let url_hash = format!("{:x}", Sha1::digest(entry.url.as_bytes()));
    dest_dir.join(format!("{}-{}.{}", url_hash, file_name, entry.version))
}

/// 并发下载补丁到暂存目录。
///
/// - 已存在且 SHA1 校验通过的文件跳过
/// - 已存在但不完整/校验失败的文件从 Range 续传（服务端不支持 Range 时自动从头重下）
/// - 每个文件最多尝试 [`MAX_DOWNLOAD_ATTEMPTS`] 次，失败后保留已下载部分供下次继续
/// - 进度通过 [`DownloadProgress`] 回调上报（含每个在途文件的子进度）
pub async fn download_patches(
    client: &reqwest::Client,
    patches: &[PatchListEntry],
    dest_dir: &Path,
    concurrency: usize,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<DownloadSummary, PatchDownloadError> {
    std::fs::create_dir_all(dest_dir).map_err(|e| PatchDownloadError::Io {
        path: dest_dir.to_path_buf(),
        source: e,
    })?;

    let total_bytes: u64 = patches.iter().map(|p| p.length).sum();
    let state = Arc::new(Mutex::new(ProgressState::new(total_bytes)));
    let mut skipped = 0;
    let mut downloaded = Vec::new();

    // 原生：Semaphore + JoinSet 并发下载，主任务负责按固定频率上报进度。
    #[cfg(not(target_arch = "wasm32"))]
    {
        use tokio::task::JoinSet;

        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
        let mut tasks = JoinSet::new();
        for (index, entry) in patches.iter().cloned().enumerate() {
            let client = client.clone();
            let semaphore = semaphore.clone();
            let dest_dir = dest_dir.to_path_buf();
            let state = state.clone();
            tasks.spawn(async move {
                let permit = semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| PatchDownloadError::Semaphore)?;
                download_one(&client, &entry, &dest_dir, index, &state, permit, semaphore).await
            });
        }

        let mut ticker = tokio::time::interval(PROGRESS_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let snapshot = state
                        .lock()
                        .expect("progress state poisoned")
                        .snapshot();
                    on_progress(snapshot);
                }
                joined = tasks.join_next() => {
                    let Some(joined) = joined else { break; };
                    let (bytes, path) = match joined {
                        Ok(Ok(result)) => result,
                        Ok(Err(e)) => {
                            tasks.abort_all();
                            return Err(e);
                        }
                        Err(e) => {
                            tasks.abort_all();
                            return Err(PatchDownloadError::TaskJoin(e));
                        }
                    };
                    match bytes {
                        Some(_) => downloaded.push(path),
                        None => skipped += 1,
                    }
                    let snapshot = state
                        .lock()
                        .expect("progress state poisoned")
                        .snapshot();
                    on_progress(snapshot);
                }
            }
        }
    }

    // wasm：reqwest Response 含 Rc 非 Send，不能 tokio::spawn；顺序下载即可。
    #[cfg(target_arch = "wasm32")]
    {
        for (index, entry) in patches.iter().enumerate() {
            let (bytes, path) = download_one(client, entry, dest_dir, index, &state).await?;
            match bytes {
                Some(_) => downloaded.push(path),
                None => skipped += 1,
            }
            let snapshot = state.lock().expect("progress state poisoned").snapshot();
            on_progress(snapshot);
        }
    }

    Ok(DownloadSummary {
        downloaded,
        skipped,
        total_bytes,
    })
}

/// 下载单个补丁（原生）。
///
/// 返回 `(Some(本次写入字节数), 文件路径)`；若文件已存在且校验通过则返回 `(None, 路径)`。
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn download_for_update(
    client: reqwest::Client,
    entry: PatchListEntry,
    dest_dir: PathBuf,
    index: usize,
    state: Arc<Mutex<ProgressState>>,
    permit: OwnedSemaphorePermit,
    semaphore: Arc<Semaphore>,
) -> Result<(Option<u64>, PathBuf), PatchDownloadError> {
    download_one(&client, &entry, &dest_dir, index, &state, permit, semaphore).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn download_one(
    client: &reqwest::Client,
    entry: &PatchListEntry,
    dest_dir: &Path,
    index: usize,
    state: &Arc<Mutex<ProgressState>>,
    permit: OwnedSemaphorePermit,
    semaphore: Arc<Semaphore>,
) -> Result<(Option<u64>, PathBuf), PatchDownloadError> {
    let dest = patch_cache_path(dest_dir, entry);
    let mut permit = Some(permit);
    let existing = file_len(&dest).await?;

    if existing == entry.length {
        // 先计入进度，让重启后校验已有大文件时进度条不是从 0 开始。
        {
            let mut st = state.lock().expect("progress state poisoned");
            st.ensure_active(index, entry, existing);
            st.set_phase(index, PatchPhase::Verifying);
        }
        // 校验不占下载槽位：否则 4 个大文件同时校验时，网络会空转，
        // 表现就是“每批补丁下完停一段时间”。
        release_permit(&mut permit);
        if verify_sha1_async(dest.clone(), entry.clone()).await? {
            let mut st = state.lock().expect("progress state poisoned");
            st.finish(index, true);
            info!(
                file = %dest.display(),
                "patch already downloaded and verified, skipping"
            );
            return Ok((None, dest));
        }
        info!(
            file = %dest.display(),
            "existing patch failed verification, re-downloading from zero"
        );
        acquire_permit_if_needed(&mut permit, &semaphore).await?;
        truncate_to(&dest, 0).await?;
        let mut st = state.lock().expect("progress state poisoned");
        st.set_done(index, 0);
        st.set_phase(index, PatchPhase::Downloading);
    } else if existing > entry.length {
        warn!(
            file = %dest.display(),
            expected = entry.length,
            actual = existing,
            "cached patch is larger than expected, restarting"
        );
        truncate_to(&dest, 0).await?;
    } else if existing > 0 {
        {
            let mut st = state.lock().expect("progress state poisoned");
            st.ensure_active(index, entry, existing);
            st.set_phase(index, PatchPhase::Verifying);
        }
        release_permit(&mut permit);
        let resume_from = valid_resume_offset(&dest, entry, existing).await?;
        acquire_permit_if_needed(&mut permit, &semaphore).await?;
        state
            .lock()
            .expect("progress state poisoned")
            .set_phase(index, PatchPhase::Downloading);
        if resume_from < existing {
            truncate_to(&dest, resume_from).await?;
            state
                .lock()
                .expect("progress state poisoned")
                .set_done(index, resume_from);
        }
        if resume_from > 0 {
            info!(
                file = %dest.display(),
                resume_from,
                total = entry.length,
                "resuming partial patch"
            );
        } else {
            info!(
                file = %dest.display(),
                "partial patch prefix failed verification, restarting from zero"
            );
        }
    }

    info!(
        url = %entry.url,
        bytes = entry.length,
        dest = %dest.display(),
        "downloading patch"
    );

    let mut last_error = None;
    for attempt in 1..=MAX_DOWNLOAD_ATTEMPTS {
        acquire_permit_if_needed(&mut permit, &semaphore).await?;
        let mut offset = file_len(&dest).await?;
        if offset > entry.length {
            warn!(
                file = %dest.display(),
                expected = entry.length,
                actual = offset,
                "partial patch is larger than expected, truncating"
            );
            truncate_to(&dest, 0).await?;
            offset = 0;
        }
        {
            let mut st = state.lock().expect("progress state poisoned");
            st.ensure_active(index, entry, offset);
            st.set_phase(index, PatchPhase::Downloading);
        }

        match download_attempt(client, entry, &dest, index, state).await {
            Ok(received) => {
                if received != entry.length {
                    warn!(
                        file = %dest.display(),
                        expected = entry.length,
                        actual = received,
                        "patch size mismatch"
                    );
                    last_error = Some(PatchDownloadError::SizeMismatch {
                        url: entry.url.clone(),
                        expected: entry.length,
                        actual: received,
                    });
                } else {
                    // 网络部分结束，释放槽位给下一批补丁，再在后台校验。
                    state
                        .lock()
                        .expect("progress state poisoned")
                        .set_phase(index, PatchPhase::Verifying);
                    release_permit(&mut permit);
                    if verify_sha1_async(dest.clone(), entry.clone()).await? {
                        state
                            .lock()
                            .expect("progress state poisoned")
                            .finish(index, false);
                        debug!(file = %dest.display(), bytes = received, "patch downloaded");
                        return Ok((Some(received), dest));
                    }

                    warn!(
                        file = %dest.display(),
                        attempt,
                        "patch SHA1 mismatch after download, restarting"
                    );
                    acquire_permit_if_needed(&mut permit, &semaphore).await?;
                    truncate_to(&dest, 0).await?;
                    let mut st = state.lock().expect("progress state poisoned");
                    st.set_done(index, 0);
                    st.set_phase(index, PatchPhase::Downloading);
                    last_error = Some(PatchDownloadError::HashMismatch {
                        file: dest.clone(),
                        url: entry.url.clone(),
                    });
                }
            }
            Err(e) => {
                warn!(
                    url = %entry.url,
                    attempt,
                    error = %e,
                    "patch download attempt failed, will resume if possible"
                );
                last_error = Some(e);
            }
        }

        if attempt < MAX_DOWNLOAD_ATTEMPTS {
            let delay = Duration::from_millis(500 * attempt as u64);
            tokio::time::sleep(delay).await;
        }
    }

    Err(
        last_error.unwrap_or_else(|| PatchDownloadError::RetryExhausted {
            url: entry.url.clone(),
            attempts: MAX_DOWNLOAD_ATTEMPTS,
        }),
    )
}

/// 释放下载槽位（校验/截断等非网络阶段不应占用并发名额）。
#[cfg(not(target_arch = "wasm32"))]
fn release_permit(permit: &mut Option<OwnedSemaphorePermit>) {
    if let Some(permit) = permit.take() {
        drop(permit);
    }
}

/// 需要继续下载时重新获取槽位。
#[cfg(not(target_arch = "wasm32"))]
async fn acquire_permit_if_needed(
    permit: &mut Option<OwnedSemaphorePermit>,
    semaphore: &Arc<Semaphore>,
) -> Result<(), PatchDownloadError> {
    if permit.is_none() {
        *permit = Some(
            semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| PatchDownloadError::Semaphore)?,
        );
    }
    Ok(())
}

/// 下载单个补丁（wasm：不支持流式 chunk，一次性读完）。
#[cfg(target_arch = "wasm32")]
async fn download_one(
    client: &reqwest::Client,
    entry: &PatchListEntry,
    dest_dir: &Path,
    index: usize,
    state: &Arc<Mutex<ProgressState>>,
) -> Result<(Option<u64>, PathBuf), PatchDownloadError> {
    let dest = patch_cache_path(dest_dir, entry);

    if dest.exists() {
        {
            let mut st = state.lock().expect("progress state poisoned");
            st.ensure_active(index, entry, entry.length);
            st.set_phase(index, PatchPhase::Verifying);
        }
        match verify_patch_sha1(&dest, entry) {
            Ok(true) => {
                state
                    .lock()
                    .expect("progress state poisoned")
                    .finish(index, true);
                info!(file = %dest.display(), "patch already downloaded and verified, skipping");
                return Ok((None, dest));
            }
            Ok(false) => {
                info!(file = %dest.display(), "existing patch failed verification, re-downloading");
            }
            Err(e) => return Err(e),
        }
    }

    {
        let mut st = state.lock().expect("progress state poisoned");
        st.ensure_active(index, entry, 0);
        st.set_phase(index, PatchPhase::Downloading);
    }

    let mut response = client
        .get(&entry.url)
        .header("Accept", "*/*")
        .header("Accept-Encoding", "identity")
        .send()
        .await
        .map_err(|e| PatchDownloadError::Request {
            url: entry.url.clone(),
            source: e,
        })?;

    let status = response.status();
    if !status.is_success() {
        return Err(PatchDownloadError::Http {
            url: entry.url.clone(),
            status,
        });
    }

    let body = response
        .bytes()
        .await
        .map_err(|e| PatchDownloadError::Body {
            url: entry.url.clone(),
            source: e,
        })?;

    {
        use std::io::Write;
        let mut file = std::fs::File::create(&dest).map_err(|e| PatchDownloadError::Io {
            path: dest.clone(),
            source: e,
        })?;
        file.write_all(&body).map_err(|e| PatchDownloadError::Io {
            path: dest.clone(),
            source: e,
        })?;
        file.flush().map_err(|e| PatchDownloadError::Io {
            path: dest.clone(),
            source: e,
        })?;
    }

    state
        .lock()
        .expect("progress state poisoned")
        .set_done(index, body.len() as u64);

    let bytes = body.len() as u64;
    if bytes != entry.length {
        return Err(PatchDownloadError::SizeMismatch {
            url: entry.url.clone(),
            expected: entry.length,
            actual: bytes,
        });
    }

    state
        .lock()
        .expect("progress state poisoned")
        .set_phase(index, PatchPhase::Verifying);

    match verify_patch_sha1(&dest, entry) {
        Ok(true) => {
            state
                .lock()
                .expect("progress state poisoned")
                .finish(index, false);
            Ok((Some(bytes), dest))
        }
        Ok(false) => Err(PatchDownloadError::HashMismatch {
            file: dest,
            url: entry.url.clone(),
        }),
        Err(e) => Err(e),
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn file_len(path: &Path) -> Result<u64, PatchDownloadError> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => Ok(metadata.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(PatchDownloadError::Io {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn truncate_to(path: &Path, len: u64) -> Result<(), PatchDownloadError> {
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(path)
        .await
        .map_err(|e| PatchDownloadError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    file.set_len(len).await.map_err(|e| PatchDownloadError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

#[cfg(not(target_arch = "wasm32"))]
async fn verify_sha1_async(
    path: PathBuf,
    entry: PatchListEntry,
) -> Result<bool, PatchDownloadError> {
    tokio::task::spawn_blocking(move || verify_patch_sha1(&path, &entry))
        .await
        .map_err(PatchDownloadError::TaskJoin)?
}

#[cfg(not(target_arch = "wasm32"))]
async fn verify_prefix_async(
    path: PathBuf,
    entry: PatchListEntry,
    available: u64,
) -> Result<bool, PatchDownloadError> {
    tokio::task::spawn_blocking(move || verify_patch_prefix(&path, &entry, available))
        .await
        .map_err(PatchDownloadError::TaskJoin)?
}

/// 返回可安全续传的偏移量：校验已有完整块，损坏则从 0 开始。
#[cfg(not(target_arch = "wasm32"))]
async fn valid_resume_offset(
    dest: &Path,
    entry: &PatchListEntry,
    existing: u64,
) -> Result<u64, PatchDownloadError> {
    if existing == 0 {
        return Ok(0);
    }
    if entry.hash_type != "sha1" || entry.hashes.is_empty() || entry.hash_block_size == 0 {
        // boot 补丁没有块 hash，只能信任已有长度；最终仍会做大小校验。
        return Ok(existing);
    }

    let block_size = entry.hash_block_size;
    let aligned = existing - existing % block_size;
    if aligned == 0 {
        // 连一个完整块都没有，直接从这个偏移继续，最终整体 SHA1 会兜底。
        return Ok(existing);
    }

    if verify_prefix_async(dest.to_path_buf(), entry.clone(), aligned).await? {
        Ok(aligned)
    } else {
        Ok(0)
    }
}

/// 发起一次 HTTP 下载（支持 Range），返回本次结束后磁盘上的字节数。
#[cfg(not(target_arch = "wasm32"))]
async fn download_attempt(
    client: &reqwest::Client,
    entry: &PatchListEntry,
    dest: &Path,
    index: usize,
    state: &Arc<Mutex<ProgressState>>,
) -> Result<u64, PatchDownloadError> {
    use tokio::io::AsyncWriteExt;

    let offset = file_len(dest).await?;
    let mut request = client
        .get(&entry.url)
        .header("Accept", "*/*")
        // Range 偏移必须对应原始字节；不要让 reqwest 的 gzip 特性改变响应长度。
        .header("Accept-Encoding", "identity");
    if offset > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
    }

    let mut response = request
        .send()
        .await
        .map_err(|e| PatchDownloadError::Request {
            url: entry.url.clone(),
            source: e,
        })?;

    let status = response.status();

    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        if offset >= entry.length {
            return Ok(offset);
        }
        truncate_to(dest, 0).await?;
        state
            .lock()
            .expect("progress state poisoned")
            .set_done(index, 0);
        return Ok(0);
    }

    if !status.is_success() {
        return Err(PatchDownloadError::Http {
            url: entry.url.clone(),
            status,
        });
    }

    let append = status == reqwest::StatusCode::PARTIAL_CONTENT && offset > 0;
    if offset > 0 && !append {
        info!(
            url = %entry.url,
            "patch server ignored Range, restarting from zero"
        );
        state
            .lock()
            .expect("progress state poisoned")
            .set_done(index, 0);
    }

    let mut file = if append {
        tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(dest)
            .await
            .map_err(|e| PatchDownloadError::Io {
                path: dest.to_path_buf(),
                source: e,
            })?
    } else {
        tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dest)
            .await
            .map_err(|e| PatchDownloadError::Io {
                path: dest.to_path_buf(),
                source: e,
            })?
    };

    let mut received = if append { offset } else { 0 };
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| PatchDownloadError::Body {
            url: entry.url.clone(),
            source: e,
        })?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| PatchDownloadError::Io {
                path: dest.to_path_buf(),
                source: e,
            })?;
        received += chunk.len() as u64;
        state
            .lock()
            .expect("progress state poisoned")
            .set_done(index, received);
    }

    file.flush().await.map_err(|e| PatchDownloadError::Io {
        path: dest.to_path_buf(),
        source: e,
    })?;

    Ok(received)
}

/// 补丁下载错误。
#[derive(Debug, thiserror::Error)]
pub enum PatchDownloadError {
    #[error("semaphore acquire failed")]
    Semaphore,

    #[error("task join failed: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),

    #[error("HTTP request failed for {url}: {source}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("HTTP {status} for {url}")]
    Http {
        url: String,
        status: reqwest::StatusCode,
    },

    #[error("failed reading response body for {url}: {source}")]
    Body {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("IO error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("size mismatch for {url}: expected {expected}, got {actual}")]
    SizeMismatch {
        url: String,
        expected: u64,
        actual: u64,
    },

    #[error("SHA1 verification failed for {file} ({url})")]
    HashMismatch { file: PathBuf, url: String },

    #[error("download failed after {attempts} attempts for {url}")]
    RetryExhausted { url: String, attempts: usize },

    #[error("patch has more data blocks than provided hashes (block {index})")]
    TooManyBlocks { index: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha1::{Digest, Sha1};
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn entry_for(data: &[u8], block_size: usize) -> PatchListEntry {
        let hashes: Vec<String> = data
            .chunks(block_size)
            .map(|b| format!("{:x}", Sha1::digest(b)))
            .collect();
        PatchListEntry {
            version: "test".to_string(),
            url: "http://test/patch".to_string(),
            hash_type: "sha1".to_string(),
            hash_block_size: block_size as u64,
            hashes,
            length: data.len() as u64,
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("xl-rs-verify-test-{n}-{name}"))
    }

    #[test]
    fn test_verify_sha1_ok() {
        let data = b"hello world patch data, more than one block";
        let entry = entry_for(data, 16);
        let dir = temp_dir("xl-rs-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("patch.bin");
        std::fs::write(&path, data).unwrap();
        assert!(verify_patch_sha1(&path, &entry).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_sha1_mismatch() {
        let data = b"hello world patch data, more than one block";
        let entry = entry_for(data, 16);
        let dir = temp_dir("xl-rs-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("patch.bin");
        std::fs::write(&path, b"hello world patch data, DIFFERENT block").unwrap();
        assert!(!verify_patch_sha1(&path, &entry).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_sha1_wrong_length() {
        let data = b"hello world patch data, more than one block";
        let entry = entry_for(data, 16);
        let dir = temp_dir("xl-rs-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("patch.bin");
        std::fs::write(&path, b"short").unwrap();
        assert!(!verify_patch_sha1(&path, &entry).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_boot_patch_no_hash() {
        let dir = temp_dir("xl-rs-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("boot.patch");
        std::fs::write(&path, b"anything").unwrap();
        let entry = PatchListEntry {
            version: "boot".to_string(),
            url: "http://test/boot".to_string(),
            hash_type: String::new(),
            hash_block_size: 0,
            hashes: Vec::new(),
            length: 8,
        };
        // 无 hash 信息 → 跳过校验返回 true
        assert!(verify_patch_sha1(&path, &entry).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_boot_patch_wrong_length() {
        let dir = temp_dir("xl-rs-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("boot.patch");
        std::fs::write(&path, b"short").unwrap();
        let entry = PatchListEntry {
            version: "boot".to_string(),
            url: "http://test/boot".to_string(),
            hash_type: String::new(),
            hash_block_size: 0,
            hashes: Vec::new(),
            length: 8,
        };
        assert!(!verify_patch_sha1(&path, &entry).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_patch_cache_path_includes_url_identity() {
        let data = b"same patch metadata";
        let mut first = entry_for(data, 16);
        first.version = "2026.07.16.0001.0000".to_string();
        first.url = "http://patch/game/D2026.07.16.0001.0000.patch".to_string();

        let mut second = first.clone();
        second.url = "http://patch/game/ex5/D2026.07.16.0001.0000.patch".to_string();

        let dir = Path::new("/patches");
        let first_path = patch_cache_path(dir, &first);
        let second_path = patch_cache_path(dir, &second);
        assert_ne!(first_path, second_path);
        assert!(
            first_path
                .to_string_lossy()
                .contains("D2026.07.16.0001.0000.patch")
        );
        assert!(
            second_path
                .to_string_lossy()
                .contains("D2026.07.16.0001.0000.patch")
        );
    }

    #[test]
    fn test_verify_patch_prefix_detects_corrupt_prefix() {
        let data: Vec<u8> = (0..256).map(|i| (i % 251) as u8).collect();
        let entry = entry_for(&data, 16);
        let dir = temp_dir("xl-rs-prefix-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.patch");

        std::fs::write(&path, &data[..64]).unwrap();
        assert!(verify_patch_prefix(&path, &entry, 64).unwrap());

        let mut corrupt = data[..64].to_vec();
        corrupt[0] ^= 0xff;
        std::fs::write(&path, &corrupt).unwrap();
        assert!(!verify_patch_prefix(&path, &entry, 64).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_progress_state_counts_skipped_and_active() {
        let entry = entry_for(b"0123456789", 5);
        let mut state = ProgressState::new(20);

        state.ensure_active(0, &entry, 4);
        assert_eq!(state.snapshot().done, 4);
        assert_eq!(state.snapshot().active[0].phase, PatchPhase::Downloading);

        state.set_done(0, 7);
        state.set_phase(0, PatchPhase::Verifying);
        assert_eq!(state.snapshot().done, 7);
        assert_eq!(state.snapshot().active[0].phase, PatchPhase::Verifying);

        state.finish(0, false);
        assert_eq!(state.snapshot().done, 10);

        state.ensure_active(1, &entry, 10);
        state.finish(1, true);
        let snapshot = state.snapshot();
        assert_eq!(snapshot.done, 20);
        assert_eq!(snapshot.completed, 1);
        assert_eq!(snapshot.skipped, 1);
        assert!(snapshot.active.is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn test_client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    /// 极简本地 HTTP 服务：按 `Range` 返回 206；`support_range=false` 时故意忽略 Range。
    /// `drop_first_body=true` 时第一次响应只发一半就断开，用来测试失败后续传。
    #[cfg(not(target_arch = "wasm32"))]
    async fn spawn_range_server(
        body: Vec<u8>,
        support_range: bool,
        drop_first_body: bool,
    ) -> (String, Arc<Mutex<Vec<u64>>>, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_server = seen.clone();
        let requests = Arc::new(AtomicUsize::new(0));

        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                let seen = seen_server.clone();
                let first_request = requests.fetch_add(1, Ordering::SeqCst) == 0;
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 1024];
                    loop {
                        let n = socket.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        request.extend_from_slice(&buf[..n]);
                        if request.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }

                    let request = String::from_utf8_lossy(&request);
                    let range_start = request.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if !name.eq_ignore_ascii_case("range") {
                            return None;
                        }
                        value
                            .trim()
                            .strip_prefix("bytes=")?
                            .split('-')
                            .next()?
                            .parse::<u64>()
                            .ok()
                    });
                    if let Some(start) = range_start {
                        seen.lock().unwrap().push(start);
                    }

                    let use_range = support_range && range_start.is_some();
                    let start = if use_range {
                        (range_start.unwrap() as usize).min(body.len())
                    } else {
                        0
                    };
                    let (status, reason, payload, content_range) =
                        if use_range && start < body.len() {
                            (
                                206,
                                "Partial Content",
                                &body[start..],
                                Some(format!(
                                    "Content-Range: bytes {start}-{}/{}\r\n",
                                    body.len() - 1,
                                    body.len()
                                )),
                            )
                        } else {
                            (200, "OK", &body[..], None)
                        };

                    let mut response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\nAccept-Ranges: bytes\r\n",
                        payload.len()
                    );
                    if let Some(content_range) = content_range {
                        response.push_str(&content_range);
                    }
                    response.push_str("\r\n");

                    let _ = socket.write_all(response.as_bytes()).await;
                    if drop_first_body && first_request && !use_range {
                        let split = payload.len() / 2;
                        let _ = socket.write_all(&payload[..split]).await;
                    } else {
                        let _ = socket.write_all(payload).await;
                    }
                });
            }
        });

        (format!("http://{addr}/patch"), seen, handle)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn test_download_resumes_partial_file_with_range() {
        let payload: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let (url, seen, server) = spawn_range_server(payload.clone(), true, false).await;
        let mut entry = entry_for(&payload, 64);
        entry.url = url;

        let dir = temp_dir("xl-rs-resume-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = patch_cache_path(&dir, &entry);
        std::fs::write(&dest, &payload[..2048]).unwrap();

        let mut last = DownloadProgress::default();
        let summary = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            download_patches(
                &test_client(),
                std::slice::from_ref(&entry),
                &dir,
                1,
                |progress| last = progress,
            ),
        )
        .await
        .expect("download timed out")
        .expect("download failed");

        assert_eq!(summary.downloaded.len(), 1);
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        assert_eq!(seen.lock().unwrap().last().copied(), Some(2048));
        assert_eq!(last.total, payload.len() as u64);
        assert_eq!(last.done, last.total);
        assert_eq!(last.completed, 1);
        assert!(last.active.is_empty());

        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn test_download_restarts_when_server_ignores_range() {
        let payload: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let (url, seen, server) = spawn_range_server(payload.clone(), false, false).await;
        let mut entry = entry_for(&payload, 64);
        entry.url = url;

        let dir = temp_dir("xl-rs-no-range-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = patch_cache_path(&dir, &entry);
        std::fs::write(&dest, &payload[..2048]).unwrap();

        let summary = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            download_patches(
                &test_client(),
                std::slice::from_ref(&entry),
                &dir,
                1,
                |_| {},
            ),
        )
        .await
        .expect("download timed out")
        .expect("download failed");

        assert_eq!(summary.downloaded.len(), 1);
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        // 客户端确实带了 Range，但服务端返回 200，代码应自动从头写。
        assert_eq!(seen.lock().unwrap().last().copied(), Some(2048));

        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn test_download_restarts_corrupt_partial_prefix() {
        let payload: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let (url, seen, server) = spawn_range_server(payload.clone(), true, false).await;
        let mut entry = entry_for(&payload, 64);
        entry.url = url;

        let dir = temp_dir("xl-rs-corrupt-prefix-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = patch_cache_path(&dir, &entry);
        let mut corrupt = payload[..2048].to_vec();
        corrupt[0] ^= 0xff;
        std::fs::write(&dest, &corrupt).unwrap();

        let summary = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            download_patches(
                &test_client(),
                std::slice::from_ref(&entry),
                &dir,
                1,
                |_| {},
            ),
        )
        .await
        .expect("download timed out")
        .expect("download failed");

        assert_eq!(summary.downloaded.len(), 1);
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        // 前缀校验失败后应从头下载，不应带 Range。
        assert!(seen.lock().unwrap().is_empty());

        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn test_download_retries_and_resumes_after_connection_drop() {
        let payload: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let (url, seen, server) = spawn_range_server(payload.clone(), true, true).await;
        let mut entry = entry_for(&payload, 64);
        entry.url = url;

        let dir = temp_dir("xl-rs-retry-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = patch_cache_path(&dir, &entry);

        // 第一次响应会在 2048 字节处断开；客户端应自动 Range 续传。
        let summary = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            download_patches(
                &test_client(),
                std::slice::from_ref(&entry),
                &dir,
                1,
                |_| {},
            ),
        )
        .await
        .expect("download timed out")
        .expect("download failed");

        assert_eq!(summary.downloaded.len(), 1);
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        assert_eq!(seen.lock().unwrap().last().copied(), Some(2048));

        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn test_download_skips_complete_file_without_request() {
        let payload: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
        let (url, seen, server) = spawn_range_server(payload.clone(), true, false).await;
        let mut entry = entry_for(&payload, 64);
        entry.url = url;

        let dir = temp_dir("xl-rs-skip-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = patch_cache_path(&dir, &entry);
        std::fs::write(&dest, &payload).unwrap();

        let summary = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            download_patches(
                &test_client(),
                std::slice::from_ref(&entry),
                &dir,
                1,
                |_| {},
            ),
        )
        .await
        .expect("download timed out")
        .expect("download failed");

        assert_eq!(summary.skipped, 1);
        assert!(seen.lock().unwrap().is_empty());

        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
