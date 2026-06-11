# 剪贴板助手 — 打包说明

## 一、环境依赖

第一次构建需要装好以下工具：

| 依赖 | 用途 | 安装方式 |
|---|---|---|
| **Node.js 18+** | 前端 / Vite / npm | https://nodejs.org/ |
| **Rust 工具链** | 后端编译 | https://rustup.rs/ ， Windows 选 `x86_64-pc-windows-msvc` |
| **Visual Studio Build Tools** | MSVC 链接器 | 装 Rust 时会提示一并装上；勾选「使用 C++ 的桌面开发」 |
| **WebView2 Runtime** | 应用运行依赖 | Windows 11 自带；Win10 可从 [Microsoft 官网](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) 装 |

验证：
```bash
node -v        # >= 18
rustc --version
cargo --version
```

---

## 二、安装项目依赖

进入项目根目录后只需要一次：

```bash
cd C:\Users\PC\Desktop\clipboard-helper
npm install
```

这会装好 Vue、TypeScript、Vite、Tauri CLI、Dialog plugin 等。

> **首次启动很慢**：Cargo 需要从源码编译 400+ crates，约 5 ~ 8 分钟。后续增量编译只需几秒。

---

## 三、开发模式（热重载）

```bash
npm run tauri dev
```

- 改 Vue / TS 文件：浏览器视图立即更新
- 改 Rust 文件：Cargo 自动重新编译并重启 exe
- 关掉 dev：`Ctrl+C` 终端，或关掉窗口

---

## 四、生产构建

### 方案 A：只要 exe（最快，推荐自用）

```bash
npm run tauri build -- --no-bundle
```

输出文件：
```
src-tauri\target\release\clipboard-helper.exe       ≈ 12 MB
```

这是一个完全独立的可执行文件，可以直接双击运行，也可以拷到任何地方。

### 方案 B：带 Windows 安装包（分发用）

```bash
npm run tauri build
```

输出文件（在 `src-tauri\target\release\bundle\`）：
```
msi\剪贴板助手_0.1.0_x64_zh-CN.msi               ≈ 4 MB    （MSI 静默安装）
nsis\剪贴板助手_0.1.0_x64-setup.exe              ≈ 3 MB    （NSIS 自解压安装器）
```

两种安装包二选一即可，NSIS 体积更小，MSI 更适合企业部署。

### 方案 C：只想要某一种安装包

```bash
npm run tauri build -- --bundles msi      # 只生成 MSI
npm run tauri build -- --bundles nsis     # 只生成 NSIS
```

---

## 五、应用图标

源文件：`app-icon.svg`（项目根目录）

修改后重新生成全部尺寸：

```bash
npx tauri icon ./app-icon.svg
```

会自动覆盖 `src-tauri/icons/` 下的所有 ico / icns / png（Windows / macOS / iOS / Android）。

---

## 六、首次启动失败排查

| 现象 | 原因 | 处理 |
|---|---|---|
| `error: linker 'link.exe' not found` | 没装 VS Build Tools | 装 Visual Studio Build Tools，勾「C++ 桌面开发」 |
| `error: failed to run custom build command for 'webview2-com-sys'` | 缺 WebView2 | 装 WebView2 Runtime |
| `error: HotKey already registered` | 上次的 exe 没退干净 | 任务管理器结束 `clipboard-helper.exe` 后重试 |
| `error: Port 1420 is already in use` | 上次的 dev 没退干净 | 关旧的 vite，或改 `vite.config.ts` 端口 |
| `OS error 5: 拒绝访问` | 杀软（如 360 / Defender）拦截 | 临时退出 / 加白名单 |
| 黑屏 / 看不到内容 | WebView2 启动失败 | 装最新版 WebView2 Runtime |

---

## 七、文件结构（构建相关）

```
clipboard-helper/
├── app-icon.svg                 # 图标源文件
├── package.json                 # 前端依赖与脚本
├── package-lock.json
├── vite.config.ts               # Vite 配置（端口 1420）
├── index.html
├── tsconfig.json
├── src/                         # Vue + TS 源码（App.vue 主文件）
└── src-tauri/
    ├── Cargo.toml               # Rust 依赖
    ├── Cargo.lock
    ├── build.rs
    ├── tauri.conf.json          # 应用 / 窗口 / 打包配置
    ├── capabilities/            # Tauri v2 权限文件
    ├── icons/                   # 由 tauri icon 生成
    ├── src/lib.rs               # Rust 后端全部逻辑
    └── target/                  # 编译产物（构建时生成，可删）
```

---

## 八、清理编译冗余

Windows 终端（cmd 或 PowerShell）：

```bash
rd /s /q src-tauri\target
rd /s /q node_modules
rd /s /q dist
del build.log dev*.log kill.ps1
```

或在 bash / git-bash：

```bash
rm -rf src-tauri/target node_modules dist
rm -f build.log dev*.log kill.ps1
```

清理后下次 `npm install` + `npm run tauri build` 会重新生成。

---

## 九、打包后体积参考

| 项 | 大小 |
|---|---|
| `clipboard-helper.exe` (release) | ~ 12 MB |
| NSIS 安装包 | ~ 3 MB |
| MSI 安装包 | ~ 4 MB |
| 完整 `target/` 编译目录 | 2 ~ 4 GB |
| `node_modules/` | ~ 150 MB |

可以放心删 `target` 和 `node_modules`，源码 + `app-icon.svg` + `src-tauri/icons/` 加起来不到 1 MB，足以从头构建出完全相同的应用。

---

## 十、常用命令速查

```bash
# 装依赖
npm install

# 开发
npm run tauri dev

# 只编 exe
npm run tauri build -- --no-bundle

# 完整打包（exe + msi + nsis）
npm run tauri build

# 只生成 NSIS 安装器
npm run tauri build -- --bundles nsis

# 重新生成图标
npx tauri icon ./app-icon.svg

# 看版本
npx tauri info
```
