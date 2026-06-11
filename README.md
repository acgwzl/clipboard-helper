# 剪贴板助手

本地运行的剪贴板历史与快速粘贴工具，基于 Tauri 2、Vue 3 和 Rust 构建。

## 功能

- 后台监听系统剪贴板，保存文本和图片历史
- 文本重复时刷新时间，图片转为 PNG 存入本地库
- 全局快捷键唤起窗口，默认 `Ctrl+Shift+V`
- 搜索、视图筛选、时间筛选、复制次数筛选、正则和大小写匹配
- 标签、收藏、日期分组、批量收藏 / 取消收藏 / 删除
- 点击条目后写入剪贴板，并可自动粘贴到当前应用
- 图片预览、链接打开、JSON 格式化、复制域名
- 小窗模式、窗口置顶、12 套主题
- 隐私规则：暂停监听、忽略应用、忽略敏感窗口标题
- 系统托盘菜单和最近 5 条快捷粘贴
- JSON 导入 / 导出与使用统计

## 快捷键

| 快捷键 | 作用 |
|---|---|
| `Ctrl+Shift+V` | 默认全局唤起窗口，可在设置中修改 |
| `Ctrl/⌘ + K` | 聚焦搜索框 |
| `Ctrl/⌘ + M` | 切换小窗 / 完整模式 |
| `↑` / `↓` | 上下选择条目 |
| `Enter` | 复制并自动粘贴选中条目 |
| `Ctrl + Enter` | 仅写入剪贴板，不自动粘贴 |
| `Space` | 预览选中条目 |
| `Tab` | 切换全部 / 文本 / 图片 / 收藏视图 |
| `Esc` | 关闭弹窗、退出多选或隐藏窗口 |

## 开发

```bash
npm install
npm run tauri dev
```

前端入口在 `src/App.vue`，Rust 后端逻辑在 `src-tauri/src/lib.rs`。

## 构建

只生成可直接运行的 exe：

```bash
npx tauri build --no-bundle
```

生成 NSIS 安装器：

```bash
npx tauri build --bundles nsis
```

完整打包说明见 `BUILD.md`。

## 当前产物

已整理到 `release/`：

- `release/剪贴板助手.exe`
- `release/剪贴板助手_0.1.0_x64-setup.exe`

`dist/`、`node_modules/`、`src-tauri/target/` 都是可重新生成的构建产物，本目录已清理掉这些缓存。
