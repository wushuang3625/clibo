# Clibo

Windows 本地剪贴板工具，Rust + egui 原生实现，运行不依赖 WebView、Node 或其他运行时。当前版本 **0.4.0 原生预览版**。

架构、macOS 适配、迁移差异与演示开关见 [native/README.md](native/README.md)。0.3.x 及更早的 Tauri WebView 版本已于 2026-09-07 移除，历史设计与验证记录见 [docs/](docs/)。

## 快速开始

```powershell
cargo run --release --manifest-path native/Cargo.toml
```

1. 首次打开默认暂停，点击「● 记录中」开始采集，只记录开启后新复制的内容。
2. 在其他应用复制文字、链接或截图。
3. 按 **Ctrl + Shift + V** 呼出面板（排列粘贴默认 Ctrl+Alt+Q），搜索并选择内容。
4. **Enter** 尝试粘贴回原窗口；**Shift + Enter** 仅复制；**Esc** 先清空搜索，再隐藏窗口。
5. 快捷键在偏好设置中修改：点击输入框后直接按组合键录入，自动校验修饰键与冲突。

## 主要功能

- 文字、链接与 PNG / DIB 图片历史；中文子串搜索（正文、来源、备注、分组）、类型筛选、虚拟列表。
- 收藏、备注、多分组归属；按住 Alt 点击加入排列队列，支持上移/下移、下一条、合并粘贴、恢复上次排列和命名队列。
- 去空白、合并空行、大小写转换与 JSON 格式化，先预览再复制或粘贴。
- 全局快捷键、托盘菜单、固定面板、失焦隐藏、单实例。
- 输入 `=` 进入临时固定的计算器；实时计算、连续运算、结果复制和最近计算，Esc 返回剪贴板。
- 浅色 / 深色「精密仪器」主题，界面状态独立持久化。

## 数据位置与边界

- Windows：`%LOCALAPPDATA%\local.clibo.native\history.db`；macOS：`~/Library/Application Support/local.clibo.native/`。可用 `CLIBO_DATA_DIR` 覆盖用于隔离测试。
- 正文、来源、图片与设置使用 DPAPI（Windows）/ AES-256-GCM + 钥匙串（macOS）加密，绑定当前用户；加密数据库不可跨平台或跨账户直接复制。
- 不上传剪贴板内容；写回剪贴板时带禁止云剪贴板上传标记。默认保留 2,000 条 / 30 天，收藏不按时间过期。
- 旧 Tauri 版数据目录（`local.clibo.desktop`）不做迁移，两版数据互不影响。

## 工程结构

| 路径 | 用途 |
|---|---|
| `native/src/ui.rs` | 界面布局、主题、交互与快捷键录入 |
| `native/src/backend.rs` | 采集、粘贴、队列与后台任务协调 |
| `native/src/store.rs` | SQLite 存储、去重、清理和事务 |
| `native/src/platform.rs` | Windows 剪贴板、图片格式与回粘 |
| `native/src/model.rs` | 数据模型、设置与敏感内容规则 |
| `native/README.md` | 构建方法与迁移说明 |
| `docs/` | 历史设计、竞品分析与各版本验证记录 |

## 开发

需要 Rust 1.95.0（`rust-toolchain.toml` 已固定）、Visual Studio C++ Build Tools。

```powershell
cargo test --locked --manifest-path native/Cargo.toml
cargo clippy --locked --manifest-path native/Cargo.toml -- -D warnings
cargo build --release --locked --manifest-path native/Cargo.toml
```

`npm run native:build` 在构建后将程序、说明与 SHA256 导出到 `output/native/`；该 Node 脚本仅用于导出，运行程序不需要 Node。

## 发布

当前发布形态是 Windows x64 便携包，不提供安装器。发布前依次执行测试、Clippy 和构建导出：

```powershell
cargo test --locked --manifest-path native/Cargo.toml
cargo clippy --locked --manifest-path native/Cargo.toml -- -D warnings
npm run native:build
Compress-Archive -Path output/native/* -DestinationPath output/Clibo-0.4.0-windows-x64.zip
```

`output/native/` 中的 `Clibo.exe`、`使用说明.md` 和 `SHA256.txt` 是便携包内容。推送形如 `v0.4.0` 的 Git tag 后，`.github/workflows/release.yml` 会自动构建 Windows 版本并创建 GitHub Release。当前 macOS 尚未完成 `.app` 打包、签名和实机验收。
