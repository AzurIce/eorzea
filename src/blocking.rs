//! 将同步进程等待、解压和文件校验移出异步调用线程。

use std::io;

pub(crate) async fn run<F, T>(work: F) -> io::Result<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        // 保留日志上下文；不能把 Dioxus signal 或其读写 guard 交给 worker。
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || span.in_scope(work))
            .await
            .map_err(io::Error::other)
    }
    #[cfg(target_arch = "wasm32")]
    {
        // 浏览器仅用于界面预览，没有原生线程/进程能力。
        Ok(work())
    }
}

/// worker 只发送最新进度，回调始终在调用方线程执行，无无限增长的进度队列。
pub(crate) async fn with_progress<F, T>(
    work: F,
    mut on_progress: impl FnMut(u64, u64),
) -> io::Result<T>
where
    F: FnOnce(&mut dyn FnMut(u64, u64)) -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, mut rx) = tokio::sync::watch::channel(None);
        let task = run(move || {
            work(&mut |done, total| {
                tx.send_replace(Some((done, total)));
            })
        });
        tokio::pin!(task);
        loop {
            tokio::select! {
                result = &mut task => {
                    let latest = *rx.borrow_and_update();
                    if let Some((done, total)) = latest {
                        on_progress(done, total);
                    }
                    return result;
                }
                changed = rx.changed() => {
                    if changed.is_err() {
                        return task.await;
                    }
                    let latest = *rx.borrow_and_update();
                    if let Some((done, total)) = latest {
                        on_progress(done, total);
                    }
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        Ok(work(&mut on_progress))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_does_not_stall_caller() {
        let caller = std::thread::current().id();
        let (release, wait) = std::sync::mpsc::channel();
        let (started, ready) = tokio::sync::oneshot::channel();
        let work = run(move || {
            assert_ne!(std::thread::current().id(), caller);
            started.send(()).unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            42
        });
        let ui = async move {
            ready.await.unwrap();
            tokio::task::yield_now().await;
            release.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(work, ui);
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn progress_stays_on_caller_and_delivers_final_value() {
        let caller = std::thread::current().id();
        // 非 Send 回调（与 UI signal 一样）不能被移进 worker。
        let progress = std::rc::Rc::new(std::cell::Cell::new((0, 0)));
        let (release, wait) = std::sync::mpsc::channel();
        let result = with_progress(
            move |report| {
                assert_ne!(std::thread::current().id(), caller);
                report(1, 100);
                // 回调必须在 worker 完成之前就能运行。
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                for done in 2..=100 {
                    report(done, 100);
                }
                42
            },
            |done, total| {
                assert_eq!(std::thread::current().id(), caller);
                progress.set((done, total));
                if done == 1 {
                    release.send(()).unwrap();
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(result, 42);
        assert_eq!(progress.get(), (100, 100));
    }

    #[tokio::test]
    async fn worker_panic_is_an_error() {
        assert!(run(|| panic!("worker failure")).await.is_err());
    }
}
