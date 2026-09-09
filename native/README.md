# Clibo Native 0.4.0 迁移预览

Rust + egui/eframe + Glow。使用原生窗口直接绘制，无 Tauri、React、Node 或 WebView 运行时依赖。Windows 和 macOS 共用界面、SQLite、搜索、分组、队列和文字处理代码。

## 构建运行

需要 Rust 1.95.0（仓库已固定该版本）；Windows 需要 Visual Studio C++ Build Tools，Mac 需要 Xcode Command Line Tools。

```sh
cargo run --manifest-path native/Cargo.toml
cargo build --release --locked --manifest-path native/Cargo.toml
cargo test --locked --manifest-path native/Cargo.toml
```

Windows 输出 `native/target/release/clibo-native.exe`。macOS 输出 `native/target/release/clibo-native`。本机旧 stable 工具链损坏时，可使用已安装的 `cargo +1.95.0`。

仓库已固定 Rust 1.95.0。`npm run native:build` 同时将程序、说明及校验值导出到 `output/native/`，该 Node 脚本仅用于构建导出，运行程序不需要 Node。

## 已接入

- 搜索框以 `=` 或 `＝` 开头进入计算器：临时固定、十进制四则运算、括号、百分数、实时结果和本次运行最近 50 条计算。
- 计算器中 Enter 确认，确认后输入运算符续算、输入数字开始新算式；Shift+Enter 复制完整结果，Esc 返回并恢复原固定状态。百分号始终除以 100；例如 `200+10%` 为 `200.1`。
- 搜索以等号开头的剪贴板文本时使用 `\=` 前缀。计算记录只在当前进程内保留，重启清空；实时预览不会写剪贴板。
- 中文搜索（正文、来源、备注、分组）、类型筛选、虚拟列表、文字及图片缩略预览。
- 浅色/深色主题切换（ui-state.json 持久化）；列表圆角卡片行、时间标签与图片行缩略图（LRU 纹理缓存）。
- 剪贴板后台采集、暂停/恢复、敏感内容过滤、容量和保留期限。
- 收藏、删除、确认清理、备注和多个分组归属、创建和解散分组。
- 排列队列、上移/下移、下一条、合并粘贴、恢复上次排列。
- 去空白、合并空行、大小写及 JSON 格式化；复制使用完整处理结果。
- 全局快捷键、托盘菜单、隐藏/唤醒、固定面板、失焦隐藏、单实例文件锁。
- Enter 粘贴、Shift+Enter 复制、方向键和 PageUp/PageDown 导航、Esc 清空搜索/收起。输入法组词和备注编辑不触发粘贴。
- 隐藏时不持续刷新 UI；采集线程按需通知可见窗口。行缩略纹理由 24 张 LRU 缓存约束，每帧解码条数设预算；隐藏时全部释放。

## 数据与并行试用

默认独立目录，不自动导入旧版，避免两个进程同时操作旧库：

- Windows：`%LOCALAPPDATA%/local.clibo.native/history.db`
- macOS：`~/Library/Application Support/local.clibo.native/history.db`
- 可用 `CLIBO_DATA_DIR` 覆盖，用于隔离测试。

Windows 使用原有 DPAPI 加密；Mac 使用 AES-256-GCM，随机密钥保存在登录钥匙串，无明文回退。两端的加密数据库不可直接互换。

旧版程序（0.3.x Tauri 版，代码已移除）若仍在运行会占用 Ctrl+Alt+V/Q 并持续显示快捷键冲突；请从托盘退出旧版。托盘菜单可以打开窗口。再次启动只提示已有实例，不会开第二个数据库连接。

## macOS 状态

已经提供 AppKit 剪贴板变化检测、文字/图片读写、NSRunningApplication 激活、CoreGraphics Command+V 和钥匙串加密适配。自动粘贴需要用户在「系统设置 → 隐私与安全性 → 辅助功能」授予权限；无权限时保留复制结果并提示手动粘贴。

这些代码尚未在 Mac 实机编译/运行验收。CI 文件提供 Windows/macOS 检查入口，配置文件的存在不代表已经通过。Mac 剪贴板来源使用捕获时的前台应用 Bundle ID，是近似值；变化计数每 500ms 检查一次，可能合并快速连续复制。

## 当前迁移差异

这是可运行的原生版本，尚未达到旧版所有交互的一致性：队列目前用上移/下移按钮；跨应用拖拽、查看原图、分组改名、登录启动、鼠标附近定位以及 macOS .app 打包签名尚未接入。使用系统标题栏移动窗口。

实测内存应统计 release 进程的工作集和私有内存，并测试文字/图片、隐藏和反复打开场景；不把可执行文件大小或空窗口内存当作完整使用量。

## 隔离演示

`CLIBO_DATA_DIR` 指向空目录后，可传 `--demo` 写入三条固定演示文字，默认暂停采集并使用 Ctrl+Alt+F11/F12。该开关拒绝非空历史。`--background` 在托盘可用时后台启动。

`--pinned` 启动后固定窗口。`--demo --smoke` 用于隔离渲染检查：程序在数据目录导出自身界面的 `native-smoke.png` 后自动退出（15 秒超时退出，检查脚本需断言图片存在）。不截图其他应用，不启用剪贴板采集。

追加 `--calculator` 检查计算器，`--small`（仅 smoke）检查 620×440 最小尺寸。隔离计算器演示可用 `CLIBO_CALCULATOR_EXPRESSION` 指定初始算式，默认 `128*3+56`。这些演示数据仅在显式 `--demo` 且指定独立数据目录时加载。

计算精度采用 rust_decimal 的十进制范围（约 28–29 位有效数字，小数位最多 28 位），默认显示约 12 位有效数字；“更多位数”和复制使用完整内部值。不能表示的极小值/极大值提示超出范围，不静默变成零。

## Windows 本机验证（2026-09-07）

- release 编译通过；21 项测试通过，包含共享业务、DPAPI、迁移恢复、图片编码和新搜索适配。
- 实际依赖树未出现 Tauri / wry / WebView。
- 程序自身渲染检查已导出 PNG；中文和预览可见，长摘要限制在固定行高内。
- 三条演示文字、暂停采集的 release 实例，采样工作集约 79 MiB，私有内存约 95 MiB；这是小数据场景的单次采样，不代表大图、完整历史或所有设备的占用，也没有与旧版做相同条件的对照。
- 桌面自动化应用授权超时，真实外部应用回粘、快捷键/托盘反复唤醒、中文输入法和多屏 DPI 仍需交互验收。macOS 需实机编译和权限验收。

## 界面重设计（2026-09-07，仪器风格）

- 双主题改为「精密仪器」方向：深色暖黑曜石 + 琥珀信号色，浅色暖纸面 + 墨色；顶部 2px 琥珀条与方块品牌标记。
- 元数据、计数、快捷键提示与筛选片统一等宽字体（Windows 用 Cascadia Mono/Consolas，macOS 用 Menlo，CJK 仍为回退）。
- 头部记录状态改为 ●/○ 药丸开关；筛选改为描边药丸片；行卡片选中态为琥珀描边 + 左侧竖条，悬停 80ms 渐变；类型标签改为 IMG/URL/TXT 描边签，来源左置、时间右置。
- 预览面板分区加横线小标题，主操作（粘贴、保存）为琥珀实心按钮，危险操作仅红色文字；设置/确认对话框同步重排。
- 快捷键设置从手填文本改为按键录入：点击组合框后直接按键（支持字母、数字、F1–F35、方向键和常用标点，修饰键 Ctrl/Alt/Shift，macOS 另支持 Cmd），实时显示按下的修饰键；必须有 Ctrl/Alt（macOS 或 Cmd）修饰，Esc 取消，自动拒绝与另一快捷键相同的组合。录入写入标准字符串（如 `control+alt+KeyV`），显示层转成「Ctrl + Alt + V」。
- 修复录入无法识别 V/C/X：egui-winit 会把 Ctrl/Cmd+V、C、X 转成剪贴板事件并丢弃按键事件（剪贴板为空时连事件都没有）。现从 Paste/Copy/Cut 事件还原按键，Windows 上再用 GetAsyncKeyState 轮询原始键态兜底（仅窗口聚焦时轮询）；顺带支持录入 Win 组合键（macOS 无对应 API，仍不可录）。
- 默认呼出快捷键改为 Ctrl+Shift+V（原 Ctrl+Alt+V，仅影响全新数据目录，已保存设置不变）。
- 仅改 `src/ui.rs` 表现层（及共享 `model.rs` 默认值），交互与数据逻辑不变；30 项测试通过（新增默认键位与 VK 表覆盖 2 项），Clippy 无新增警告，`--demo --smoke` 截图检查通过。

## 内存懒加载与主题重设计（2026-09-07）

- 常驻索引只保留视图元数据、搜索副本与去重指纹；原文按需从加密库解密（`Store::secret`），双 manifest 各 25/22 项测试通过。
- 同库对照实测（20 条互异 1MB 文本、release、可见窗口稳态三采样）：旧版 220 MiB 工作集 / 228 MiB 私有内存，新版 154 / 169 MiB，工作集约 -30%。
- `--demo --smoke` 双主题截图检查通过；Clippy 无新增警告（沿用 1 条历史提示：`use_clip` 参数数）。
