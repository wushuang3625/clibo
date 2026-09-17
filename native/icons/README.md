# Clibo 应用图标

深靛蓝圆角底，白色剪贴板组成字母 C，薄荷绿箭头表示快捷启动。

- `clibo-master.png`：使用内置 imagegen 生成的透明背景原稿。
- `clibo.png`：256 px 窗口和任务栏图标。
- `clibo-tray.png`：32 px 托盘图标。
- `clibo.ico`：包含 16、24、32、48、64、128、256 px 的 Windows 图标。
- `clibo.rc`：由 `build.rs` 自动嵌入 Windows EXE 的图标资源。

重新导出：在项目根目录运行 `cargo run --locked --manifest-path native/Cargo.toml --example export_icon`。导出仅做透明 PNG 缩放与 ICO 封装，保留原稿。

## 原稿生成提示词

Use case: logo-brand. Asset type: final production Windows desktop application icon for Clibo, a private clipboard manager and instant app launcher. Create ONE original icon, square 1024x1024 PNG with genuinely transparent background outside its rounded-square tile. Design a bold, exceptionally clean rounded-square deep indigo tile (#242A63), with a subtle periwinkle-blue highlight towards its upper-left, no outer drop shadow. Inside, a thick, warm-white geometric clipboard outline cleverly forms a distinctive open C; a small integrated top clip identifies clipboard history. At the upper-right opening, a single vivid mint/aqua northeast arrow conveys quick launch. Use very few large confident shapes, 80% occupied canvas, generous internal negative space, carefully balanced optical proportions, crisp antialiased vector-like edges, flat premium utility-app aesthetic. The central white symbol and mint arrow must remain unmistakable at 16 and 32 px. Front-facing, perfectly level, symmetric rounded tile, no perspective. No writing, no wordmark, no extra letters, no badge, no mockup, no presentation board, no extra icons or variations, no hairline detail, no texture, no decorative particles, no heavy 3D. Output a standalone polished production icon, not a concept sheet.
