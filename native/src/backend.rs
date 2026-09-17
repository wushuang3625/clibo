use crate::{model::*, platform, store::Store};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub fn data_dir() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CLIBO_DATA_DIR") {
        return Ok(path.into());
    }
    #[cfg(windows)]
    let path = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("local.clibo.native"));
    #[cfg(target_os = "macos")]
    let path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join("Library/Application Support/local.clibo.native"));
    path.ok_or_else(|| "无法确定本地数据目录".into())
}

pub struct Backend {
    pub store: Mutex<Store>,
    pub status: Mutex<String>,
    pub revision: AtomicU64,
    pub visible: AtomicBool,
    pub busy: AtomicBool,
    pub window: AtomicUsize,
    gate: Mutex<()>,
    ctx: egui::Context,
}
impl Backend {
    pub fn new(store: Store, ctx: egui::Context) -> Arc<Self> {
        let status = store
            .recovery_status()
            .unwrap_or_else(|| "本机加密保存 · 默认暂停记录".into());
        Arc::new(Self {
            store: Mutex::new(store),
            status: Mutex::new(status),
            revision: AtomicU64::new(1),
            visible: AtomicBool::new(true),
            busy: AtomicBool::new(false),
            window: AtomicUsize::new(0),
            gate: Mutex::new(()),
            ctx,
        })
    }
    pub fn report(&self, result: Result<(), String>) {
        *self.status.lock().unwrap() = result.err().unwrap_or_else(|| "已保存".into());
        self.changed();
    }
    pub fn changed(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
        if self.visible.load(Ordering::Relaxed) {
            self.ctx.request_repaint();
        }
    }
    /// Read the clipboard immediately and promote its current contents to the
    /// front of history. This complements the asynchronous OS notification:
    /// opening the panel immediately after Copy must still select that item.
    pub fn capture_current(&self) -> Result<Option<String>, String> {
        let _gate = self.gate.lock().unwrap();
        let settings = self.store.lock().unwrap().settings.clone();
        if !settings.enabled {
            return Ok(None);
        }
        let Some(capture) = platform::read(&settings)? else {
            return Ok(None);
        };
        let mut store = self.store.lock().unwrap();
        if !store.settings.enabled
            || (!store.settings.capture_images && capture.png.is_some())
            || (store.settings.detect_sensitive
                && capture.text.as_deref().is_some_and(is_sensitive))
            || store
                .settings
                .excluded_apps
                .iter()
                .any(|s| s.eq_ignore_ascii_case(&capture.source))
        {
            return Ok(None);
        }
        if !store.insert(capture)? {
            return Ok(None);
        }
        let id = store.entries.first().map(|entry| entry.view.id.clone());
        drop(store);
        self.changed();
        Ok(id)
    }
    pub fn monitor(self: &Arc<Self>) {
        let this = self.clone();
        std::thread::spawn(move || {
            let rx = match platform::subscribe() {
                Ok(rx) => rx,
                Err(e) => {
                    this.report(Err(e));
                    return;
                }
            };
            loop {
                match rx.recv_timeout(Duration::from_secs(60)) {
                    Ok(()) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let mut store = this.store.lock().unwrap();
                        let before = store.entries.len();
                        let result = store.cleanup();
                        let changed = before != store.entries.len();
                        drop(store);
                        if result.is_err() {
                            this.report(result);
                        } else if changed {
                            this.changed();
                        }
                        continue;
                    }
                    Err(_) => {
                        this.report(Err("剪贴板监听已停止，请重启".into()));
                        break;
                    }
                }
                if let Err(error) = this.capture_current() {
                    this.report(Err(error));
                }
            }
        });
    }
    #[allow(clippy::too_many_arguments)]
    pub fn use_clip(
        self: &Arc<Self>,
        id: String,
        paste: bool,
        advance: bool,
        merged: bool,
        transform: String,
        owner: usize,
        target: usize,
    ) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let _gate = this.gate.lock().unwrap();
                let (text, png, used_id) = {
                    let store = this.store.lock().unwrap();
                    let used_id = if advance {
                        store.organizer.queue.first().cloned().ok_or("队列为空")?
                    } else {
                        id
                    };
                    if merged {
                        (Some(store.merged_queue_text()?), None, used_id)
                    } else {
                        let entry = store
                            .entries
                            .iter()
                            .find(|e| e.view.id == used_id)
                            .ok_or("记录已不存在")?;
                        if entry.view.kind == "image" {
                            let png = store.image(&used_id)?;
                            (None, Some(png), used_id)
                        } else {
                            let text = store
                                .secret(&used_id)?
                                .text
                                .as_deref()
                                .map(|t| crate::text_tools::transform(t, &transform))
                                .transpose()?;
                            (text, None, used_id)
                        }
                    }
                };
                platform::write(text.as_deref(), png.as_deref(), owner)?;
                if paste {
                    if !platform::valid_target(target, owner) {
                        return Err("已复制，请切换到目标应用手动粘贴".into());
                    }
                    this.ctx
                        .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    this.visible.store(false, Ordering::Relaxed);
                    this.ctx.request_repaint();
                    std::thread::sleep(Duration::from_millis(90));
                    if !platform::activate(target) {
                        return Err("已复制；目标激活失败，请手动粘贴".into());
                    }
                    std::thread::sleep(Duration::from_millis(70));
                    platform::paste(target)?;
                    if advance {
                        let mut store = this.store.lock().unwrap();
                        let mut ids = store.organizer.queue.clone();
                        // Do not advance a queue edited while a paste was in flight.
                        if ids.first() == Some(&used_id) {
                            ids.remove(0);
                            store.queue(ids)?;
                        }
                    }
                }
                Ok(())
            })();
            let failed = result.is_err();
            *this.status.lock().unwrap() = result.err().unwrap_or_else(|| {
                if paste {
                    "已发出粘贴按键，请在目标应用确认".into()
                } else {
                    "已复制".into()
                }
            });
            this.busy.store(false, Ordering::SeqCst);
            if failed && paste {
                this.visible.store(true, Ordering::Relaxed);
                this.ctx
                    .send_viewport_cmd(egui::ViewportCommand::Visible(true));
                this.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            this.changed();
        });
    }
}

pub fn matching_ids(store: &Store, query: &str, filter: &str, group: &str) -> Vec<String> {
    let query = query.trim().to_lowercase();
    let groups: Vec<_> = store
        .organizer
        .groups
        .iter()
        .filter(|g| g.name.to_lowercase().contains(&query))
        .map(|g| &g.id)
        .collect();
    let positions: std::collections::HashMap<_, _> = store
        .organizer
        .queue
        .iter()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect();
    let mut entries: Vec<_> = store
        .entries
        .iter()
        .filter(|e| {
            (group.is_empty() || e.view.group_ids.iter().any(|g| g == group))
                && (filter != "queue" || positions.contains_key(&e.view.id))
                && (matches(e, &query, filter)
                    || (matches(e, "", filter)
                        && e.view.group_ids.iter().any(|g| groups.contains(&g))))
        })
        .collect();
    if filter == "queue" {
        entries.sort_by_key(|e| positions.get(&e.view.id));
    }
    entries.iter().map(|e| e.view.id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_order_group_search_and_filters_use_shared_core() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("test.db")).unwrap();
        for text in ["first 中文", "second"] {
            s.insert(Captured {
                text: Some(text.into()),
                png: None,
                source: "test.exe".into(),
            })
            .unwrap();
        }
        let ids: Vec<_> = s.entries.iter().map(|e| e.view.id.clone()).collect();
        s.queue(vec![ids[1].clone(), ids[0].clone()]).unwrap();
        assert_eq!(
            matching_ids(&s, "", "queue", ""),
            vec![ids[1].clone(), ids[0].clone()]
        );
        assert_eq!(matching_ids(&s, "中文", "all", "").len(), 1);
        assert!(matching_ids(&s, "", "image", "").is_empty());
        s.group("", "工作", false).unwrap();
        let group = s.organizer.groups[0].id.clone();
        s.annotate(&ids[0], &[group], "note").unwrap();
        assert_eq!(matching_ids(&s, "工作", "all", ""), vec![ids[0].clone()]);
    }
}
