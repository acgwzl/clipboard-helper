# 优化清单 — 2026-07-03 审查

> 产生方式:三个维度的审查代理(后端性能 / 前端性能 / 交互与配置)逐条读码后输出,其中标注 **[已人工核实]** 的条目由主会话二次确认过代码。按优先级排列,每条附具体改法。

## P0 · 正确性 bug(建议最先修)

### 1. ~~选取图片条目必然重复入库~~ ✅ 已修复(c26050e,2026-07-04)
pick_item 与 paste_sequence 的图片哈希已改为解码后 RGBA(与轮询同数据源)。长期项仍可选:给 clips 表加 image_hash 列做图片内容去重。

### 2. ~~老化清理泄漏图片文件~~ ✅ 已修复(c26050e,2026-07-04)
`prune_old` 现在删行的同时 `fs::remove_file` 删 PNG;启动时 `cleanup_orphan_images` 清扫 images 目录中无 DB 记录引用的孤儿文件,自动回收旧版本泄漏的空间。

### 3. ~~「全部」视图排序:拖拽排序过的收藏项永久霸占顶部~~ ✅ 已修复(e19c999,2026-07-04)
后端只按 created_at 排序,sort_order 随条目返回;前端仅收藏视图按 sort_order 排(拖拽定位同步改用显示顺序);toggle_pin/batch_pin 取消收藏时清除 sort_order。

### 4. ~~启动时全局快捷键注册失败 → 应用直接崩溃~~ ✅ 已修复(e19c999,2026-07-04)
注册失败只 eprintln 并继续启动(托盘/窗口正常可用),可进设置重设快捷键。可选增强:失败状态存 AppState 并在设置面板显式提示。

### 5. ~~快捷键可录入无修饰键单键,保存后全系统吞键~~ ✅ 已修复(e19c999,2026-07-04)
前端 captureHotkey 与后端 parse_shortcut 双重校验:必须含 Ctrl/Alt/Win 之一,功能键 F1-F24 允许单独使用(Shift 单独不算,Shift+字母同样会吞大写输入)。

## P1 · 性能

### 6. 500ms 盲轮询 ⏳ 部分完成(c26050e:序列号门控 + 去 to_vec)
已加 `GetClipboardSequenceNumber()` 门控——内容未变的 tick 直接跳过(隐私查库/读图/哈希全部省掉);两处哈希去掉了多余的 `to_vec()` 拷贝。**剩余可选**:message-only 窗口 + `AddClipboardFormatListener` 彻底事件驱动(收益已大幅降低)。

### 7. ~~图片无缩略图 + base64 IPC + imageCache 无淘汰~~ ✅ 已修复(b6bddcf,2026-07-04)
入库生成 320px 缩略图(thumb_path 列,启动时为旧图后台补生成);前端全部 convertFileSrc 走 asset 协议(scope 限 $APPDATA/images),列表用缩略图、灯箱/OCR 用原图;imageCache/预载/淘汰与 read_image_as_data_url 命令整体删除。

### 8. clips-changed 全量刷新 ⏳ 部分完成(0540b75:300ms 防抖 + 隐藏标脏、显示补刷)
**剩余可选**:事件携带增量 payload 就地插入;SELECT 只传 `substr(text,1,500)` 预览 + 按需取全文(与 #9 一起做收益更大)。

### 9. ~~列表渲染:每行十余个派生函数全量重算 + O(n²) 序号查找~~ ✅ 已修复(90f2230)
列表行加 v-memo(导航/搜索/多选只重渲染受影响行);quickIndex 改 computed Map 查表 O(n);搜索正则缓存为 computed;looksJson 结果缓存;items 改 shallowRef。剩余可选:虚拟滚动(当前 200 条上限下收益有限)。
### 10. ~~OCR 阻塞 async runtime worker~~ ✅ 已修复(0540b75:整体挪入 spawn_blocking)

### 11. ~~多行写操作无事务、未开 WAL~~ ✅ 已修复(0540b75)
init_db 开启 WAL + synchronous=NORMAL;update_sort_order / batch_pin / batch_delete / import_history 循环写全部改单事务(unchecked_transaction)。prune_old 单次通常只删 1 行,保持原样。

### 12. ~~refresh 串行逐张加载图片,每张触发一次全列表重渲染~~ ✅ 已随 #7 消灭(b6bddcf)
base64 加载路径整体删除,图片由 WebView 经 asset 协议按需加载。

### 13. ~~30 秒定时器:隐藏时空转,隐私状态无谓轮询~~ ✅ 已修复(0540b75:隐藏时跳过 tick;隐私状态仅设置弹窗打开期间轮询)

## P2 · 可靠性 / 交互

### 14. ~~自动粘贴:固定 120ms 赌焦点 + 物理 Shift 未松开会触发自家全局热键~~ ✅ 已修复(c9d7693)
simulate_paste 前用 GetAsyncKeyState 轮询等 Ctrl/Shift/Alt/Win 全部物理松开(超时 1.5s)再发 Ctrl+V。
### 15. ~~轮询任务锁中毒即静默死亡;insert 失败内容无声丢失~~ ✅ 已修复(90f2230)
轮询/托盘/补缩略图统一 `lock_ok`(中毒时 into_inner 继续);落库成功后才更新 last_hash。
### 16. ~~托盘「清空未收藏历史」零确认~~ ✅ 已修复(c9d7693:plugin-dialog 原生警告确认,独立线程 blocking_show)
### 17. ~~小窗模式下所有 toast 不可见~~ ✅ 已修复(c9d7693:toast 拆为固定定位浮层挂 .app 根部,与 dragDebug 分离)
### 18. ~~破坏性操作用原生 confirm()/alert()~~ ✅ 已修复(c9d7693:应用内确认框 askConfirm(Enter 确定/Esc 取消)+ 全局 toast,6 处原生弹窗全部替换)
### 19. ~~正则搜索语义不对称~~ ✅ 已修复(90f2230:正则与普通搜索同 haystack(正文+标签),图片条目可按标签搜到)
### 20. 导出不带图片、导入产生共享文件的多行,删除互相误伤
export 只写含绝对路径的 JSON;import 时路径存在才收、且直接引用原文件不复制([lib.rs:1346](../src-tauri/src/lib.rs#L1346))——重复导入产生多行指向同一 PNG,`delete_item` 又会物理删文件,删一条把别条的图也删没。
**改法**:导出 zip(data.json + images/)或内嵌 base64 选项;导入把图片复制进 images_dir 换新 uuid,按内容哈希去重。

## P3 · 配置 / 工程卫生

### 21. ~~字体走 Google Fonts CDN~~ ✅ 已完成(2026-07-04)+ CSP 仍为 null
字体已改 @fontsource 本地打包(main.ts 引入 ibm-plex-sans 400-700 / zilla-slab 600-700 / courier-prime 400-700 / noto-serif-sc 700),CDN link 已从 index.html 删除,离线渲染有保证;代价:exe 13MB→21MB(Noto Serif SC 子集占大头,且 fontsource CSS 同时引用 woff+woff2,可再瘦身)。**遗留**:[tauri.conf.json:36](../src-tauri/tauri.conf.json#L36) CSP 仍为 null,建议补显式 CSP(img-src 需含 data: 供缩略图/二维码)。

### 22. 开机自启未实现
剪贴板工具的核心价值依赖常驻,重启后不手动打开就漏采。
**改法**:接 tauri-plugin-autostart,设置面板加开关;主窗口本就 visible:false,自启后天然静默驻留托盘。

### 23. ~~死依赖清理~~ ✅ 已完成(c9d7693)
npm 移除 @vueuse/core、@tauri-apps/plugin-clipboard-manager、@tauri-apps/plugin-global-shortcut;Cargo 移除 window-vibrancy,tokio 特性 full→time。
### 24. ~~杂项修正~~ ✅ 已全部完成(c9d7693 + 90f2230)
mobile_entry_point 归位 / 文件头注释 / get_has_mica 移除 / searchError 拆独立 computed / tauri://focus 收集 unlisten / items 改 shallowRef。
### 25. (可选)App.vue 拆分
3789 行单文件目前尚可维护,若继续加功能建议按痛点渐进拆:先抽 OCR 弹窗(约 400 行,最独立)、设置弹窗、ClipRow 行组件(配合 #9 的 v-memo),逻辑侧抽 useClipboardList / useKeyboardNav 两个 composable。不为拆而拆。
