use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_TEXT: usize = 1024 * 1024;
pub const MAX_IMAGE: usize = 10 * 1024 * 1024;
pub const TEXT_BUDGET: usize = 16 * 1024 * 1024;
pub const DATA_BUDGET: usize = 1024 * 1024 * 1024;
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct LauncherSearchPrefixes {
    pub apps: String,
    pub files: String,
    pub bookmarks: String,
    pub system: String,
    pub google: String,
    pub bing: String,
    pub baidu: String,
    pub web: String,
    pub calculator: String,
}
impl Default for LauncherSearchPrefixes {
    fn default() -> Self {
        Self {
            apps: "app".into(),
            files: "file".into(),
            bookmarks: "bm".into(),
            system: "sys".into(),
            google: "g".into(),
            bing: "b".into(),
            baidu: "bd".into(),
            web: "web".into(),
            calculator: "=".into(),
        }
    }
}
impl LauncherSearchPrefixes {
    fn normalize(label: &str, value: &mut String) -> Result<(), String> {
        *value = value.trim().to_lowercase();
        if value.is_empty() || value.chars().count() > 12 || value.chars().any(char::is_whitespace)
        {
            return Err(format!("{label}搜索指令应为 1–12 个不含空格的字符"));
        }
        Ok(())
    }
    pub fn validate(&mut self) -> Result<(), String> {
        Self::normalize("应用", &mut self.apps)?;
        Self::normalize("文件", &mut self.files)?;
        Self::normalize("书签", &mut self.bookmarks)?;
        Self::normalize("系统", &mut self.system)?;
        Self::normalize("Google", &mut self.google)?;
        Self::normalize("Bing", &mut self.bing)?;
        Self::normalize("百度", &mut self.baidu)?;
        Self::normalize("网页", &mut self.web)?;
        Self::normalize("计算", &mut self.calculator)?;
        let values = [
            ("应用", &self.apps),
            ("文件", &self.files),
            ("书签", &self.bookmarks),
            ("系统", &self.system),
            ("Google", &self.google),
            ("Bing", &self.bing),
            ("百度", &self.baidu),
            ("网页", &self.web),
            ("计算", &self.calculator),
        ];
        for (index, (label, value)) in values.iter().enumerate() {
            if let Some((other, _)) = values[..index].iter().find(|(_, old)| *old == *value) {
                return Err(format!("{label}搜索指令与{other}重复"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub launcher_hotkey: String,
    pub launcher_recent: Vec<String>,
    pub launcher_roots: Vec<String>,
    pub launcher_bookmarks: bool,
    pub launcher_entries: Vec<crate::launcher::Entry>,
    pub launcher_search_prefixes: LauncherSearchPrefixes,
    pub enabled: bool,
    pub capture_images: bool,
    pub detect_sensitive: bool,
    pub max_items: usize,
    pub retention_days: i64,
    pub json_history_max_items: usize,
    pub excluded_apps: Vec<String>,
    pub hotkey: String,
    pub queue_hotkey: String,
    pub find_hotkey: String,
    pub json_hotkey: String,
    pub timestamp_hotkey: String,
    pub autostart: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            launcher_hotkey: "Alt+S".into(),
            launcher_recent: Vec::new(),
            launcher_roots: Vec::new(),
            launcher_bookmarks: true,
            launcher_entries: Vec::new(),
            launcher_search_prefixes: LauncherSearchPrefixes::default(),
            enabled: false,
            capture_images: true,
            detect_sensitive: true,
            max_items: 2000,
            retention_days: 30,
            json_history_max_items: 100,
            excluded_apps: if cfg!(target_os = "macos") {
                vec![
                    "com.1password.1password".into(),
                    "com.agilebits.onepassword7".into(),
                    "com.bitwarden.desktop".into(),
                    "org.keepassxc.keepassxc".into(),
                ]
            } else {
                vec![
                    "1password.exe".into(),
                    "keepass.exe".into(),
                    "keepassxc.exe".into(),
                    "bitwarden.exe".into(),
                ]
            },
            hotkey: "Ctrl+Shift+V".into(),
            queue_hotkey: "Ctrl+Alt+Q".into(),
            find_hotkey: "Ctrl+F".into(),
            // 面板激活时生效，默认未设置，可在偏好设置中录入。
            json_hotkey: String::new(),
            timestamp_hotkey: String::new(),
            autostart: false,
        }
    }
}
impl Settings {
    pub fn validate(&mut self) -> Result<(), String> {
        self.launcher_recent.truncate(50);
        self.launcher_search_prefixes.validate()?;
        if self.launcher_roots.len() > 20 {
            return Err("最多添加 20 个搜索目录".into());
        }
        if !(100..=2000).contains(&self.max_items) {
            return Err("历史上限应为 100–2000 条".into());
        }
        if !(1..=365).contains(&self.retention_days) {
            return Err("保留时间应为 1–365 天".into());
        }
        if !(10..=500).contains(&self.json_history_max_items) {
            return Err("JSON 历史应保留 10–500 条".into());
        }
        if self.excluded_apps.len() > 100 {
            return Err("最多排除 100 个应用".into());
        }
        for app in &mut self.excluded_apps {
            *app = app.trim().to_lowercase();
            if app.is_empty()
                || (cfg!(windows) && !app.ends_with(".exe"))
                || app.contains(['/', '\\'])
                || app.len() > 100
            {
                return Err(if cfg!(target_os = "macos") {
                    "请填写应用 Bundle ID，例如 com.bitwarden.desktop"
                } else {
                    "请填写应用进程名，例如 bitwarden.exe"
                }
                .into());
            }
        }
        self.excluded_apps.sort();
        self.excluded_apps.dedup();
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct JsonHistoryView {
    pub id: String,
    pub preview: String,
    pub created_at: i64,
    pub bytes: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipView {
    pub id: String,
    pub kind: String,
    pub preview: String,
    pub source: String,
    pub copied_at: i64,
    pub pinned: bool,
    pub bytes: usize,
    pub group_ids: Vec<String>,
    pub note: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Secret {
    pub text: Option<String>,
    pub source: String,
    pub fingerprint: Vec<u8>,
    #[serde(default, alias = "group_id", deserialize_with = "deserialize_groups")]
    pub group_ids: Vec<String>,
    #[serde(default)]
    pub note: String,
}
fn deserialize_groups<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StoredGroups {
        Legacy(String),
        Multiple(Vec<String>),
    }
    let mut groups = match StoredGroups::deserialize(d)? {
        StoredGroups::Legacy(id) => {
            if id.is_empty() {
                vec![]
            } else {
                vec![id]
            }
        }
        StoredGroups::Multiple(ids) => ids,
    };
    groups.retain(|id| !id.is_empty());
    groups.sort();
    groups.dedup();
    Ok(groups)
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Organizer {
    pub groups: Vec<ClipGroup>,
    pub queue: Vec<String>,
    #[serde(default)]
    pub previous_queue: Vec<String>,
    #[serde(default)]
    pub previous_queue_at: i64,
}
impl Organizer {
    pub fn protected_ids(&self) -> std::collections::HashSet<&String> {
        self.queue
            .iter()
            .chain(
                self.previous_queue
                    .iter()
                    .filter(|_| now() - self.previous_queue_at < 300_000),
            )
            .collect()
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ClipGroup {
    pub id: String,
    pub name: String,
}
/// Resident index row: metadata plus the lowercase search copy only.
/// The original `Secret` stays encrypted in SQLite and is loaded on demand
/// via `Store::secret`, so clipboard text is held once, not twice.
#[derive(Clone)]
pub struct Entry {
    pub view: ClipView,
    pub search: [String; 3],
    pub fingerprint: Vec<u8>,
}
impl Secret {
    pub fn search_fields(&self) -> [String; 3] {
        [
            self.text.as_deref().unwrap_or("").to_lowercase(),
            self.source.to_lowercase(),
            self.note.to_lowercase(),
        ]
    }
}
pub struct Captured {
    pub text: Option<String>,
    pub png: Option<Vec<u8>>,
    pub source: String,
}

pub fn is_sensitive(text: &str) -> bool {
    if text.contains("-----BEGIN ") && text.contains("PRIVATE KEY-----") {
        return true;
    }
    text.split_whitespace().any(|word| {
        let w = word.trim_matches(|c: char| "\"'`,;()[]{}".contains(c));
        (w.starts_with("ghp_") && w.len() >= 36)
            || (w.starts_with("github_pat_") && w.len() >= 40)
            || (w.starts_with("sk-") && w.len() >= 32 && !w.contains(' '))
    })
}
pub fn matches(entry: &Entry, query: &str, filter: &str) -> bool {
    let kind = entry.view.kind.as_str();
    let eligible = match filter {
        "pinned" => entry.view.pinned,
        "text" => kind != "image",
        "image" => kind == "image",
        _ => true,
    };
    eligible && (query.is_empty() || entry.search.iter().any(|field| field.contains(query)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_settings_keep_primary_shortcut_and_default_queue_shortcut() {
        let settings: Settings = serde_json::from_str(r#"{"hotkey":"Ctrl+Shift+V"}"#).unwrap();
        assert_eq!(settings.hotkey, "Ctrl+Shift+V");
        assert_eq!(settings.queue_hotkey, "Ctrl+Alt+Q");
        assert_eq!(settings.find_hotkey, "Ctrl+F");
        assert_eq!(settings.json_history_max_items, 100);
        let mut custom = settings;
        custom.find_hotkey = "Ctrl+Shift+F".into();
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&custom).unwrap()).unwrap();
        assert_eq!(restored.find_hotkey, "Ctrl+Shift+F");
    }
    #[test]
    fn legacy_saved_queues_field_is_ignored() {
        let organizer: Organizer = serde_json::from_str(
            r#"{"groups":[],"queue":["current"],"savedQueues":[{"id":"old","name":"旧模板","ids":["saved"]}],"previousQueue":["previous"],"previousQueueAt":123}"#,
        )
        .unwrap();
        assert_eq!(organizer.queue, vec!["current"]);
        assert_eq!(organizer.previous_queue, vec!["previous"]);
        assert_eq!(organizer.previous_queue_at, 123);
    }
    #[test]
    fn sensitive_rules_do_not_drop_regular_numbers() {
        assert!(is_sensitive("-----BEGIN RSA PRIVATE KEY-----\nsecret"));
        assert!(is_sensitive("ghp_012345678901234567890123456789012345"));
        assert!(!is_sensitive("订单 123456，联系电话 13800138000"));
    }
    #[test]
    fn chinese_substrings_and_code_whitespace() {
        let secret = Secret {
            text: Some("  SELECT 用户名称\nFROM accounts;".into()),
            source: "Code.exe".into(),
            fingerprint: vec![],
            group_ids: Vec::new(),
            note: String::new(),
        };
        let entry = Entry {
            search: secret.search_fields(),
            view: ClipView {
                id: "test".into(),
                kind: "text".into(),
                preview: "".into(),
                source: "Code.exe".into(),
                copied_at: 0,
                pinned: false,
                bytes: 40,
                group_ids: Vec::new(),
                note: String::new(),
            },
            fingerprint: vec![],
        };
        assert!(matches(&entry, "用", "all"));
        assert!(matches(&entry, "用户", "all"));
        assert!(matches(&entry, "select", "text"));
        assert!(!matches(&entry, "用", "image"));
        assert_eq!(secret.text.as_deref().unwrap().chars().next(), Some(' '));
    }
}
