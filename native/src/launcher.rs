pub mod catalog;
pub mod search;
#[cfg(windows)]
pub mod windows;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub target: String,
}

impl Entry {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.target.trim().is_empty() {
            return Err("请填写名称和目标".into());
        }
        if self.target.contains(['\0', '\n', '\r']) {
            return Err("目标包含无效字符".into());
        }
        if is_web_url(&self.target) || is_app_reference(&self.target) {
            return Ok(());
        } else if !Path::new(&self.target).is_absolute() || !Path::new(&self.target).exists() {
            return Err("请选择存在的文件或文件夹，并填写完整路径（不附带命令参数）".into());
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn matches(&self, query: &str) -> bool {
        let text = format!("{} {}", self.name, self.target).to_lowercase();
        query
            .split_whitespace()
            .all(|word| text.contains(&word.to_lowercase()))
    }
}

fn scan(root: &Path, depth: usize, entries: &mut Vec<Entry>) {
    if depth > 12 || entries.len() >= 5000 {
        return;
    }
    let Ok(children) = std::fs::read_dir(root) else {
        return;
    };
    for child in children.flatten() {
        let path = child.path();
        let Ok(kind) = child.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if (cfg!(windows) && kind.is_file() && matches!(ext.as_str(), "lnk" | "appref-ms"))
            || (cfg!(target_os = "macos") && ext == "app")
        {
            entries.push(Entry {
                name: path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                target: path.to_string_lossy().into_owned(),
            });
        } else if kind.is_dir() {
            scan(&path, depth + 1, entries);
        }
        if entries.len() >= 5000 {
            break;
        }
    }
}

pub fn discover() -> Vec<Entry> {
    let mut roots: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    for key in ["APPDATA", "PROGRAMDATA"] {
        if let Some(root) = std::env::var_os(key) {
            roots.push(PathBuf::from(root).join("Microsoft/Windows/Start Menu/Programs"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        roots.extend([
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
        ]);
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
    }
    let mut entries = Vec::new();
    for root in roots {
        scan(&root, 0, &mut entries);
    }
    // Microsoft Store apps live in the shell:AppsFolder namespace instead of the Start Menu.
    #[cfg(windows)]
    entries.extend(windows::store_apps());
    entries.sort_by_key(|entry| (entry.name.to_lowercase(), entry.target.to_lowercase()));
    entries.dedup_by(|a, b| a.target.eq_ignore_ascii_case(&b.target));
    entries
}

pub fn is_web_url(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !target.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Recognizes packaged app targets like `shell:AppsFolder\PackageFamily!ApplicationId`.
pub fn is_app_reference(target: &str) -> bool {
    target
        .strip_prefix("shell:AppsFolder\\")
        .is_some_and(|aumid| {
            !aumid.is_empty()
                && aumid.contains('!')
                && !aumid.chars().any(|c| c.is_whitespace() || c.is_control())
        })
}

pub fn launch(entry: &Entry) -> Result<(), String> {
    launch_as(entry, false)
}
pub fn launch_as(entry: &Entry, admin: bool) -> Result<(), String> {
    entry.validate()?;
    #[cfg(windows)]
    {
        if is_app_reference(&entry.target) {
            return windows::activate_app(&entry.target);
        }
        windows::open(&entry.target, admin)
    }
    #[cfg(not(windows))]
    {
        if admin {
            return Err("此平台不支持以管理员身份启动".into());
        }
        std::process::Command::new("/usr/bin/open")
            .arg(&entry.target)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub fn system_action(action: search::Builtin) -> Result<(), String> {
    #[cfg(windows)]
    {
        use search::Builtin;
        let system = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:/Windows"))
            .join("System32");
        match action {
            Builtin::Settings => windows::open("ms-settings:", false),
            Builtin::TaskManager => {
                windows::open(&system.join("Taskmgr.exe").to_string_lossy(), false)
            }
            Builtin::RecycleBin => windows::open("shell:RecycleBinFolder", false),
            Builtin::Lock => {
                if unsafe { windows_sys::Win32::System::Shutdown::LockWorkStation() } == 0 {
                    Err("无法锁定电脑".into())
                } else {
                    Ok(())
                }
            }
            Builtin::Shutdown | Builtin::Restart => {
                use std::os::windows::process::CommandExt;
                let status = std::process::Command::new(system.join("shutdown.exe"))
                    .args([
                        if action == Builtin::Shutdown {
                            "/s"
                        } else {
                            "/r"
                        },
                        "/t",
                        "0",
                    ])
                    .creation_flags(0x08000000)
                    .status()
                    .map_err(|e| e.to_string())?;
                if status.success() {
                    Ok(())
                } else {
                    Err("系统拒绝了该操作".into())
                }
            }
            _ => Err("不是系统操作".into()),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = action;
        Err("此系统操作仅适用于 Windows".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_entries_survive_encrypted_store_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher.db");
        let entry = Entry {
            name: "文档".into(),
            target: "https://example.com/private-launcher".into(),
        };
        let mut store = crate::store::Store::open(&path).unwrap();
        let mut settings = store.settings.clone();
        settings.launcher_entries.push(entry.clone());
        settings.launcher_hotkey = "Alt+S".into();
        settings.launcher_recent.push(entry.target.clone());
        store.save_settings(settings).unwrap();
        drop(store);
        let store = crate::store::Store::open(&path).unwrap();
        assert_eq!(store.settings.launcher_entries, vec![entry]);
        let old: crate::model::Settings = serde_json::from_str("{}").unwrap();
        assert!(old.launcher_entries.is_empty());
        assert_eq!(old.launcher_hotkey, "Alt+S");
        assert_eq!(store.settings.launcher_hotkey, "Alt+S");
        assert_eq!(store.settings.launcher_recent.len(), 1);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes
            .windows(b"private-launcher".len())
            .any(|window| window == b"private-launcher"));
    }
    #[test]
    fn validates_targets_and_searches() {
        let mut entry = Entry {
            name: "项目文档".into(),
            target: "https://example.com/docs".into(),
        };
        assert!(entry.validate().is_ok());
        assert!(entry.matches("项目 EXAMPLE"));
        assert!(!entry.matches("不存在"));
        for target in [
            "https://",
            "https://a b",
            "cmd /c calc",
            "javascript:alert(1)",
            "https://a\0",
        ] {
            entry.target = target.into();
            assert!(entry.validate().is_err(), "{target}");
        }
        entry.target = "shell:AppsFolder\\OpenAI.ChatGPT_2p2nqsd0c76g0!App".into();
        assert!(entry.validate().is_ok());
        for target in [
            "shell:AppsFolder\\",
            "shell:AppsFolder\\notepad",
            "shell:AppsFolder\\a!b c",
        ] {
            entry.target = target.into();
            assert!(entry.validate().is_err(), "{target}");
        }
        let dir = tempfile::tempdir().unwrap();
        entry.target = dir.path().to_string_lossy().into_owned();
        assert!(entry.validate().is_ok());
    }
    #[test]
    #[cfg(windows)]
    fn scans_nested_shortcuts_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Tools")).unwrap();
        std::fs::write(dir.path().join("Tools/Editor.LNK"), "").unwrap();
        std::fs::write(dir.path().join("ignore.txt"), "").unwrap();
        let mut entries = Vec::new();
        scan(dir.path(), 0, &mut entries);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Editor");
    }
}
