//! Local launcher catalog. All enumeration is performed by a background worker.
use super::Entry;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    File,
    Folder,
    Bookmark,
    Web,
    Calculator,
    System,
    Tool,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::App => "应用",
            Self::File => "文件",
            Self::Folder => "文件夹",
            Self::Bookmark => "书签",
            Self::Web => "网页",
            Self::Calculator => "计算",
            Self::System => "系统",
            Self::Tool => "Clibo",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Self::App => "A",
            Self::File => "≡",
            Self::Folder => "▰",
            Self::Bookmark => "★",
            Self::Web => "↗",
            Self::Calculator => "=",
            Self::System => "S",
            Self::Tool => "C",
        }
    }
}

#[derive(Clone)]
pub struct Indexed {
    pub entry: Entry,
    pub kind: Kind,
    pub source: String,
    pub name_lower: String,
    pub target_lower: String,
    pub pinyin: (String, String),
}
impl Indexed {
    pub fn new(entry: Entry, kind: Kind, source: &str) -> Self {
        Self {
            pinyin: super::search::pinyin_keys(&entry.name),
            name_lower: entry.name.to_lowercase(),
            target_lower: entry.target.to_lowercase(),
            entry,
            kind,
            source: source.into(),
        }
    }
}
#[derive(Default, Clone)]
pub struct Catalog {
    pub items: Vec<Indexed>,
    pub roots: Vec<PathBuf>,
    pub notes: Vec<String>,
    pub everything: Option<PathBuf>,
}

pub fn default_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        super::windows::known_folders()
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .map(|home| {
                ["Desktop", "Documents", "Downloads"]
                    .iter()
                    .map(|name| PathBuf::from(&home).join(name))
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub fn build(roots: &[String], bookmarks: bool, on_apps: impl FnOnce(Catalog)) -> Catalog {
    let mut catalog = Catalog {
        items: super::discover()
            .into_iter()
            .map(|e| Indexed::new(e, Kind::App, "应用程序"))
            .collect(),
        roots: if roots.is_empty() {
            default_roots()
        } else {
            roots.iter().map(PathBuf::from).collect()
        },
        everything: find_everything(),
        ..Catalog::default()
    };
    on_apps(catalog.clone());
    let roots = catalog.roots.clone();
    let mut seen = HashSet::new();
    for root in roots {
        let mut count = 0;
        let complete = scan_files(
            &root,
            0,
            Instant::now() + Duration::from_secs(3),
            &mut count,
            &mut seen,
            &mut catalog.items,
        );
        if !complete {
            catalog.notes.push(format!(
                "{}：索引已达扫描上限或目录不可访问，可缩小搜索目录",
                root.display()
            ));
        }
    }
    if bookmarks {
        load_bookmarks(&mut catalog);
    }
    catalog
}

fn scan_files(
    root: &Path,
    depth: usize,
    deadline: Instant,
    count: &mut usize,
    seen: &mut HashSet<PathBuf>,
    items: &mut Vec<Indexed>,
) -> bool {
    if depth > 10 || *count >= 12000 || Instant::now() > deadline {
        return false;
    }
    let Ok(children) = std::fs::read_dir(root) else {
        return false;
    };
    let mut complete = true;
    for child in children.flatten() {
        if *count >= 12000 || Instant::now() > deadline {
            return false;
        }
        let path = child.path();
        let Ok(kind) = child.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if child
                .metadata()
                .is_ok_and(|m| m.file_attributes() & 0x400 != 0)
            {
                continue;
            }
        }
        let name = child.file_name().to_string_lossy().into_owned();
        if name.starts_with('.')
            || [
                "node_modules",
                "target",
                "AppData",
                "$RECYCLE.BIN",
                "Windows",
            ]
            .contains(&name.as_str())
        {
            continue;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        *count += 1;
        items.push(Indexed::new(
            Entry {
                name,
                target: path.to_string_lossy().into_owned(),
            },
            if kind.is_dir() {
                Kind::Folder
            } else {
                Kind::File
            },
            "本地文件",
        ));
        if kind.is_dir() {
            complete &= scan_files(&path, depth + 1, deadline, count, seen, items);
        }
    }
    complete
}

fn chromium_nodes(node: &serde_json::Value, source: &str, depth: usize, items: &mut Vec<Indexed>) {
    if depth > 32 || items.len() > 100_000 {
        return;
    }
    if let (Some(name), Some(url)) = (node["name"].as_str(), node["url"].as_str()) {
        if super::is_web_url(url) {
            items.push(Indexed::new(
                Entry {
                    name: name.into(),
                    target: url.into(),
                },
                Kind::Bookmark,
                source,
            ));
        }
    }
    if let Some(children) = node["children"].as_array() {
        for child in children {
            chromium_nodes(child, source, depth + 1, items);
        }
    }
}

fn load_bookmarks(catalog: &mut Catalog) {
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        for (browser, relative) in [
            ("Chrome", "Google/Chrome/User Data"),
            ("Edge", "Microsoft/Edge/User Data"),
            ("Brave", "BraveSoftware/Brave-Browser/User Data"),
        ] {
            let root = PathBuf::from(&local).join(relative);
            let Ok(profiles) = std::fs::read_dir(root) else {
                continue;
            };
            for profile in profiles.flatten() {
                let name = profile.file_name().to_string_lossy().into_owned();
                if name != "Default" && !name.starts_with("Profile ") {
                    continue;
                }
                let path = profile.path().join("Bookmarks");
                if !path.is_file() {
                    continue;
                }
                let source = format!("{browser} · {name}");
                let result = (|| {
                    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 16 * 1024 * 1024
                    {
                        return Err("书签文件超过 16 MB".into());
                    }
                    let value: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                            .map_err(|e| e.to_string())?;
                    if let Some(roots) = value["roots"].as_object() {
                        for node in roots.values() {
                            chromium_nodes(node, &source, 0, &mut catalog.items);
                        }
                    }
                    Ok::<_, String>(())
                })();
                if result.is_err() {
                    catalog
                        .notes
                        .push(format!("{source} 书签暂不可读，请稍后刷新"));
                }
            }
        }
    }
    #[cfg(windows)]
    if let Some(roaming) = std::env::var_os("APPDATA") {
        let root = PathBuf::from(roaming).join("Mozilla/Firefox/Profiles");
        if let Ok(profiles) = std::fs::read_dir(root) {
            for profile in profiles.flatten() {
                let path = profile.path().join("places.sqlite");
                if !path.is_file() {
                    continue;
                }
                let result = (|| -> rusqlite::Result<()> {
                    let db = rusqlite::Connection::open_with_flags(
                        path,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )?;
                    db.busy_timeout(Duration::from_millis(100))?;
                    let mut stmt = db.prepare("SELECT COALESCE(b.title, p.title, p.url), p.url FROM moz_bookmarks b JOIN moz_places p ON b.fk=p.id WHERE b.type=1 LIMIT 20000")?;
                    let rows = stmt.query_map([], |r| {
                        Ok(Entry {
                            name: r.get(0)?,
                            target: r.get(1)?,
                        })
                    })?;
                    for entry in rows
                        .flatten()
                        .filter(|entry| super::is_web_url(&entry.target))
                    {
                        catalog
                            .items
                            .push(Indexed::new(entry, Kind::Bookmark, "Firefox"));
                    }
                    Ok(())
                })();
                if result.is_err() {
                    catalog
                        .notes
                        .push("Firefox 书签暂不可读，请稍后刷新".into());
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = catalog;
    }
}

fn find_everything() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut candidates = Vec::new();
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|p| p.join("es.exe")));
        }
        for key in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(key) {
                candidates.push(PathBuf::from(root).join("Everything/es.exe"));
            }
        }
        candidates
            .into_iter()
            .find(|path| path.is_absolute() && path.is_file())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bookmark_import_keeps_only_web_links_and_nested_titles() {
        let node = serde_json::json!({"children":[{"name":"文档","url":"https://example.com"},{"children":[{"name":"Bad","url":"javascript:alert(1)"}]}]});
        let mut items = Vec::new();
        chromium_nodes(&node, "Test", 0, &mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].entry.name, "文档");
    }
    #[test]
    fn bounded_index_skips_generated_and_hidden_directories() {
        let root = tempfile::tempdir().unwrap();
        for name in ["docs", "node_modules", ".git"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
            std::fs::write(root.path().join(name).join("a.txt"), "").unwrap();
        }
        let mut items = Vec::new();
        assert!(scan_files(
            root.path(),
            0,
            Instant::now() + Duration::from_secs(2),
            &mut 0,
            &mut HashSet::new(),
            &mut items
        ));
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| item.entry.name == "a.txt"));
    }
}
