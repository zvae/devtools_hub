//! 统一退出协调：唤醒可取消的后台任务、有界等待线程结束，并通知系统会话监听器收尾完成。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const CLEANUP_BUDGET: Duration = Duration::from_secs(3);
#[cfg(windows)]
pub const SESSION_WAIT: Duration = Duration::from_secs(4);
#[cfg(not(test))]
pub const EXIT_WATCHDOG: Duration = Duration::from_secs(6);

#[derive(Default)]
pub struct Shutdown {
    requested: AtomicBool,
    system_session: AtomicBool,
    exited: AtomicBool,
    complete: Mutex<bool>,
    changed: Condvar,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl Shutdown {
    /// 第一次请求返回 true；系统关机标记即使在普通退出已经开始后也不能丢失。
    pub fn request(&self, system_session: bool) -> bool {
        if system_session {
            self.system_session.store(true, Ordering::Release);
        }
        let _complete = self.complete.lock().unwrap_or_else(|p| p.into_inner());
        let first = !self.requested.swap(true, Ordering::AcqRel);
        self.changed.notify_all();
        first
    }

    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    pub fn system_session(&self) -> bool {
        self.system_session.load(Ordering::Acquire)
    }

    /// 代替长时间 sleep：退出请求会立即唤醒剪贴板轮询和更新检查。
    pub fn wait_requested(&self, timeout: Duration) -> bool {
        let complete = self.complete.lock().unwrap_or_else(|p| p.into_inner());
        drop(
            self.changed
                .wait_timeout_while(complete, timeout, |_| !self.requested())
                .unwrap_or_else(|p| p.into_inner()),
        );
        self.requested()
    }

    pub fn track(&self, worker: JoinHandle<()>) {
        let mut workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        // 及时回收已完成的短任务，避免 OCR、截屏或手动更新的句柄随使用次数增长。
        workers.retain(|worker| !worker.is_finished());
        workers.push(worker);
    }

    /// 只 join 已结束的线程；外部系统调用不能取消时最多等到共同截止时间。
    pub fn join_until(&self, deadline: Instant) -> Vec<String> {
        loop {
            let mut workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
            let mut index = 0;
            while index < workers.len() {
                if workers[index].is_finished() {
                    let worker = workers.swap_remove(index);
                    if worker.join().is_err() {
                        tracing::warn!("后台任务异常退出");
                    }
                } else {
                    index += 1;
                }
            }
            if workers.is_empty() || Instant::now() >= deadline {
                return workers
                    .iter()
                    .map(|worker| worker.thread().name().unwrap_or("未命名任务").to_owned())
                    .collect();
            }
            drop(workers);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn finish(&self) {
        *self.complete.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.changed.notify_all();
    }

    pub fn wait_finished(&self, timeout: Duration) -> bool {
        let complete = self.complete.lock().unwrap_or_else(|p| p.into_inner());
        let (complete, _) = self
            .changed
            .wait_timeout_while(complete, timeout, |done| !*done)
            .unwrap_or_else(|p| p.into_inner());
        *complete
    }

    /// 清理完成和事件循环结束是两件事；退出看门狗必须覆盖二者之间的阶段。
    pub fn mark_exited(&self) {
        let _complete = self.complete.lock().unwrap_or_else(|p| p.into_inner());
        self.exited.store(true, Ordering::Release);
        self.changed.notify_all();
    }

    pub fn wait_exited(&self, timeout: Duration) -> bool {
        let complete = self.complete.lock().unwrap_or_else(|p| p.into_inner());
        drop(
            self.changed
                .wait_timeout_while(complete, timeout, |_| !self.exited.load(Ordering::Acquire))
                .unwrap_or_else(|p| p.into_inner()),
        );
        self.exited.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};

    #[test]
    fn exit_is_idempotent_and_wakes_sleeping_workers() {
        let shutdown = Arc::new(Shutdown::default());
        let sleeping = shutdown.clone();
        let (tx, rx) = mpsc::channel();
        shutdown.track(std::thread::spawn(move || {
            tx.send(sleeping.wait_requested(Duration::from_secs(30)))
                .unwrap();
        }));
        assert!(shutdown.request(false));
        assert!(!shutdown.request(true));
        assert!(shutdown.system_session());
        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
        assert!(shutdown
            .join_until(Instant::now() + Duration::from_secs(1))
            .is_empty());
        assert!(!shutdown.wait_finished(Duration::ZERO));
        shutdown.finish();
        assert!(shutdown.wait_finished(Duration::ZERO));
        assert!(
            !shutdown.wait_exited(Duration::ZERO),
            "清理完成不能代替事件循环结束"
        );
        shutdown.mark_exited();
        assert!(shutdown.wait_exited(Duration::ZERO));
    }

    #[test]
    fn an_unresponsive_worker_does_not_make_join_unbounded() {
        let shutdown = Shutdown::default();
        let (tx, rx) = mpsc::channel();
        shutdown.track(
            std::thread::Builder::new()
                .name("blocked-worker".into())
                .spawn(move || {
                    let _ = rx.recv();
                })
                .unwrap(),
        );
        assert_eq!(shutdown.join_until(Instant::now()), ["blocked-worker"]);
        tx.send(()).unwrap();
        assert!(shutdown
            .join_until(Instant::now() + Duration::from_secs(1))
            .is_empty());
    }
}
