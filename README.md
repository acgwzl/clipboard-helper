# Clipboard Helper / 剪贴板助手

[中文说明](#中文说明)

Clipboard Helper is a local clipboard history and quick paste panel built with Tauri 2, Vue 3, TypeScript, and Rust.

It runs in the background, records text and image clipboard entries, and lets you bring up a compact command-style window to search, organize, and paste previous clipboard content back into the current app.

## Features

- Background clipboard monitoring for text and images
- Text deduplication by content, with repeated text moved back to the top
- Image capture as local PNG files
- Global hotkey launcher, defaulting to `Ctrl+Shift+V`
- Search across text and tags
- Filters for item type, date, copy count, regular expressions, and case sensitivity
- Tags, favorites, date grouping, and batch actions
- Quick paste into the active application
- Copy-only mode when auto-paste is not wanted
- Image preview, link opening, JSON formatting, and domain copying
- Compact mini mode and full management mode
- Always-on-top toggle
- 12 built-in themes
- Privacy rules for ignored apps, sensitive window titles, and temporary pause
- System tray menu with the latest 5 clipboard entries
- JSON import and export
- Usage statistics and top copied items

## Shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+V` | Show or hide the app window. This can be changed in Settings. |
| `Ctrl/Cmd + K` | Focus the search box. |
| `Ctrl/Cmd + M` | Toggle mini mode. |
| `Up` / `Down` | Move through the result list. |
| `Enter` | Copy and auto-paste the selected item. |
| `Ctrl + Enter` | Copy the selected item without auto-pasting. |
| `Space` | Preview the selected item. |
| `Tab` | Switch between All, Text, Images, and Favorites. |
| `Esc` | Close dialogs, leave selection mode, or hide the main window. |

## Tech Stack

- Tauri 2
- Vue 3
- TypeScript
- Rust
- SQLite via `rusqlite`

## Development

Install dependencies:

```bash
npm install
```

Start the Tauri development app:

```bash
npm run tauri dev
```

The main frontend file is `src/App.vue`.

The main backend file is `src-tauri/src/lib.rs`.

## Build

Build a standalone executable:

```bash
npx tauri build --no-bundle
```

Build an NSIS installer:

```bash
npx tauri build --bundles nsis
```

More Windows packaging notes are available in `BUILD.md`.

## Release Artifacts

Local release files are placed in `release/`.

The `release/` directory is ignored by Git. Upload release files through GitHub Releases instead of committing them to the repository.

## Generated Files

The following directories are generated and should not be committed:

- `node_modules/`
- `dist/`
- `src-tauri/target/`
- `release/`

Run `npm install` before development or rebuilding after a clean checkout.

## License

This project is licensed under the terms in `LICENSE`.

---

## 中文说明

剪贴板助手是一个本地运行的剪贴板历史与快速粘贴工具，基于 Tauri 2、Vue 3、TypeScript 和 Rust 构建。

它会在后台记录系统剪贴板中的文本和图片，需要时可以通过快捷键唤出一个类似命令面板的窗口，用来搜索、整理并快速粘贴历史内容。

## 功能

- 后台监听系统剪贴板，保存文本和图片历史
- 文本按内容去重，重复复制时刷新到顶部
- 图片转为 PNG 文件保存到本地库
- 默认使用 `Ctrl+Shift+V` 全局快捷键唤起窗口
- 支持搜索文本和标签
- 支持按类型、时间、复制次数、正则、大小写筛选
- 支持标签、收藏、日期分组和批量操作
- 点击条目后写入剪贴板，并可自动粘贴到当前应用
- 支持只写入剪贴板，不自动粘贴
- 支持图片预览、打开链接、格式化 JSON、复制域名
- 支持小窗模式和完整管理模式
- 支持窗口置顶
- 内置 12 套主题
- 支持隐私规则：忽略应用、忽略敏感窗口标题、临时暂停监听
- 系统托盘菜单支持最近 5 条快捷粘贴
- 支持 JSON 导入 / 导出
- 支持使用统计和高频条目查看

## 快捷键

| 快捷键 | 作用 |
|---|---|
| `Ctrl+Shift+V` | 显示或隐藏窗口，可在设置中修改 |
| `Ctrl/⌘ + K` | 聚焦搜索框 |
| `Ctrl/⌘ + M` | 切换小窗模式 |
| `↑` / `↓` | 上下选择条目 |
| `Enter` | 复制并自动粘贴选中条目 |
| `Ctrl + Enter` | 只写入剪贴板，不自动粘贴 |
| `Space` | 预览选中条目 |
| `Tab` | 在全部、文本、图片、收藏视图之间切换 |
| `Esc` | 关闭弹窗、退出多选或隐藏主窗口 |

## 技术栈

- Tauri 2
- Vue 3
- TypeScript
- Rust
- SQLite / `rusqlite`

## 开发

安装依赖：

```bash
npm install
```

启动开发模式：

```bash
npm run tauri dev
```

前端主文件：`src/App.vue`

后端主文件：`src-tauri/src/lib.rs`

## 构建

生成可直接运行的 exe：

```bash
npx tauri build --no-bundle
```

生成 NSIS 安装器：

```bash
npx tauri build --bundles nsis
```

更详细的 Windows 打包说明见 `BUILD.md`。

## 发布文件

本地构建产物放在 `release/` 目录。

`release/` 已被 Git 忽略。请通过 GitHub Releases 上传 exe 或安装器，不要直接提交到源码仓库。

## 可重新生成的目录

以下目录都是构建或依赖产物，不需要提交：

- `node_modules/`
- `dist/`
- `src-tauri/target/`
- `release/`

从干净仓库重新开发或构建前，先执行 `npm install`。

## 许可证

本项目遵循 `LICENSE` 文件中的许可证条款。
