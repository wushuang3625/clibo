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
- 内置临时计算器：在搜索框输入 = 进入计算模式。
- 时间戳转换：秒 / 毫秒、日期时间、UTC 偏移和批量转换。
- 浅色 / 深色主题和图片缩略图预览。
- 历史记录手动备份与恢复，恢复前自动创建安全备份。
- Windows 登录自启：无需管理员权限，登录后可在托盘后台运行。

## 界面预览

<p align="center">
  <img src="docs/images/clipboard.png" alt="剪贴板历史" width="32%">
  <img src="docs/images/calculator.png" alt="计算器" width="32%">
  <img src="docs/images/json-format.png" alt="JSON 格式化工作页" width="32%">
</p>

剪贴板历史、临时计算器和 JSON 工作页均为原生界面，数据处理在本地完成。

## 下载

Windows x64 用户可以直接下载 [Clibo 0.5.0](https://github.com/wushuang3625/clibo/releases/tag/v0.5.0) 便携版。解压后运行 Clibo.exe，不需要 Node、WebView 或其他运行时。

## 快速使用

1. 启动后点击“记录中”开始采集。
2. 在任意应用中复制文字、链接或图片。
3. 按 Ctrl + Shift + V 呼出 Clibo。
4. 搜索并选择记录，按 Enter 尝试粘贴，按 Shift + Enter 仅复制。
5. 在偏好设置中可以修改呼出和排列粘贴快捷键。
6. Windows 用户可在偏好设置的“启动”区域开启“开机自启”，登录后自动在托盘后台运行。开关立即生效，无需管理员权限；移动便携版程序后请关闭再重新开启。

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
