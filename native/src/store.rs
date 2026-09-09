use crate::{crypto, model::*};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub struct Store {
    db: Connection,
    pub entries: Vec<Entry>,
    pub settings: Settings,
    pub organizer: Organizer,
}
fn db_err(_: rusqlite::Error) -> String {
    "本地历史读写失败，请检查磁盘空间与文件权限".into()
}
impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        let db = Connection::open(path).map_err(db_err)?;
        db.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(db_err)?;
        let version: i32 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(db_err)?;
        if version > 4 {
            return Err("历史由更新版本创建，请使用更新的 Clibo 打开".into());
        }
        if (1..4).contains(&version) {
            // VACUUM INTO creates a consistent encrypted snapshot including WAL.
            // Abort the upgrade if its recovery copy cannot be created.
            let backup = path.with_file_name(format!("history.pre-v4-{}.db", uuid::Uuid::new_v4()));
            db.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])
                .map_err(|_| "升级前备份失败，请检查磁盘空间与目录权限；历史未升级")?;
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA secure_delete=ON; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS clips(id TEXT PRIMARY KEY,kind TEXT NOT NULL,copied_at INTEGER NOT NULL,pinned INTEGER NOT NULL,bytes INTEGER NOT NULL,secret BLOB NOT NULL,image BLOB);
            CREATE TABLE IF NOT EXISTS config(id INTEGER PRIMARY KEY CHECK(id=1),sealed BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS organizer(id INTEGER PRIMARY KEY CHECK(id=1),sealed BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS thumbnails(id TEXT PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE,sealed BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS image_metadata(id TEXT PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE,width INTEGER NOT NULL,height INTEGER NOT NULL);").map_err(db_err)?;
        let organizer = match db
            .query_row("SELECT sealed FROM organizer WHERE id=1", [], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .optional()
            .map_err(db_err)?
        {
            Some(b) => serde_json::from_slice(&crypto::unprotect(&b)?)
                .map_err(|_| "分组数据损坏，原始数据已保留")?,
            None => Organizer::default(),
        };
        let sealed: Option<Vec<u8>> = db
            .query_row("SELECT sealed FROM config WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(db_err)?;
        let mut settings: Settings = match sealed {
            Some(b) => serde_json::from_slice(&crypto::unprotect(&b)?)
                .map_err(|_| "设置损坏，请保留数据并检查备份".to_string())?,
            None => Settings::default(),
        };
        settings.validate()?;
        // 默认呼出键由 Ctrl+Alt+V 迁移到 Ctrl+Shift+V：仅当保存的仍是旧默认值时
        // 一次性改写并立即落盘，用户自定义键位不受影响。
        if settings.hotkey.eq_ignore_ascii_case("Ctrl+Alt+V") {
            settings.hotkey = "Ctrl+Shift+V".into();
            let bytes = serde_json::to_vec(&settings).map_err(|_| "设置序列化失败".to_string())?;
            let sealed = crypto::protect(&bytes)?;
            db.execute(
                "INSERT INTO config(id,sealed) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET sealed=excluded.sealed",
                [sealed],
            )
            .map_err(db_err)?;
        }
        let mut entries = Vec::new();
        {
            let mut stmt=db.prepare("SELECT id,kind,copied_at,pinned,bytes,secret FROM clips ORDER BY copied_at DESC").map_err(db_err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, usize>(4)?,
                        r.get::<_, Vec<u8>>(5)?,
                    ))
                })
                .map_err(db_err)?;
            for row in rows {
                let (id, kind, copied_at, pinned, bytes, sealed) = row.map_err(db_err)?;
                let secret: Secret = serde_json::from_slice(&crypto::unprotect(&sealed)?)
                    .map_err(|_| "历史内容损坏；原始数据已保留".to_string())?;
                let preview = preview(&secret, &kind);
                // Only the view, search copy, and fingerprint stay resident;
                // the secret itself is dropped and re-read from SQLite on demand.
                entries.push(Entry {
                    search: secret.search_fields(),
                    view: ClipView {
                        id,
                        kind,
                        copied_at,
                        pinned,
                        bytes,
                        source: secret.source.clone(),
                        preview,
                        group_ids: secret.group_ids.clone(),
                        note: secret.note.clone(),
                    },
                    fingerprint: secret.fingerprint,
                });
            }
        }
        let mut store = Self {
            db,
            entries,
            settings,
            organizer,
        };
        store.cleanup()?;
        store
            .db
            .execute_batch("PRAGMA user_version=4;")
            .map_err(db_err)?;
        Ok(store)
    }
    pub fn save_settings(&mut self, mut settings: Settings) -> Result<(), String> {
        settings.validate()?;
        let protected = self.organizer.protected_ids();
        if self
            .entries
            .iter()
            .filter(|e| e.view.pinned || protected.contains(&e.view.id))
            .count()
            > settings.max_items
        {
            return Err("已保存、排列队列或撤销保护的数量超过新上限，请先整理部分内容".into());
        }
        let bytes = serde_json::to_vec(&settings).map_err(|_| "设置序列化失败".to_string())?;
        let sealed = crypto::protect(&bytes)?;
        let removed = evictions(self.entries.iter(), &settings, &protected);
        let tx = self.db.transaction().map_err(db_err)?;
        tx.execute("INSERT INTO config(id,sealed) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET sealed=excluded.sealed",[sealed]).map_err(db_err)?;
        for id in &removed {
            tx.execute("DELETE FROM clips WHERE id=?1", [id])
                .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        self.entries.retain(|e| !removed.contains(&e.view.id));
        self.settings = settings;
        Ok(())
    }
    /// On-demand decryption of a record's full secret. Keeps the resident
    /// index lean: text is stored encrypted in SQLite and materialized here.
    pub fn secret(&self, id: &str) -> Result<Secret, String> {
        let sealed: Option<Vec<u8>> = self
            .db
            .query_row("SELECT secret FROM clips WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .map_err(db_err)?;
        match sealed {
            Some(b) => {
                let bytes = crypto::unprotect(&b)?;
                serde_json::from_slice(&bytes).map_err(|_| "历史内容损坏；原始数据已保留".into())
            }
            None => Err("这条记录已被删除".into()),
        }
    }
    pub fn insert(&mut self, capture: Captured) -> Result<bool, String> {
        let data = capture
            .png
            .as_deref()
            .or_else(|| capture.text.as_deref().map(str::as_bytes))
            .ok_or("内容为空")?;
        if data.is_empty() {
            return Ok(false);
        }
        let bytes = data.len();
        let kind = if capture.png.is_some() {
            "image"
        } else if capture
            .text
            .as_deref()
            .is_some_and(|s| s.starts_with("https://") || s.starts_with("http://"))
        {
            "link"
        } else {
            "text"
        };
        if (kind == "image" && bytes > MAX_IMAGE) || (kind != "image" && bytes > MAX_TEXT) {
            return Err("内容超过单条大小限制，已跳过".into());
        }
        let fingerprint = Sha256::digest(data).to_vec();
        if let Some(position) = self
            .entries
            .iter()
            .position(|e| e.fingerprint == fingerprint && e.view.kind == kind)
        {
            // Re-copied content: the bytes are identical, so the sealed secret,
            // image, and pinned flag stay untouched. Only the timestamp moves,
            // and the source keeps the original capture's provenance.
            let mut entry = self.entries.remove(position);
            let original = position;
            entry.view.copied_at = now();
            let removed = evictions(
                std::iter::once(&entry).chain(self.entries.iter()),
                &self.settings,
                &self.organizer.protected_ids(),
            );
            if removed.contains(&entry.view.id) {
                self.entries.insert(original, entry);
                return Err(
                    "已保存、排列队列或撤销保护内容已占满容量，请先整理或等待撤销保护到期".into(),
                );
            }
            let tx = self.db.transaction().map_err(db_err)?;
            tx.execute(
                "UPDATE clips SET copied_at=?1 WHERE id=?2",
                params![entry.view.copied_at, entry.view.id],
            )
            .map_err(db_err)?;
            for id in &removed {
                tx.execute("DELETE FROM clips WHERE id=?1", [id])
                    .map_err(db_err)?;
            }
            tx.commit().map_err(db_err)?;
            self.entries.retain(|e| !removed.contains(&e.view.id));
            self.entries.insert(0, entry);
            return Ok(true);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let secret = Secret {
            group_ids: Vec::new(),
            note: String::new(),
            text: capture.text,
            source: capture.source,
            fingerprint: fingerprint.clone(),
        };
        let entry = Entry {
            search: secret.search_fields(),
            view: ClipView {
                id: id.clone(),
                kind: kind.into(),
                preview: preview(&secret, kind),
                source: secret.source.clone(),
                copied_at: now(),
                pinned: false,
                bytes,
                group_ids: Vec::new(),
                note: String::new(),
            },
            fingerprint,
        };
        // Plan with borrowed entries. Mutate memory only after commit succeeds.
        let removed = evictions(
            std::iter::once(&entry).chain(self.entries.iter()),
            &self.settings,
            &self.organizer.protected_ids(),
        );
        if removed.contains(&id) {
            return Err(
                "已保存、排列队列或撤销保护内容已占满容量，请先整理或等待撤销保护到期".into(),
            );
        }
        let sealed = crypto::protect(
            &serde_json::to_vec(&secret).map_err(|_| "内容序列化失败".to_string())?,
        )?;
        let png = match capture.png {
            Some(p) => Some(crypto::protect(&p)?),
            None => None,
        };
        let tx = self.db.transaction().map_err(db_err)?;
        tx.execute("INSERT INTO clips(id,kind,copied_at,pinned,bytes,secret,image) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,kind,entry.view.copied_at,false,bytes,sealed,png]).map_err(db_err)?;
        for id in &removed {
            tx.execute("DELETE FROM clips WHERE id=?1", [id])
                .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        self.entries.retain(|e| !removed.contains(&e.view.id));
        self.entries.insert(0, entry);
        Ok(true)
    }
    pub fn cleanup(&mut self) -> Result<(), String> {
        let removed = evictions(
            self.entries.iter(),
            &self.settings,
            &self.organizer.protected_ids(),
        );
        if removed.is_empty() {
            return Ok(());
        }
        let tx = self.db.transaction().map_err(db_err)?;
        for id in &removed {
            tx.execute("DELETE FROM clips WHERE id=?1", [id])
                .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        self.entries.retain(|e| !removed.contains(&e.view.id));
        Ok(())
    }
    pub fn organizer_view(&self) -> Organizer {
        let mut o = self.organizer.clone();
        let available: HashSet<_> = self.entries.iter().map(|e| &e.view.id).collect();
        o.queue.retain(|id| available.contains(id));
        o.previous_queue.retain(|id| available.contains(id));
        if now() - o.previous_queue_at >= 300_000 {
            o.previous_queue.clear();
        }
        o
    }
    fn persist_organizer(&mut self, o: Organizer) -> Result<(), String> {
        let sealed = crypto::protect(&serde_json::to_vec(&o).map_err(|_| "分组保存失败")?)?;
        self.db.execute("INSERT INTO organizer(id,sealed) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET sealed=excluded.sealed", [sealed]).map_err(db_err)?;
        self.organizer = o;
        Ok(())
    }
    pub fn group(&mut self, id: &str, name: &str, remove: bool) -> Result<(), String> {
        let mut o = self.organizer_view();
        if remove {
            if !o.groups.iter().any(|g| g.id == id) {
                return Err("分组已不存在".into());
            }
            // Keep records and notes when removing a group. Both writes commit together.
            let updates: Vec<_> = self
                .entries
                .iter()
                .filter(|e| e.view.group_ids.iter().any(|g| g == id))
                .map(|e| {
                    let mut secret = self.secret(&e.view.id)?;
                    secret.group_ids.retain(|g| g != id);
                    let sealed =
                        crypto::protect(&serde_json::to_vec(&secret).map_err(|_| "分组保存失败")?)?;
                    Ok((e.view.id.clone(), secret, sealed))
                })
                .collect::<Result<_, String>>()?;
            o.groups.retain(|g| g.id != id);
            let sealed = crypto::protect(&serde_json::to_vec(&o).map_err(|_| "分组保存失败")?)?;
            let tx = self.db.transaction().map_err(db_err)?;
            for (id, _, sealed) in &updates {
                tx.execute(
                    "UPDATE clips SET secret=?1 WHERE id=?2",
                    params![sealed, id],
                )
                .map_err(db_err)?;
            }
            tx.execute("INSERT INTO organizer(id,sealed) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET sealed=excluded.sealed", [sealed]).map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            for (id, secret, _) in updates {
                if let Some(e) = self.entries.iter_mut().find(|e| e.view.id == id) {
                    e.view.group_ids = secret.group_ids;
                }
            }
            self.organizer = o;
            return Ok(());
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 40 {
            return Err("分组名称应为 1–40 个字".into());
        }
        if o.groups
            .iter()
            .any(|g| g.id != id && g.name.to_lowercase() == name.to_lowercase())
        {
            return Err("已有同名分组".into());
        }
        if id.is_empty() {
            if o.groups.len() >= 50 {
                return Err("最多创建 50 个分组".into());
            }
            o.groups.push(ClipGroup {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.into(),
            });
        } else {
            o.groups
                .iter_mut()
                .find(|g| g.id == id)
                .ok_or("分组已不存在")?
                .name = name.into();
        }
        self.persist_organizer(o)
    }
    pub fn annotate(&mut self, id: &str, group_ids: &[String], note: &str) -> Result<(), String> {
        if note.chars().count() > 1000 {
            return Err("备注最多 1000 个字".into());
        }
        if group_ids.len() > 50
            || group_ids
                .iter()
                .any(|id| !self.organizer.groups.iter().any(|g| &g.id == id))
        {
            return Err("分组已不存在".into());
        }
        if !self.entries.iter().any(|e| e.view.id == id) {
            return Err("记录已不存在".into());
        }
        let mut secret = self.secret(id)?;
        secret.group_ids = group_ids.to_vec();
        secret.group_ids.sort();
        secret.group_ids.dedup();
        secret.note = note.into();
        let sealed = crypto::protect(&serde_json::to_vec(&secret).map_err(|_| "备注保存失败")?)?;
        self.db
            .execute(
                "UPDATE clips SET secret=?1 WHERE id=?2",
                params![sealed, id],
            )
            .map_err(db_err)?;
        if let Some(e) = self.entries.iter_mut().find(|e| e.view.id == id) {
            e.search = secret.search_fields();
            e.view.group_ids = secret.group_ids;
            e.view.note = note.into();
        }
        Ok(())
    }
    pub fn merged_queue_text(&self) -> Result<String, String> {
        let ids = self.organizer_view().queue;
        if ids.is_empty() {
            return Err("请先把文字加入排列粘贴".into());
        }
        let mut merged = String::new();
        let entries: HashMap<_, _> = self.entries.iter().map(|e| (&e.view.id, e)).collect();
        // Reject oversized merges from resident metadata before any secret is
        // decrypted, so the budget check never allocates the merged string.
        if ids
            .iter()
            .filter_map(|id| entries.get(id))
            .map(|e| e.view.bytes)
            .sum::<usize>()
            > TEXT_BUDGET
        {
            return Err("合并内容超过 16 MB，请减少队列条数后重试".into());
        }
        for (index, id) in ids.iter().enumerate() {
            let entry = entries
                .get(id)
                .ok_or("队列中的记录已不存在，请刷新后重试")?;
            if entry.view.kind == "image" {
                return Err(
                    "队列包含图片，无法按换行合并。请先移出图片，或使用粘贴下一条。".into(),
                );
            }
            let text = self.secret(id)?.text.ok_or("队列中有无法合并的内容")?;
            let separator = if index == 0 { "" } else { "\r\n" };
            if merged.len() + separator.len() + text.len() > TEXT_BUDGET {
                return Err("合并内容超过 16 MB，请减少队列条数后重试".into());
            }
            merged.push_str(separator);
            merged.push_str(&text);
        }
        Ok(merged)
    }

    pub fn queue(&mut self, ids: Vec<String>) -> Result<(), String> {
        if ids.len() > 2000 {
            return Err("队列最多 2000 条".into());
        }
        let mut seen = HashSet::new();
        let available: HashSet<_> = self.entries.iter().map(|e| &e.view.id).collect();
        for id in &ids {
            if !seen.insert(id) || !available.contains(id) {
                return Err("队列中有重复或已删除的记录，请刷新".into());
            }
        }
        let mut o = self.organizer_view();
        if o.queue == ids {
            return Ok(());
        }
        o.previous_queue = o.queue;
        o.previous_queue_at = now();
        o.queue = ids;
        self.persist_organizer(o)
    }
    pub fn restore_queue(&mut self) -> Result<(), String> {
        let previous = self.organizer_view().previous_queue;
        if previous.is_empty() {
            return Err("没有可恢复的队列".into());
        }
        self.queue(previous)
    }
    /// Binary preview transport for native clients; avoid Base64/JSON copies.
    pub fn thumbnail_png(&self, id: &str) -> Result<Option<Vec<u8>>, String> {
        let sealed: Option<Vec<u8>> = self
            .db
            .query_row("SELECT sealed FROM thumbnails WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(db_err)?;
        sealed.map(|b| crypto::unprotect(&b)).transpose()
    }
    pub fn cache_thumbnail(&self, id: &str, png: &[u8]) -> Result<(), String> {
        // A record can be deleted while the worker is resizing its image.
        if self.entries.iter().any(|e| e.view.id == id) {
            self.db
                .execute(
                    "INSERT OR IGNORE INTO thumbnails(id,sealed) VALUES(?1,?2)",
                    params![id, crypto::protect(png)?],
                )
                .map_err(db_err)?;
            self.db.execute("DELETE FROM thumbnails WHERE rowid IN (SELECT rowid FROM thumbnails ORDER BY rowid DESC LIMIT -1 OFFSET 128)", []).map_err(db_err)?;
        }
        Ok(())
    }
    pub fn image(&self, id: &str) -> Result<Vec<u8>, String> {
        let sealed: Vec<u8> = self
            .db
            .query_row("SELECT image FROM clips WHERE id=?1", [id], |r| r.get(0))
            .map_err(db_err)?;
        crypto::unprotect(&sealed)
    }
    pub fn pin(&mut self, id: &str) -> Result<(), String> {
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.view.id == id)
            .ok_or("这条记录已被删除")?;
        self.db
            .execute(
                "UPDATE clips SET pinned=?1 WHERE id=?2",
                params![!e.view.pinned, id],
            )
            .map_err(db_err)?;
        e.view.pinned = !e.view.pinned;
        Ok(())
    }
    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        self.db
            .execute("DELETE FROM clips WHERE id=?1", [id])
            .map_err(db_err)?;
        self.entries.retain(|e| e.view.id != id);
        Ok(())
    }
    pub fn clear(&mut self, include_pinned: bool) -> Result<(), String> {
        self.db
            .execute(
                if include_pinned {
                    "DELETE FROM clips"
                } else {
                    "DELETE FROM clips WHERE pinned=0"
                },
                [],
            )
            .map_err(db_err)?;
        self.entries.retain(|e| e.view.pinned && !include_pinned);
        // SQLite reuses freed pages. Do not block capture on a full VACUUM.
        Ok(())
    }
}
pub fn make_thumbnail(png: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageDecoder;
    let mut decoder = image::codecs::png::PngDecoder::new(std::io::Cursor::new(png))
        .map_err(|_| "图片无法预览")?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) > 25_000_000 {
        return Err("图片超过预览大小限制".into());
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(110 * 1024 * 1024);
    decoder
        .set_limits(limits)
        .map_err(|_| "图片超过预览大小限制")?;
    let image = image::DynamicImage::from_decoder(decoder).map_err(|_| "图片无法预览")?;
    let mut output = std::io::Cursor::new(Vec::new());
    image
        .thumbnail(320, 240)
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|_| "缩略图生成失败")?;
    Ok(output.into_inner())
}
fn preview(secret: &Secret, kind: &str) -> String {
    if kind == "image" {
        "图片".into()
    } else {
        secret
            .text
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(180)
            .collect()
    }
}
fn evictions<'a>(
    entries: impl Iterator<Item = &'a Entry>,
    s: &Settings,
    protected_ids: &HashSet<&String>,
) -> HashSet<String> {
    let entries: Vec<_> = entries.collect();
    let protected = |e: &Entry| e.view.pinned || protected_ids.contains(&e.view.id);
    let cutoff = now() - s.retention_days * 86_400_000;
    let mut removed: HashSet<String> = entries
        .iter()
        .filter(|e| !protected(e) && e.view.copied_at < cutoff)
        .map(|e| e.view.id.clone())
        .collect();
    let remaining: Vec<_> = entries
        .iter()
        .filter(|e| !removed.contains(&e.view.id))
        .collect();
    let mut count = remaining.len();
    let mut bytes: usize = remaining.iter().map(|e| e.view.bytes).sum();
    let mut text: usize = remaining
        .iter()
        .filter(|e| e.view.kind != "image")
        .map(|e| e.view.bytes)
        .sum();
    for e in remaining.into_iter().rev().filter(|e| !protected(e)) {
        if count <= s.max_items && bytes <= DATA_BUDGET && text <= TEXT_BUDGET {
            break;
        }
        removed.insert(e.view.id.clone());
        count -= 1;
        bytes -= e.view.bytes;
        if e.view.kind != "image" {
            text -= e.view.bytes;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v3_upgrade_creates_readable_recovery_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let mut s = Store::open(&path).unwrap();
        s.insert(clip("upgrade sample")).unwrap();
        s.db.execute_batch("PRAGMA user_version=3;").unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.entries.len(), 1);
        let backup = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("history.pre-v4-")
            })
            .unwrap();
        let db = Connection::open(backup).unwrap();
        let version: i32 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        let count: i32 = db
            .query_row("SELECT count(*) FROM clips", [], |r| r.get(0))
            .unwrap();
        assert_eq!((version, count), (3, 1));
        let sealed: Vec<u8> = db
            .query_row("SELECT secret FROM clips", [], |r| r.get(0))
            .unwrap();
        let secret: Secret = serde_json::from_slice(&crypto::unprotect(&sealed).unwrap()).unwrap();
        assert_eq!(secret.text.as_deref(), Some("upgrade sample"));
    }
    #[test]
    fn thumbnails_are_small_encrypted_and_deleted_with_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("images.db");
        let image = image::RgbaImage::from_pixel(1600, 900, image::Rgba([12, 34, 56, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let original = png.into_inner();
        let small = make_thumbnail(&original).unwrap();
        let decoded = image::load_from_memory(&small).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (320, 180));
        assert!(small.len() < original.len());
        let mut s = Store::open(&path).unwrap();
        s.insert(Captured {
            text: None,
            png: Some(original.clone()),
            source: "test.exe".into(),
        })
        .unwrap();
        let id = s.entries[0].view.id.clone();
        s.cache_thumbnail(&id, &small).unwrap();
        let sealed: Vec<u8> =
            s.db.query_row("SELECT sealed FROM thumbnails WHERE id=?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_ne!(sealed, small);
        assert_eq!(s.image(&id).unwrap(), original);
        drop(s);
        let mut s = Store::open(&path).unwrap();
        assert!(s
            .thumbnail_png(&id)
            .unwrap()
            .unwrap()
            .starts_with(&[0x89, b'P', b'N', b'G']));
        s.delete(&id).unwrap();
        assert!(s.thumbnail_png(&id).unwrap().is_none());
        s.cache_thumbnail(&id, &small).unwrap();
        assert!(s.thumbnail_png(&id).unwrap().is_none());
    }
    #[test]
    fn lazy_secrets_round_trip_and_update_search_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("incremental.db")).unwrap();
        s.insert(clip("  ORIGINAL 用户\ntext")).unwrap();
        let id = s.entries[0].view.id.clone();
        s.insert(clip("another")).unwrap();
        // Secrets are no longer resident: re-reading must round-trip the
        // original text, including leading whitespace, after other inserts.
        assert_eq!(
            s.secret(&id).unwrap().text.as_deref(),
            Some("  ORIGINAL 用户\ntext")
        );
        assert!(s.secret("missing").is_err());
        s.annotate(&id, &[], "NEEDLE").unwrap();
        assert!(matches(
            s.entries.iter().find(|e| e.view.id == id).unwrap(),
            "needle",
            "all"
        ));
        s.annotate(&id, &[], "CHANGED").unwrap();
        let entry = s.entries.iter().find(|e| e.view.id == id).unwrap();
        assert!(!matches(entry, "needle", "all"));
        assert!(matches(entry, "changed", "all"));
        assert!(matches(entry, "original", "text"));
    }
    #[test]
    fn re_copy_keeps_first_source_and_restores_index_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("dedup-source.db")).unwrap();
        s.insert(Captured {
            text: Some("same".into()),
            png: None,
            source: "First.exe".into(),
        })
        .unwrap();
        let id = s.entries[0].view.id.clone();
        s.insert(Captured {
            text: Some("same".into()),
            png: None,
            source: "Second.exe".into(),
        })
        .unwrap();
        assert_eq!(s.entries.len(), 1);
        // The deduplicated row keeps its first capture's provenance; the
        // timestamp moves but the sealed secret is not rewritten.
        assert_eq!(s.secret(&id).unwrap().source, "First.exe");
        assert_eq!(s.entries[0].view.source, "First.exe");
    }
    fn clip(s: &str) -> Captured {
        Captured {
            text: Some(s.into()),
            png: None,
            source: "test.exe".into(),
        }
    }
    #[test]
    fn merged_queue_preserves_order_full_text_whitespace_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merge.db");
        let mut store = Store::open(&path).unwrap();
        assert!(store.merged_queue_text().is_err());
        let first_text = format!("  SELECT 中文\n{}\n", "x".repeat(250));
        store.insert(clip(&first_text)).unwrap();
        let first = store.entries[0].view.id.clone();
        store.insert(clip("https://example.com")).unwrap();
        let second = store.entries[0].view.id.clone();
        store.queue(vec![second.clone(), first.clone()]).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store.merged_queue_text().unwrap(),
            format!("https://example.com\r\n{first_text}")
        );
        assert_eq!(
            store.organizer_view().queue,
            vec![second.clone(), first.clone()]
        );
        assert_eq!(store.entries.len(), 2);
        store.queue(vec![first.clone()]).unwrap();
        assert_eq!(store.merged_queue_text().unwrap(), first_text);
        store.queue(vec![first.clone(), second.clone()]).unwrap();
        store
            .entries
            .iter_mut()
            .find(|entry| entry.view.id == second)
            .unwrap()
            .view
            .kind = "image".into();
        assert!(store.merged_queue_text().unwrap_err().contains("图片"));
        assert_eq!(store.organizer_view().queue, vec![first.clone(), second]);
        store.queue(vec![first.clone()]).unwrap();
        // Inflating resident metadata must trip the byte precheck before any
        // secret is decrypted or the merged string is allocated.
        store
            .entries
            .iter_mut()
            .find(|entry| entry.view.id == first)
            .unwrap()
            .view
            .bytes = TEXT_BUDGET + 1;
        assert!(store.merged_queue_text().unwrap_err().contains("16 MB"));
    }
    #[test]
    fn queued_history_is_protected_until_removed_from_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("queue.db")).unwrap();
        s.settings.max_items = 2;
        s.insert(clip("queued first")).unwrap();
        let first = s.entries[0].view.id.clone();
        s.queue(vec![first.clone()]).unwrap();
        s.entries[0].view.copied_at = 0;
        s.insert(clip("recent second")).unwrap();
        s.insert(clip("recent third")).unwrap();
        s.cleanup().unwrap();
        assert!(s.entries.iter().any(|e| e.view.id == first));
        assert_eq!(s.entries.len(), 2);
        s.queue(vec![]).unwrap();
        s.cleanup().unwrap();
        assert!(s.entries.iter().any(|e| e.view.id == first));
        s.organizer.previous_queue_at = now() - 300_001;
        s.cleanup().unwrap();
        assert!(!s.entries.iter().any(|e| e.view.id == first));
    }
    #[test]
    fn organizer_survives_restart_dedup_and_group_removal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("organizer.db");
        let mut s = Store::open(&path).unwrap();
        s.insert(clip("first")).unwrap();
        let first = s.entries[0].view.id.clone();
        s.insert(clip("second")).unwrap();
        let second = s.entries[0].view.id.clone();
        s.group("", "客户话术", false).unwrap();
        let group = s.organizer.groups[0].id.clone();
        s.annotate(&first, std::slice::from_ref(&group), "仅供回访-keep-secret")
            .unwrap();
        s.pin(&first).unwrap();
        s.queue(vec![first.clone(), second.clone()]).unwrap();
        assert!(s.queue(vec![first.clone(), first.clone()]).is_err());
        assert!(s.annotate(&first, &["missing".into()], "bad").is_err());
        s.insert(clip("first")).unwrap();
        assert_eq!(s.entries[0].view.note, "仅供回访-keep-secret");
        drop(s);
        let mut s = Store::open(&path).unwrap();
        assert_eq!(
            s.organizer_view().queue,
            vec![first.clone(), second.clone()]
        );
        assert_eq!(
            s.entries
                .iter()
                .find(|e| e.view.id == first)
                .unwrap()
                .view
                .group_ids,
            vec![group.clone()]
        );
        s.queue(vec![second.clone(), first.clone()]).unwrap();
        s.group(&group, "新名称", false).unwrap();
        s.group(&group, "", true).unwrap();
        let entry = s.entries.iter().find(|e| e.view.id == first).unwrap();
        assert!(entry.view.group_ids.is_empty());
        assert_eq!(entry.view.note, "仅供回访-keep-secret");
        assert!(entry.view.pinned);
        s.delete(&second).unwrap();
        assert_eq!(s.organizer_view().queue, vec![first]);
        drop(s);
        for file in std::fs::read_dir(dir.path()).unwrap() {
            let data = std::fs::read(file.unwrap().path()).unwrap();
            assert!(!data.windows(11).any(|s| s == b"keep-secret"));
        }
    }
    #[test]
    fn multiple_groups_survive_legacy_migration_dedup_and_partial_removal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("groups.db");
        let mut s = Store::open(&path).unwrap();
        s.insert(clip("reusable")).unwrap();
        let id = s.entries[0].view.id.clone();
        s.group("", "项目", false).unwrap();
        s.group("", "常用", false).unwrap();
        let a = s.organizer.groups[0].id.clone();
        let b = s.organizer.groups[1].id.clone();
        let mut legacy = serde_json::to_value(s.secret(&id).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("group_ids");
        legacy["group_id"] = serde_json::json!(a);
        let sealed = crypto::protect(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        s.db.execute("UPDATE clips SET secret=?1", [sealed])
            .unwrap();
        s.db.execute_batch("PRAGMA user_version=2;").unwrap();
        drop(s);
        let mut s = Store::open(&path).unwrap();
        assert_eq!(s.entries[0].view.group_ids, vec![a.clone()]);
        s.annotate(&id, &[a.clone(), b.clone(), a.clone()], "跨组复用")
            .unwrap();
        s.insert(clip("reusable")).unwrap();
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.entries[0].view.group_ids.len(), 2);
        assert!(s.annotate(&id, &["missing".into()], "bad").is_err());
        drop(s);
        let mut s = Store::open(&path).unwrap();
        assert!(s.entries[0].view.group_ids.contains(&a));
        assert!(s.entries[0].view.group_ids.contains(&b));
        s.group(&a, "", true).unwrap();
        drop(s);
        let mut s = Store::open(&path).unwrap();
        assert_eq!(s.entries[0].view.group_ids, vec![b]);
        assert_eq!(s.entries[0].view.note, "跨组复用");
        assert_eq!(s.secret(&id).unwrap().text.as_deref(), Some("reusable"));
        s.annotate(&id, &[], "跨组复用").unwrap();
        assert!(s.entries[0].view.group_ids.is_empty());
    }
    #[test]
    fn migrates_v1_secret_without_losing_content_or_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1.db");
        let mut s = Store::open(&path).unwrap();
        s.insert(clip("legacy content")).unwrap();
        let id = s.entries[0].view.id.clone();
        let mut settings = s.settings.clone();
        settings.hotkey = "Ctrl+Shift+V".into();
        s.save_settings(settings).unwrap();
        let mut old = serde_json::to_value(s.secret(&id).unwrap()).unwrap();
        old.as_object_mut().unwrap().remove("note");
        old.as_object_mut().unwrap().remove("group_ids");
        let sealed = crypto::protect(&serde_json::to_vec(&old).unwrap()).unwrap();
        s.db.execute("UPDATE clips SET secret=?1", [sealed])
            .unwrap();
        s.db.execute_batch("DROP TABLE organizer; PRAGMA user_version=1;")
            .unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.settings.hotkey, "Ctrl+Shift+V");
        assert_eq!(
            s.secret(&id).unwrap().text.as_deref(),
            Some("legacy content")
        );
        assert!(s.entries[0].view.note.is_empty());
        assert!(s.organizer.groups.is_empty());
    }
    #[test]
    fn old_default_hotkey_migrates_to_ctrl_shift_v() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotkey.db");
        let mut s = Store::open(&path).unwrap();
        let mut settings = s.settings.clone();
        settings.hotkey = "Ctrl+Alt+V".into();
        s.save_settings(settings).unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.settings.hotkey, "Ctrl+Shift+V");
    }
    #[test]
    fn custom_hotkey_survives_default_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotkey.db");
        let mut s = Store::open(&path).unwrap();
        let mut settings = s.settings.clone();
        settings.hotkey = "Ctrl+Alt+K".into();
        s.save_settings(settings).unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.settings.hotkey, "Ctrl+Alt+K");
    }
    #[test]
    fn restart_dedup_pin_and_encryption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let id;
        {
            let mut s = Store::open(&path).unwrap();
            s.insert(clip("Clibo-secret-中文-test-unique")).unwrap();
            id = s.entries[0].view.id.clone();
            s.pin(&id).unwrap();
            s.insert(clip("Clibo-secret-中文-test-unique")).unwrap();
            assert_eq!(s.entries.len(), 1);
            assert!(s.entries[0].view.pinned);
        }
        let mut s = Store::open(&path).unwrap();
        assert_eq!(
            s.secret(&id).unwrap().text.as_deref(),
            Some("Clibo-secret-中文-test-unique")
        );
        s.clear(false).unwrap();
        assert_eq!(s.entries.len(), 1);
        for f in std::fs::read_dir(dir.path()).unwrap() {
            let bytes = std::fs::read(f.unwrap().path()).unwrap();
            assert!(!bytes.windows(12).any(|w| w == b"Clibo-secret"));
        }
        s.clear(true).unwrap();
        assert!(s.entries.is_empty());
    }
    #[test]
    fn capacity_preserves_pinned_and_expires_old_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("test.db")).unwrap();
        s.settings.max_items = 2;
        s.insert(clip("first")).unwrap();
        let first = s.entries[0].view.id.clone();
        s.pin(&first).unwrap();
        s.insert(clip("second")).unwrap();
        s.insert(clip("third")).unwrap();
        assert_eq!(s.entries.len(), 2);
        assert!(s.entries.iter().any(|e| e.view.id == first));
        s.entries
            .iter_mut()
            .find(|e| !e.view.pinned)
            .unwrap()
            .view
            .copied_at = 0;
        s.cleanup().unwrap();
        assert_eq!(s.entries.len(), 1);
    }
    #[test]
    fn failed_settings_cleanup_rolls_back_config_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("test.db")).unwrap();
        s.save_settings(Settings::default()).unwrap();
        s.insert(clip("keep-on-failure")).unwrap();
        s.entries[0].view.copied_at = 0;
        s.db.execute_batch("CREATE TRIGGER refuse_delete BEFORE DELETE ON clips BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
        let mut next = s.settings.clone();
        next.enabled = true;
        assert!(s.save_settings(next).is_err());
        assert!(!s.settings.enabled);
        assert_eq!(s.entries.len(), 1);
        let sealed: Vec<u8> =
            s.db.query_row("SELECT sealed FROM config", [], |r| r.get(0))
                .unwrap();
        let disk: Settings = serde_json::from_slice(&crypto::unprotect(&sealed).unwrap()).unwrap();
        assert!(!disk.enabled);
        assert_eq!(
            s.db.query_row("SELECT COUNT(*) FROM clips", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
