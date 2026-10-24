use serde::Serialize;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub phase: String,
    pub completed: u64,
    pub total: u64,
    pub current: String,
}

struct Inner {
    state: AtomicU8,
    completed: AtomicU64,
    total: AtomicU64,
    phase: Mutex<(String, String, Instant)>,
    callback: Box<dyn Fn(Progress) + Send + Sync>,
}

/// 取消与提交使用同一原子状态门，提交开始后不再接受取消。
#[derive(Clone)]
pub struct Control {
    inner: Arc<Inner>,
    pub(crate) recovery_dir: PathBuf,
}

impl Control {
    pub fn new(callback: impl Fn(Progress) + Send + Sync + 'static) -> anyhow::Result<Self> {
        Ok(Self::with_recovery_dir(
            super::recovery::default_recovery_dir()?,
            callback,
        ))
    }

    pub fn with_recovery_dir(
        path: PathBuf,
        callback: impl Fn(Progress) + Send + Sync + 'static,
    ) -> Self {
        Self {
            recovery_dir: path,
            inner: Arc::new(Inner {
                state: AtomicU8::new(0),
                completed: AtomicU64::new(0),
                total: AtomicU64::new(0),
                phase: Mutex::new(("scanning".into(), String::new(), Instant::now())),
                callback: Box::new(callback),
            }),
        }
    }

    pub fn cancel(&self) -> bool {
        self.inner
            .state
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            || self.inner.state.load(Ordering::SeqCst) == 1
    }

    pub(crate) fn check(&self) -> io::Result<()> {
        if self.inner.state.load(Ordering::SeqCst) == 1 {
            return Err(io::Error::other("操作已取消，未提交输出"));
        }
        Ok(())
    }

    pub(crate) fn begin_commit(&self) -> anyhow::Result<()> {
        self.inner
            .state
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| anyhow::anyhow!("操作已取消，未提交输出"))?;
        self.phase("committing", "");
        Ok(())
    }

    pub(crate) fn total(&self, total: u64) {
        self.inner.total.store(total, Ordering::Relaxed);
        self.inner.completed.store(0, Ordering::Relaxed);
    }

    pub(crate) fn phase(&self, phase: &str, current: &str) {
        let snapshot = {
            let mut state = self.inner.phase.lock().expect("progress mutex poisoned");
            let notify = state.0 != phase || state.2.elapsed() >= Duration::from_millis(100);
            state.0 = phase.into();
            state.1 = current.into();
            if !notify {
                return;
            }
            state.2 = Instant::now();
            self.snapshot(&state.0, &state.1)
        };
        (self.inner.callback)(snapshot);
    }

    pub(crate) fn current(&self, current: &str) {
        let phase = self
            .inner
            .phase
            .lock()
            .expect("progress mutex poisoned")
            .0
            .clone();
        self.phase(&phase, current);
    }

    pub(crate) fn advance(&self, bytes: u64) {
        self.inner.completed.fetch_add(bytes, Ordering::Relaxed);
        let snapshot = {
            let mut state = self.inner.phase.lock().expect("progress mutex poisoned");
            if state.2.elapsed() < Duration::from_millis(100) {
                None
            } else {
                state.2 = Instant::now();
                Some(self.snapshot(&state.0, &state.1))
            }
        };
        if let Some(snapshot) = snapshot {
            (self.inner.callback)(snapshot);
        }
    }

    fn snapshot(&self, phase: &str, current: &str) -> Progress {
        Progress {
            phase: phase.into(),
            current: current.into(),
            completed: self.inner.completed.load(Ordering::Relaxed),
            total: self.inner.total.load(Ordering::Relaxed),
        }
    }
}
