use super::{
    catalog::{Catalog, Indexed, Kind},
    Entry,
};
use crate::model::LauncherSearchPrefixes;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Clipboard,
    Json,
    Timestamp,
    Preferences,
    Refresh,
    Settings,
    TaskManager,
    RecycleBin,
    Lock,
    Shutdown,
    Restart,
}
impl Builtin {
    pub fn requires_confirmation(self) -> bool {
        matches!(self, Self::Shutdown | Self::Restart)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Open(Entry),
    Copy(String),
    Builtin(Builtin),
}
#[derive(Clone, Debug)]
pub struct Row {
    pub title: String,
    pub subtitle: String,
    pub kind: Kind,
    pub action: Action,
    pub custom: Option<usize>,
    pub score: i64,
}
impl Row {
    pub fn entry(
        entry: Entry,
        kind: Kind,
        source: &str,
        score: i64,
        custom: Option<usize>,
    ) -> Self {
        Self {
            title: entry.name.clone(),
            subtitle: format!("{source} · {}", entry.target),
            kind,
            action: Action::Open(entry),
            score,
            custom,
        }
    }
    pub fn target(&self) -> Option<&str> {
        match &self.action {
            Action::Open(e) => Some(&e.target),
            _ => None,
        }
    }
}
#[derive(Default)]
pub struct Results {
    pub rows: Vec<Row>,
    pub note: String,
}

pub fn pinyin_keys(name: &str) -> (String, String) {
    use pinyin::ToPinyin;
    let mut full = String::new();
    let mut initials = String::new();
    let mut chinese = false;
    for c in name.to_lowercase().chars() {
        if let Some(py) = c.to_pinyin() {
            chinese = true;
            full.push_str(py.plain());
            initials.push(py.plain().chars().next().unwrap());
        } else if c.is_alphanumeric() {
            full.push(c);
            initials.push(c);
        }
    }
    if chinese {
        (full, initials)
    } else {
        (String::new(), String::new())
    }
}

// Literal name matches outrank transliterations, then paths and fuzzy matches.
pub fn score(name: &str, path: &str, query: &str) -> Option<i64> {
    score_with_pinyin(name, path, query, &pinyin_keys(name))
}

fn score_with_pinyin(
    name: &str,
    path: &str,
    query: &str,
    aliases: &(String, String),
) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let mut total = 0;
    for word in query.split_whitespace() {
        total += if name == word {
            1500
        } else if name.starts_with(word) {
            1100
        } else if let Some(at) = name.find(word) {
            800 - at.min(200) as i64
        } else if word.is_ascii()
            && word.len() >= 2
            && (aliases.0.contains(word) || aliases.1.contains(word))
        {
            if aliases.0 == word || aliases.1 == word {
                590
            } else if aliases.0.starts_with(word) || aliases.1.starts_with(word) {
                540
            } else {
                450
            }
        } else if path.contains(word) {
            300
        } else if word.is_ascii() && word.len() >= 2 {
            let mut chars = word.chars();
            let mut next = chars.next();
            let mut span = 0;
            let mut started = false;
            for c in name.chars() {
                if next == Some(c) {
                    started = true;
                    next = chars.next();
                }
                if started {
                    span += 1;
                }
                if next.is_none() {
                    break;
                }
            }
            if next.is_some() {
                return None;
            }
            150 - span.min(100)
        } else {
            return None;
        };
    }
    Some(total)
}

fn encode(value: &str) -> String {
    let mut result = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            result.push(b as char);
        } else {
            result.push_str(&format!("%{b:02X}"));
        }
    }
    result
}

pub fn expand_path(input: &str) -> String {
    let mut path = input.trim().trim_matches('"').to_owned();
    if let Some(rest) = path.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with(['/', '\\']) {
            if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            {
                path = format!("{}{rest}", home.to_string_lossy());
            }
        }
    }
    if let Some(rest) = path.strip_prefix('%') {
        if let Some((key, tail)) = rest.split_once('%') {
            if let Some(root) = std::env::var_os(key) {
                path = format!("{}{tail}", root.to_string_lossy());
            }
        }
    }
    path
}

fn path_rows(query: &str) -> Vec<Row> {
    let expanded = expand_path(query);
    let path = Path::new(&expanded);
    if !path.is_absolute() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    if path.exists() {
        rows.push(Row::entry(
            Entry {
                name: path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
                target: expanded.clone(),
            },
            if path.is_dir() {
                Kind::Folder
            } else {
                Kind::File
            },
            "直接打开",
            4000,
            None,
        ));
    }
    let (parent, prefix) = if path.is_dir() && expanded.ends_with(['/', '\\']) {
        (path, String::new())
    } else {
        (
            path.parent().unwrap_or(path),
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
        )
    };
    if let Ok(children) = std::fs::read_dir(parent) {
        for child in children.flatten().take(3000) {
            let name = child.file_name().to_string_lossy().into_owned();
            if name.to_lowercase().starts_with(&prefix) {
                rows.push(Row::entry(
                    Entry {
                        name,
                        target: child.path().to_string_lossy().into_owned(),
                    },
                    if child.file_type().is_ok_and(|t| t.is_dir()) {
                        Kind::Folder
                    } else {
                        Kind::File
                    },
                    "路径",
                    2000,
                    None,
                ));
                if rows.len() >= 80 {
                    break;
                }
            }
        }
    }
    rows
}

#[cfg(test)]
pub fn search(catalog: &Catalog, custom: &[Entry], recent: &[String], input: &str) -> Results {
    search_with_prefixes(
        catalog,
        custom,
        recent,
        &LauncherSearchPrefixes::default(),
        input,
    )
}

pub fn search_with_prefixes(
    catalog: &Catalog,
    custom: &[Entry],
    recent: &[String],
    prefixes: &LauncherSearchPrefixes,
    input: &str,
) -> Results {
    // `>` 或全角 `》` 进入工具模式，只列出 Clibo 内置工具，类似剪贴板面板的命令模式。
    let tools_input = input.trim_start();
    let tools_input = tools_input
        .strip_prefix('>')
        .or_else(|| tools_input.strip_prefix('》'));
    if let Some(tools_query) = tools_input.map(str::trim) {
        let mut result = Results::default();
        for (title, keywords, builtin) in [
            ("剪贴板历史", "clipboard history", Builtin::Clipboard),
            ("JSON 工作页", "json format", Builtin::Json),
            ("时间戳转换", "timestamp time", Builtin::Timestamp),
            (
                "Clibo 偏好设置",
                "settings preferences",
                Builtin::Preferences,
            ),
            ("刷新启动器索引", "refresh reload", Builtin::Refresh),
        ] {
            if let Some(s) = score(&title.to_lowercase(), keywords, &tools_query.to_lowercase()) {
                result.rows.push(Row {
                    title: title.into(),
                    subtitle: "Clibo 工具 · Enter 执行".into(),
                    kind: Kind::Tool,
                    action: Action::Builtin(builtin),
                    custom: None,
                    score: s + 200,
                });
            }
        }
        result.rows.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        return result;
    }
    let raw = input.trim();
    let lower = input.trim_start().to_lowercase();
    let (mode, query) = match lower.split_once(' ') {
        Some((prefix, query)) => {
            let mode = if prefix == prefixes.apps {
                "app"
            } else if prefix == prefixes.files {
                "file"
            } else if prefix == prefixes.bookmarks {
                "bm"
            } else if prefix == prefixes.system {
                "sys"
            } else if prefix == prefixes.google {
                "g"
            } else if prefix == prefixes.bing {
                "b"
            } else if prefix == prefixes.baidu {
                "bd"
            } else if prefix == prefixes.web {
                "web"
            } else {
                ""
            };
            if mode.is_empty() {
                ("", lower.as_str())
            } else {
                (mode, query.trim())
            }
        }
        _ => ("", lower.as_str()),
    };
    let mut result = Results::default();
    if raw.len() > 4096 {
        result.note = "搜索内容最多 4096 字节".into();
        return result;
    }
    if ["g", "b", "bd", "web"].contains(&mode) {
        let original = raw.split_once(' ').map(|(_, q)| q.trim()).unwrap_or("");
        if !original.is_empty() {
            let (engine, base) = match mode {
                "g" => ("Google", "https://www.google.com/search?q="),
                "bd" => ("百度", "https://www.baidu.com/s?wd="),
                _ => ("Bing", "https://www.bing.com/search?q="),
            };
            result.rows.push(Row::entry(
                Entry {
                    name: format!("在 {engine} 搜索 {original}"),
                    target: format!("{base}{}", encode(original)),
                },
                Kind::Web,
                "回车后打开浏览器",
                5000,
                None,
            ));
        }
        return result;
    }
    if mode.is_empty() && super::is_web_url(raw) {
        result.rows.push(Row::entry(
            Entry {
                name: raw.into(),
                target: raw.into(),
            },
            Kind::Web,
            "打开网址",
            5000,
            None,
        ));
        return result;
    }
    let calc_prefix = prefixes.calculator.as_str();
    let full_width_default = calc_prefix == "=" && raw.starts_with('＝');
    let explicit_calc = raw.starts_with(calc_prefix) || full_width_default;
    let expression = if full_width_default {
        raw.strip_prefix('＝').unwrap_or(raw).trim_start()
    } else if explicit_calc {
        raw.strip_prefix(calc_prefix).unwrap_or(raw).trim_start()
    } else {
        raw
    };
    let looks_like_calc = expression.chars().any(|c| c.is_ascii_digit())
        && expression.contains(['+', '-', '*', '/', '%', '^', '×', '÷']);
    if explicit_calc || looks_like_calc {
        match crate::calculator::evaluate(expression) {
            Ok(answer) => result.rows.push(Row {
                title: answer.display(),
                subtitle: format!("{expression}  ·  Enter 复制结果"),
                kind: Kind::Calculator,
                action: Action::Copy(answer.full()),
                custom: None,
                score: 6000,
            }),
            Err(error) if explicit_calc => result.note = error.message,
            _ => {}
        }
        if explicit_calc {
            return result;
        }
    }
    if mode.is_empty() || mode == "file" {
        let path_query = if mode == "file" {
            input
                .trim_start()
                .split_once(' ')
                .map(|(_, value)| value)
                .unwrap_or("")
        } else {
            raw
        };
        result.rows.extend(path_rows(path_query));
    }
    for (index, entry) in custom.iter().enumerate() {
        let kind = if super::is_web_url(&entry.target) {
            Kind::Web
        } else {
            Kind::App
        };
        if mode == "sys" || mode == "bm" {
            continue;
        }
        if let Some(s) = score(
            &entry.name.to_lowercase(),
            &entry.target.to_lowercase(),
            query,
        ) {
            result.rows.push(Row::entry(
                entry.clone(),
                kind,
                "自定义",
                s + 70 + recent_bonus(recent, &entry.target),
                Some(index),
            ));
        }
    }
    for Indexed {
        entry,
        kind,
        source,
        name_lower,
        target_lower,
        pinyin,
    } in &catalog.items
    {
        if !match mode {
            "app" => *kind == Kind::App,
            "file" => matches!(kind, Kind::File | Kind::Folder),
            "bm" => *kind == Kind::Bookmark,
            "sys" => false,
            _ => true,
        } {
            continue;
        }
        if query.is_empty()
            && mode.is_empty()
            && !matches!(kind, Kind::App)
            && !recent.contains(&entry.target)
        {
            continue;
        }
        if let Some(s) = score_with_pinyin(name_lower, target_lower, query, pinyin) {
            let boost = if *kind == Kind::App { 40 } else { 0 };
            result.rows.push(Row::entry(
                entry.clone(),
                *kind,
                source,
                s + boost + recent_bonus(recent, &entry.target),
                None,
            ));
        }
    }
    if mode == "file" && !query.is_empty() {
        if let Some(es) = &catalog.everything {
            match everything(
                es,
                input
                    .trim_start()
                    .split_once(' ')
                    .map(|(_, value)| value)
                    .unwrap_or("")
                    .trim(),
            ) {
                Ok(paths) => {
                    for path in paths {
                        result.rows.push(Row::entry(
                            Entry {
                                name: path
                                    .file_name()
                                    .unwrap_or(path.as_os_str())
                                    .to_string_lossy()
                                    .into_owned(),
                                target: path.to_string_lossy().into_owned(),
                            },
                            Kind::File,
                            "Everything",
                            900,
                            None,
                        ));
                    }
                }
                Err(error) => result.note = format!("{error}；已显示本地索引结果"),
            }
        }
    }
    if mode.is_empty() || mode == "sys" {
        for (title, keywords, builtin) in [
            ("剪贴板历史", "clipboard history", Builtin::Clipboard),
            ("JSON 工作页", "json format", Builtin::Json),
            ("时间戳转换", "timestamp time", Builtin::Timestamp),
            (
                "Clibo 偏好设置",
                "settings preferences",
                Builtin::Preferences,
            ),
            ("刷新启动器索引", "refresh reload", Builtin::Refresh),
            ("Windows 设置", "windows settings", Builtin::Settings),
            ("任务管理器", "task manager taskmgr", Builtin::TaskManager),
            ("回收站", "recycle bin", Builtin::RecycleBin),
            ("锁定电脑", "lock", Builtin::Lock),
            ("关闭电脑", "shutdown", Builtin::Shutdown),
            ("重新启动电脑", "restart reboot", Builtin::Restart),
        ] {
            if !cfg!(windows)
                && matches!(
                    builtin,
                    Builtin::Settings
                        | Builtin::TaskManager
                        | Builtin::RecycleBin
                        | Builtin::Lock
                        | Builtin::Shutdown
                        | Builtin::Restart
                )
            {
                continue;
            }
            if let Some(s) = score(&title.to_lowercase(), keywords, query) {
                result.rows.push(Row {
                    title: title.into(),
                    subtitle: if builtin.requires_confirmation() {
                        "系统操作 · 执行前需要确认"
                    } else {
                        "Enter 执行"
                    }
                    .into(),
                    kind: if matches!(
                        builtin,
                        Builtin::Clipboard
                            | Builtin::Json
                            | Builtin::Timestamp
                            | Builtin::Preferences
                            | Builtin::Refresh
                    ) {
                        Kind::Tool
                    } else {
                        Kind::System
                    },
                    action: Action::Builtin(builtin),
                    custom: None,
                    score: s - 20,
                });
            }
        }
    }
    result.rows.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    let mut seen = HashSet::new();
    result.rows.retain(|row| {
        row.target()
            .is_none_or(|target| seen.insert(target.to_lowercase()))
    });
    result.rows.truncate(80);
    if result.rows.is_empty() && !raw.is_empty() && mode.is_empty() {
        result.rows.push(Row::entry(
            Entry {
                name: format!("在 Bing 搜索 {raw}"),
                target: format!("https://www.bing.com/search?q={}", encode(raw)),
            },
            Kind::Web,
            "未找到本地结果 · 回车后搜索网页",
            0,
            None,
        ));
    }
    result
}
fn recent_bonus(recent: &[String], target: &str) -> i64 {
    recent
        .iter()
        .position(|v| v == target)
        .map(|i| 120 - i.min(49) as i64 * 2)
        .unwrap_or(0)
}

fn everything(exe: &Path, query: &str) -> Result<Vec<PathBuf>, String> {
    let mut command = Command::new(exe);
    command
        .args(["-n", "80", "-sort", "name", "-search", query])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "无法启动 Everything 命令行工具".to_owned())?;
    let Some(mut stdout) = child.stdout.take() else {
        return Err("Everything 输出不可用".into());
    };
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + Duration::from_millis(1200);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let bytes = reader.join().unwrap_or_default();
                if !status.success() {
                    return Err("Everything 服务未就绪".into());
                }
                let output =
                    String::from_utf8(bytes).map_err(|_| "Everything 输出不是 UTF-8".to_owned())?;
                return Ok(output
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .take(80)
                    .collect());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err("Everything 搜索超时".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn chinese_names_match_full_pinyin_and_initials() {
        assert_eq!(super::pinyin_keys("微信"), ("weixin".into(), "wx".into()));
        for query in ["weixin", "wx", "wei xin"] {
            assert!(super::score("微信", "", query).is_some(), "{query}");
        }
        assert!(super::score("企业微信", "", "qywx").is_some());
        assert!(super::score("微信 dev", "", "wxdev").is_some());
        assert!(super::score("微信", "", "qq").is_none());
        assert!(super::score("微信", "", "微信") > super::score("微信", "", "wx"));
        assert!(super::score("wx", "", "wx") > super::score("微信", "", "wx"));
        let catalog = super::Catalog {
            items: vec![super::Indexed::new(
                super::Entry {
                    name: "微信".into(),
                    target: "C:/Apps/WeChat.exe".into(),
                },
                super::Kind::App,
                "应用",
            )],
            ..Default::default()
        };
        for query in ["WX", "weixin", "app wx", "wei xin"] {
            let results = super::search(&catalog, &[], &[], query);
            assert!(
                results.rows.iter().any(|row| row.title == "微信"),
                "{query}"
            );
        }
    }
    use super::*;
    #[test]
    fn configured_app_prefix_filters_to_apps() {
        let catalog = Catalog {
            items: vec![
                Indexed::new(
                    Entry {
                        name: "微信".into(),
                        target: "C:/Apps/WeChat.exe".into(),
                    },
                    Kind::App,
                    "应用",
                ),
                Indexed::new(
                    Entry {
                        name: "微信文档".into(),
                        target: "C:/Docs/微信.txt".into(),
                    },
                    Kind::File,
                    "文件",
                ),
            ],
            ..Catalog::default()
        };
        let mut prefixes = LauncherSearchPrefixes::default();
        prefixes.apps = "a".into();
        let result = search_with_prefixes(&catalog, &[], &[], &prefixes, "a wx");
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].kind, Kind::App);
        assert_eq!(result.rows[0].title, "微信");
    }

    #[test]
    fn fuzzy_ranking_and_unicode_are_stable() {
        assert!(score("visual studio code", "", "vsc").is_some());
        assert!(score("文档管理", "", "文档").is_some());
        assert!(score("visual studio", "", "vs").unwrap() < score("vs code", "", "vs").unwrap());
        assert!(score("other", "", "vsc").is_none());
    }
    #[test]
    fn empty_category_prefix_and_recent_order() {
        let first = Entry {
            name: "Alpha".into(),
            target: "C:/alpha.exe".into(),
        };
        let second = Entry {
            name: "Beta".into(),
            target: "C:/beta.exe".into(),
        };
        let catalog = Catalog {
            items: vec![
                Indexed::new(first, Kind::App, "App"),
                Indexed::new(second.clone(), Kind::App, "App"),
            ],
            ..Catalog::default()
        };
        let rows = search(&catalog, &[], &[second.target.clone()], "app ").rows;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].target(), Some(second.target.as_str()));
        assert!(search(&catalog, &[], &[], "file ").rows.is_empty());
        assert!(search(&catalog, &[], &[], "= 1/0").rows.is_empty());
    }
    #[test]
    fn routing_calculation_url_encoding_and_dedup() {
        let catalog = Catalog::default();
        let result = search(&catalog, &[], &[], "= 12 * 3");
        assert_eq!(result.rows[0].action, Action::Copy("36".into()));
        let result = search(&catalog, &[], &[], "g Rust 中文 & x");
        assert!(result.rows[0]
            .target()
            .unwrap()
            .ends_with("Rust%20%E4%B8%AD%E6%96%87%20%26%20x"));
        let entry = Entry {
            name: "Editor".into(),
            target: "C:/Editor.exe".into(),
        };
        let mut catalog = Catalog::default();
        catalog
            .items
            .push(Indexed::new(entry.clone(), Kind::App, "App"));
        let result = search(&catalog, &[entry], &[], "editor");
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].custom, Some(0));
        assert!(search(&catalog, &[], &[], "bm editor").rows.is_empty());
        let result = search(&catalog, &[], &[], "sys shutdown");
        // 关机/重启等系统操作仅在 Windows 提供；其他平台 sys 查询不应返回它们。
        #[cfg(windows)]
        assert_eq!(result.rows[0].action, Action::Builtin(Builtin::Shutdown));
        #[cfg(not(windows))]
        assert!(result.rows.is_empty());
        assert!(Builtin::Shutdown.requires_confirmation());
    }
    #[test]
    fn gt_prefix_lists_builtin_tools_only() {
        let catalog = Catalog {
            items: vec![Indexed::new(
                Entry {
                    name: "微信".into(),
                    target: "C:/Apps/WeChat.exe".into(),
                },
                Kind::App,
                "应用",
            )],
            ..Catalog::default()
        };
        // `>` 列出全部工具且不混入应用，全角 `》` 与过滤词同样生效。
        let results = search(&catalog, &[], &[], ">");
        assert_eq!(results.rows.len(), 5);
        assert!(results
            .rows
            .iter()
            .all(|row| matches!(row.action, Action::Builtin(_)) && row.kind == Kind::Tool));
        let results = search(&catalog, &[], &[], "》json");
        assert_eq!(results.rows.len(), 1);
        assert_eq!(results.rows[0].action, Action::Builtin(Builtin::Json));
        let results = search(&catalog, &[], &[], "> 时间戳");
        assert_eq!(results.rows.len(), 1);
        assert_eq!(results.rows[0].action, Action::Builtin(Builtin::Timestamp));
        assert!(search(&catalog, &[], &[], "> 不存在").rows.is_empty());
    }
    #[test]
    fn paths_with_spaces_return_actual_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("中文 document.txt");
        std::fs::write(&path, "").unwrap();
        let result = search(
            &Catalog::default(),
            &[],
            &[],
            &format!("\"{}\"", path.display()),
        );
        assert_eq!(result.rows[0].target(), path.to_str());
    }
}
