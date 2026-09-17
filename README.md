# Clibo

[English](README.en.md) · 中文

[![Native checks](https://github.com/wushuang3625/clibo/actions/workflows/native.yml/badge.svg)](https://github.com/wushuang3625/clibo/actions/workflows/native.yml)

Clibo 是一款基于 Rust + egui 构建的原生本地剪贴板管理工具，面向 Windows 提供快捷、轻量且注重隐私的剪贴板历史管理体验。

当前版本：**0.5.0 原生预览版**。

## 功能

- 保存文字、链接以及 PNG / DIB 图片历史。
- 中文子串搜索、类型筛选、收藏、备注和分组。
- 队列粘贴：排序、逐条粘贴、合并粘贴和队列恢复。
- 去空白、合并空行和大小写转换等文字处理。
- JSON 工作页：自动校验、格式化、树形展开 / 折叠、嵌套字段搜索，以及格式化 / 压缩复制。
- 全局快捷键、系统托盘、固定面板、失焦隐藏和单实例。
- 启动器：自动发现开始菜单应用，支持自定义软件、文件夹、文件和网址入口。
- 内置临时计算器：在搜索框输入 = 进入计算模式。
- 时间戳转换：秒 / 毫秒、日期时间、UTC 偏移和批量转换。
- 浅色 / 深色外观与琥珀 / 蓝灰双主题，图片缩略图预览和 Ctrl+P 预览抽屉。
- 历史记录手动备份与恢复，恢复前自动创建安全备份。
- Windows 登录自启：无需管理员权限，登录后可在托盘后台运行。

## 界面预览

<p align="center">
  <img src="docs/images/clipboard.png" alt="剪贴板历史" width="32%">
  <img src="docs/images/json-format.png" alt="JSON 格式化工作页" width="32%">
  <img src="docs/images/launcher.png" alt="启动器" width="32%">
</p>
<p align="center">
  <img src="docs/images/calculator.png" alt="计算器" width="32%">
  <img src="docs/images/timestamp.png" alt="时间戳转换" width="32%">
  <img src="docs/images/settings.png" alt="偏好设置" width="32%">
</p>

剪贴板面板、JSON 工作页、启动器、临时计算器、时间戳转换和偏好设置均为原生界面，数据处理在本地完成。

## 下载

Windows x64 用户可以直接下载 [Clibo 0.5.0](https://github.com/wushuang3625/clibo/releases/tag/v0.5.0) 便携版。解压后运行 Clibo.exe，不需要 Node、WebView 或其他运行时。

## 快速使用

1. 启动后点击“记录中”开始采集。
2. 在任意应用中复制文字、链接或图片。
3. 按 Ctrl + Shift + V 呼出 Clibo。
4. 搜索并选择记录，按 Enter 尝试粘贴，按 Shift + Enter 仅复制。
5. 在搜索框输入 > 进入工具命令模式，可打开 JSON 工作页、时间戳转换、启动器和偏好设置。
6. 在偏好设置中可以修改呼出和排列粘贴快捷键。
7. Windows 用户可在偏好设置的“启动”区域开启“开机自启”，登录后自动在托盘后台运行。开关立即生效，无需管理员权限；移动便携版程序后请关闭再重新开启。

## 启动器

按 **Alt + S** 呼出紧凑的悬浮搜索框，也可在面板搜索框输入 > 选择“启动器”，或点击托盘菜单中的“启动器”。再次按 Alt + S 或 Esc 收起；未固定窗口时点击外部也会收起。启动器由 Clibo 原生实现，不需要 Flow Launcher，不包含其插件市场或第三方插件运行环境。

| 输入 | 功能 |
| --- | --- |
| 名称、关键词 | 搜索应用、自定义入口、文件、书签；英文模糊匹配、中文子串匹配，最近使用优先 |
| `app 名称` | 仅搜索应用 |
| `file 关键词` | 搜索文件／文件夹；可选查询 Everything |
| 完整路径、`~`、`%USERPROFILE%` | 打开路径、列出匹配的子项 |
| `bm 名称` | 搜索 Chrome、Edge、Brave 和 Firefox 本地书签 |
| `g 关键词` / `b 关键词` / `bd 关键词` | Google / Bing / 百度网页搜索；仅在执行后打开浏览器 |
| `= 12*3` 或 `12*3` | 本地计算，回车复制结果 |
| `sys 关键词` | Windows 设置、任务管理器、回收站、锁屏、关机、重启，以及 Clibo 内置工具 |
| `https://example.com` | 打开网址 |

- ↑ / ↓ 或 Tab / Shift+Tab 选择结果，Enter 打开；Ctrl+Enter 打开所在文件夹，Ctrl+Shift+Enter 请求以管理员身份启动。
- 右键结果可复制路径／网址、打开所在文件夹、编辑或删除自定义入口。关机、重启会在界面中再次确认。
- 点击右上角齿轮或 Ctrl+I 管理自定义入口、搜索目录和书签开关。自定义入口、最近使用、搜索范围及 Alt+S 配置保存在本地加密设置中。
- 文件默认索引桌面、文档和下载目录；可添加至多 20 个目录。每个目录最多扫描 12,000 项、10 层、约 3 秒，排除隐藏目录、链接和常见构建目录，达到上限会在管理页提示。不是全盘文件内容索引。
- 应用从 Windows 当前用户及公共开始菜单发现。后台先加载应用，再扫描文件和书签；F5 刷新索引，文件变动不会实时监听。
- Everything 是可选项：安装并运行 Everything，再将官方 `es.exe` 加入 PATH 或放在 Everything 安装目录中，`file` 查询会同时使用它。服务不可用或超时会提示并保留本地结果。它不依赖 Flow Launcher。
- Alt+S 可在 Clibo 偏好设置中修改；被其他程序占用时会提示冲突，其他可用快捷键仍正常注册。
- 开发运行：`cargo run --manifest-path native/Cargo.toml -- --launcher`（需先退出同一数据目录的已有实例）。

macOS 保留应用目录与本地文件搜索路径；Windows 系统操作及浏览器书签自动发现仅在 Windows 启用，macOS 尚未实机验证。当前没有拼音搜索、Shell 脚本执行、Flow 插件兼容或插件市场。

## 数据与隐私

默认数据位置：

- Windows：%LOCALAPPDATA%\local.clibo.native\history.db
- macOS：~/Library/Application Support/local.clibo.native/history.db

界面主题状态保存在同一目录下的 ui-state.json。剪贴板正文、来源、图片和设置保存在本地加密数据库中：Windows 使用 DPAPI，macOS 使用 AES-256-GCM 和系统钥匙串。Clibo 不上传剪贴板内容。

测试或演示时可以使用 CLIBO_DATA_DIR 指定独立数据目录：

~~~powershell
$env:CLIBO_DATA_DIR = "D:\CliboTestData"
~~~

## 从源码构建

需要 Rust 1.95.0（仓库已通过 rust-toolchain.toml 固定），Windows 还需要 Visual Studio C++ Build Tools，macOS 需要 Xcode Command Line Tools。

~~~powershell
cargo fmt --check --manifest-path native/Cargo.toml
cargo test --locked --manifest-path native/Cargo.toml
cargo clippy --locked --manifest-path native/Cargo.toml -- -D warnings
cargo build --release --locked --manifest-path native/Cargo.toml
~~~

运行开发版本：

~~~powershell
cargo run --manifest-path native/Cargo.toml
~~~

构建并导出便携版文件：

~~~powershell
npm run native:build
~~~

Windows 导出到 output/native/：

~~~text
Clibo.exe
使用说明.md
SHA256.txt
~~~

Node 仅用于导出文件，运行 Clibo 不需要 Node。

## 发布

发布版本时需要同步更新 package.json 和 native/Cargo.toml 中的版本号，然后创建并推送版本标签：

~~~powershell
git tag -a vX.Y.Z -m "Clibo X.Y.Z"
git push origin vX.Y.Z
~~~

推送 vX.Y.Z 后，GitHub Actions 会自动运行检查、构建 Windows x64 便携包、生成 SHA256 文件并创建 GitHub Release。发布工作流见 [.github/workflows/release.yml](.github/workflows/release.yml)。

## 平台状态

- Windows x64：提供可下载的便携版。
- macOS：包含原生适配代码和 CI 检查，但尚未提供 .app、签名和实机验收版本。
- 当前没有安装器、自动更新和代码签名。

## 项目结构

~~~text
native/                       Rust 原生应用
scripts/export-native.mjs     构建产物导出脚本
.github/workflows/            CI 与 Release 工作流
~~~

## 参与贡献

欢迎提交 Issue 和 Pull Request。提交前请运行格式检查、测试和 Clippy，并在描述中说明操作系统和复现步骤。

## 许可证

本项目采用 [MIT License](LICENSE)。
