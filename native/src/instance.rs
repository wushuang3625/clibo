//! A second launch asks the lock owner to open instead of failing silently.
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub struct Instance {
    _lock: File,
    request: PathBuf,
    stopped: Arc<AtomicBool>,
}

impl Instance {
    pub fn acquire(path: &Path) -> Result<Option<Self>, String> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.join("native.lock"))
            .map_err(|e| e.to_string())?;
        let request = path.join("native-open.request");
        match fs2::FileExt::try_lock_exclusive(&lock) {
            Ok(()) => Ok(Some(Self {
                _lock: lock,
                request,
                stopped: Arc::new(AtomicBool::new(false)),
            })),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock || e.raw_os_error() == Some(33) =>
            {
                std::fs::write(request, b"open")
                    .map_err(|e| format!("无法唤起已运行的 Clibo：{e}"))?;
                Ok(None)
            }
            Err(e) => Err(format!("无法锁定 Clibo 数据目录：{e}")),
        }
    }

    pub fn listen(&self, wake: impl Fn() + Send + 'static) {
        let request = self.request.clone();
        let stopped = self.stopped.clone();
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                if std::fs::remove_file(&request).is_ok() && !stopped.load(Ordering::Acquire) {
                    wake();
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_launch_requests_open_and_exit_releases_lock() {
        let dir = tempfile::tempdir().unwrap();
        let first = Instance::acquire(dir.path()).unwrap().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        first.listen(move || {
            let _ = tx.send(());
        });
        assert!(Instance::acquire(dir.path()).unwrap().is_none());
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!dir.path().join("native-open.request").exists());
        drop(first);
        assert!(Instance::acquire(dir.path()).unwrap().is_some());
    }
}
