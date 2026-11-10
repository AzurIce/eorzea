//! 有界下载队列 + 严格按序应用。补丁删除并持久化队头后才能推进。

use super::patch_manager::DownloadProgress;
use super::{GameFileError, GameFileManager, InstallProgress};
use eorzea_auth::PatchListEntry;
use std::path::{Path, PathBuf};

/// 两条独立进度，不会把“已下载”误认为“已安装”。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateProgress {
    pub download: DownloadProgress,
    pub install: InstallProgress,
    pub waiting_for_download: bool,
}

#[derive(Debug, Clone)]
pub struct PendingUpdate {
    pub patches: Vec<PatchListEntry>,
    pub patch_dir: PathBuf,
}

fn plan_path(root: &Path) -> PathBuf {
    root.join(".eorzea/update.json")
}

/// 未完成队列必须先恢复，不能依据可能已被 RemoveAll 删除的 .ver 启动游戏。
pub fn has_pending_update(root: &Path) -> bool {
    plan_path(root).exists()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

impl GameFileManager {
    /// 重新请求服务端补丁列表之前调用，优先恢复原队列（支持离线复用完整缓存）。
    pub async fn pending_update(
        &self,
        root: &Path,
    ) -> Result<Option<PendingUpdate>, GameFileError> {
        let root = root.to_path_buf();
        crate::blocking::run(move || {
            let plan = backend::load_plan(&root)?;
            Ok(plan.map(|plan| PendingUpdate {
                patches: plan.patches[plan.next..].to_vec(),
                patch_dir: plan.patch_dir,
            }))
        })
        .await
        .map_err(|e| GameFileError::Update(e.to_string()))?
    }

    /// 并发下载，队头就绪立即后台应用。成功提交并删除后释放预取窗口。
    pub async fn update(
        &self,
        patches: &[PatchListEntry],
        patch_dir: &Path,
        game_root: &Path,
        concurrency: usize,
        on_progress: impl FnMut(UpdateProgress),
    ) -> Result<super::InstallSummary, GameFileError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            backend::run(
                self,
                patches,
                patch_dir,
                game_root,
                concurrency,
                on_progress,
            )
            .await
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (patches, patch_dir, game_root, concurrency, on_progress);
            Err(GameFileError::Update("浏览器预览不支持安装游戏补丁".into()))
        }
    }
}

mod backend {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::fs;
    use std::io;
    #[cfg(not(target_arch = "wasm32"))]
    use std::{fs::File, io::Write};

    #[derive(Clone, Serialize, Deserialize)]
    pub(super) struct Plan {
        pub patches: Vec<PatchListEntry>,
        pub patch_dir: PathBuf,
        pub next: usize,
    }

    fn io_error(path: &Path, source: io::Error) -> GameFileError {
        GameFileError::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    pub(super) fn load_plan(root: &Path) -> Result<Option<Plan>, GameFileError> {
        let path = plan_path(root);
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error(&path, e)),
        };
        let plan: Plan = serde_json::from_slice(&data)
            .map_err(|e| GameFileError::Update(format!("更新队列损坏 {}: {e}", path.display())))?;
        if plan.next > plan.patches.len() || !plan.patch_dir.is_absolute() {
            return Err(GameFileError::Update("更新队列的游标或缓存目录无效".into()));
        }
        Ok(Some(plan))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn sync_dir(path: &Path) -> Result<(), GameFileError> {
        #[cfg(unix)]
        File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|e| io_error(path, e))?;
        let _ = path;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), GameFileError> {
        let parent = path.parent().unwrap();
        let mut created = Vec::new();
        for dir in parent.ancestors() {
            if dir.exists() {
                break;
            }
            created.push(dir);
        }
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        for dir in created.into_iter().rev() {
            if let Some(parent) = dir.parent() {
                sync_dir(parent)?;
            }
        }
        let tmp = path.with_extension("update-tmp");
        let mut file = File::create(&tmp).map_err(|e| io_error(&tmp, e))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| io_error(&tmp, e))?;
        drop(file);
        fs::rename(&tmp, path).map_err(|e| io_error(path, e))?;
        sync_dir(parent)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn save_plan(root: &Path, plan: &Plan) -> Result<(), GameFileError> {
        let bytes = serde_json::to_vec(plan).map_err(|e| GameFileError::Update(e.to_string()))?;
        atomic_write(&plan_path(root), &bytes)
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod native {
        use super::*;
        use crate::game_files::patch_manager::{
            ProgressState, download_for_update, patch_cache_path,
        };
        use crate::game_files::{
            InstallPatchProgress, InstallPhase, InstallSummary, Repo, repo_from_url, zpatch,
        };
        use std::collections::BTreeSet;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;
        use tokio::sync::Semaphore;
        use tokio::task::{JoinHandle, JoinSet};

        fn prepare(
            root: &Path,
            patches: Vec<PatchListEntry>,
            patch_dir: PathBuf,
        ) -> Result<(Plan, Arc<File>), GameFileError> {
            fs::create_dir_all(root.join(".eorzea")).map_err(|e| io_error(root, e))?;
            sync_dir(root)?;
            let lock_path = root.join(".eorzea/update.lock");
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .map_err(|e| io_error(&lock_path, e))?;
            lock.try_lock()
                .map_err(|e| GameFileError::Update(format!("游戏目录已有更新任务: {e}")))?;
            let plan = match load_plan(root)? {
                Some(plan) => {
                    if patches != plan.patches[plan.next..] && patches != plan.patches {
                        return Err(GameFileError::Update(
                            "存在未完成的更新，请先恢复原补丁队列".into(),
                        ));
                    }
                    plan
                }
                None => {
                    fs::create_dir_all(&patch_dir).map_err(|e| io_error(&patch_dir, e))?;
                    let plan = Plan {
                        patches,
                        patch_dir: fs::canonicalize(&patch_dir)
                            .map_err(|e| io_error(&patch_dir, e))?,
                        next: 0,
                    };
                    save_plan(root, &plan)?;
                    plan
                }
            };
            fs::create_dir_all(&plan.patch_dir).map_err(|e| io_error(&plan.patch_dir, e))?;
            Ok((plan, Arc::new(lock)))
        }

        fn apply_one(
            root: &Path,
            plan: &mut Plan,
            state: &Mutex<InstallProgress>,
        ) -> Result<(), GameFileError> {
            let index = plan.next;
            let patch = &plan.patches[index];
            let repo = repo_from_url(&patch.url);
            let file = patch_cache_path(&plan.patch_dir, patch);
            let before: u64 = plan.patches[..index].iter().map(|p| p.length).sum();
            let report = |phase, done: u64| {
                let mut state = state.lock().unwrap();
                state.done = before + done.min(patch.length);
                state.current = Some(InstallPatchProgress {
                    index,
                    version: patch.version.clone(),
                    phase,
                    done: done.min(patch.length),
                    total: patch.length,
                });
            };
            // download_for_update 已完成校验；持久化缓存后才开始改游戏。
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&file)
                .and_then(|f| f.sync_all())
                .map_err(|e| io_error(&file, e))?;
            report(InstallPhase::Applying, 0);
            let ctx = zpatch::apply::apply_patch_file_with_progress(
                &file,
                &root.join(repo.install_subdir()),
                |done, _| {
                    report(InstallPhase::Applying, done);
                },
            )?;
            // apply 已检查 EOF 并 sync_all；提交版本之前关闭所有写入句柄。
            drop(ctx);
            report(InstallPhase::Committing, patch.length);
            atomic_write(&repo.ver_path(root), patch.version.as_bytes())?;
            // 删除失败就不推进：禁止后续补丁覆盖后再重放旧补丁。
            fs::remove_file(&file).map_err(|e| io_error(&file, e))?;
            sync_dir(&plan.patch_dir)?;
            plan.next += 1;
            save_plan(root, plan)?;
            let mut state = state.lock().unwrap();
            state.completed = plan.next;
            state.current = None;
            Ok(())
        }

        fn finish(root: &Path) -> Result<(), GameFileError> {
            for repo in [
                Repo::Boot,
                Repo::Ffxiv,
                Repo::Ex1,
                Repo::Ex2,
                Repo::Ex3,
                Repo::Ex4,
                Repo::Ex5,
            ] {
                let ver = repo.ver_path(root);
                match fs::read(&ver) {
                    Ok(data) => atomic_write(&repo.bck_path(root), &data)?,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io_error(&ver, e)),
                }
            }
            let path = plan_path(root);
            fs::remove_file(&path).map_err(|e| io_error(&path, e))?;
            sync_dir(path.parent().unwrap())
        }

        pub(crate) async fn run(
            manager: &GameFileManager,
            patches: &[PatchListEntry],
            patch_dir: &Path,
            root: &Path,
            concurrency: usize,
            mut on_progress: impl FnMut(UpdateProgress),
        ) -> Result<InstallSummary, GameFileError> {
            let setup_root = root.to_path_buf();
            let input = patches.to_vec();
            let patch_dir = patch_dir.to_path_buf();
            let (root, mut plan, lock) = crate::blocking::run(move || {
                fs::create_dir_all(&setup_root).map_err(|e| io_error(&setup_root, e))?;
                let root = fs::canonicalize(&setup_root).map_err(|e| io_error(&setup_root, e))?;
                let (plan, lock) = prepare(&root, input, patch_dir)?;
                Ok::<_, GameFileError>((root, plan, lock))
            })
            .await
            .map_err(|e| io_error(root, e))??;
            let start = plan.next;
            let total: u64 = plan.patches.iter().map(|p| p.length).sum();
            let mut progress = ProgressState::new(total);
            progress.resume(&plan.patches[..start]);
            let download = Arc::new(Mutex::new(progress));
            let install = Arc::new(Mutex::new(InstallProgress {
                done: plan.patches[..start].iter().map(|p| p.length).sum(),
                total,
                completed: start,
                total_patches: plan.patches.len(),
                current: None,
            }));
            let snapshot = |waiting| UpdateProgress {
                download: download.lock().unwrap().snapshot(),
                install: install.lock().unwrap().clone(),
                waiting_for_download: waiting,
            };
            let concurrency = concurrency.max(1);
            // 包含正在应用的补丁；最多预取 concurrency 份，不会无限积压。
            let window = concurrency.saturating_add(1);
            let semaphore = Arc::new(Semaphore::new(concurrency));
            let mut downloads = JoinSet::new();
            let mut applying: Option<JoinHandle<Result<(), GameFileError>>> = None;
            let mut ready = BTreeSet::new();
            let mut scheduled = start;
            let mut failure = None;
            let mut ticker = tokio::time::interval(Duration::from_millis(120));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            on_progress(snapshot(plan.next < plan.patches.len()));

            loop {
                if failure.is_some() && applying.is_none() {
                    break;
                }
                if failure.is_none() {
                    while scheduled < plan.patches.len().min(plan.next.saturating_add(window)) {
                        // 在主队列按序领取槽位，不能让乱序被调度的 task 抢走队头槽位。
                        let Ok(permit) = semaphore.clone().try_acquire_owned() else {
                            break;
                        };
                        let index = scheduled;
                        let client = manager.client.clone();
                        let entry = plan.patches[index].clone();
                        let dir = plan.patch_dir.clone();
                        let state = download.clone();
                        let slots = semaphore.clone();
                        downloads.spawn(async move {
                            download_for_update(client, entry, dir, index, state, permit, slots)
                                .await
                                .map(|_| index)
                        });
                        scheduled += 1;
                    }
                    if applying.is_none() && ready.remove(&plan.next) {
                        let mut work_plan = plan.clone();
                        let root = root.clone();
                        let state = install.clone();
                        let guard = lock.clone();
                        let span = tracing::Span::current();
                        applying = Some(tokio::task::spawn_blocking(move || {
                            let _guard = guard;
                            span.in_scope(|| apply_one(&root, &mut work_plan, &state))
                        }));
                    }
                    if plan.next == plan.patches.len() {
                        break;
                    }
                }

                tokio::select! {
                    _ = ticker.tick() => {},
                    joined = downloads.join_next(), if !downloads.is_empty() => {
                        match joined.unwrap() {
                            Ok(Ok(index)) => { ready.insert(index); },
                            Ok(Err(error)) if failure.is_none() => {
                                failure = Some(GameFileError::Download(error));
                                downloads.abort_all();
                            },
                            Err(error) if failure.is_none() => {
                                failure = Some(GameFileError::Update(error.to_string()));
                                downloads.abort_all();
                            },
                            _ => {},
                        }
                    },
                    result = async { applying.as_mut().unwrap().await }, if applying.is_some() => {
                        applying = None;
                        match result {
                            Ok(Ok(())) => plan.next += 1,
                            Ok(Err(error)) => { failure = Some(error); downloads.abort_all(); },
                            Err(error) => { failure = Some(GameFileError::Update(error.to_string())); downloads.abort_all(); },
                        }
                    },
                }
                on_progress(snapshot(
                    applying.is_none() && plan.next < plan.patches.len() && failure.is_none(),
                ));
            }
            downloads.abort_all();
            while downloads.join_next().await.is_some() {}
            on_progress(snapshot(false));
            if let Some(error) = failure {
                return Err(error);
            }
            let done_root = root.clone();
            let guard = lock.clone();
            crate::blocking::run(move || {
                let _guard = guard;
                finish(&done_root)
            })
            .await
            .map_err(|e| io_error(&root, e))??;
            Ok(InstallSummary {
                installed: plan.patches[start..]
                    .iter()
                    .map(|p| p.version.clone())
                    .collect(),
                skipped: 0,
            })
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) use native::run;
}
