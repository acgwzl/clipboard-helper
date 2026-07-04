# 优化清单 — 2026-07-03 审查

> 产生方式:三个维度的审查代理(后端性能 / 前端性能 / 交互与配置)逐条读码后输出,其中标注 **[已人工核实]** 的条目由主会话二次确认过代码。按优先级排列,每条附具体改法。

## P0 · 正确性 bug(建议最先修)

### 1. 选取图片条目必然重复入库 [已人工核实]
[lib.rs:638-639](../src-tauri/src/lib.rs#L638) `pick_item` 图片分支用 `std::fs::read` 对 **PNG 文件字节** 哈希写入 `last_hash`;而轮询([lib.rs:1818](../src-tauri/src/lib.rs#L1818))对剪贴板图片计算的是 **解码后 RGBA 像素** 的哈希——两者永不相等,且轮询对图片没有内容去重。结果:每从历史选一张图,≤500ms 后就多一条重复记录 + 一份全尺寸 PNG 文件。`paste_sequence` 末条为图片时同理(lib.rs:753-759)。
**改法**:改为 `hash_bytes(img.rgba())`(与轮询同数据源);长期给 clips 表加 `image_hash` 列,轮询插入图片前按哈希去重。

### 2. 老化清理泄漏图片文件 [已人工核实]
`prune_old`([lib.rs:271-292](../src-tauri/src/lib.rs#L271))只删 DB 行,把待删的 `image_path` 返回给调用方——但全部 6 个调用点(697 / 1032 / 1056 / 1096 / 1807 / 1828)都是 `let _ = prune_old(&conn);`,路径被丢弃,没有任何一处删文件。图片被挤出 200 条上限后 PNG 永久残留,磁盘无上界增长。
**改法**:在 `prune_old` 内部对返回路径逐个 `fs::remove_file`(或用 `DELETE ... RETURNING image_path` 一条语句);启动时做一次孤儿文件清扫,回收已泄漏空间。

### 3. 「全部」视图排序:拖拽排序过的收藏项永久霸占顶部 [已人工核实,刚提交 0aa2ba5 的边界情况]
[lib.rs:234](../src-tauri/src/lib.rs#L234) `ORDER BY sort_order ASC NULLS LAST` 会把所有**有** sort_order 值的条目排在全部 NULL 条目之前;而 sort_order 只在收藏视图拖拽时写入、普通条目永远是 NULL、`toggle_pin` 取消收藏也不清除它。于是只要在收藏页拖过一次排序,这些条目就永远钉在「全部/文本/图片」顶部,与"按时间混排"的意图矛盾,还污染 Alt+1-9 序号和小窗前 5 条。
**改法**:`query_all_items` 只按 `created_at DESC` 排;把 sort_order 返回给前端,仅收藏视图在前端按它排序;`toggle_pin` 取消收藏时 `UPDATE clips SET sort_order=NULL`。

### 4. 启动时全局快捷键注册失败 → 应用直接崩溃
[lib.rs:1655](../src-tauri/src/lib.rs#L1655) `register(...)?` 沿 `?` 传播到 [lib.rs:1879](../src-tauri/src/lib.rs#L1879) 的 `.expect()` panic 退出。Ctrl+Shift+V 是热门组合,被其它工具抢注时应用启动即消失:无窗口、无托盘、无提示。
**改法**:`if let Err(e) = register(...)` 仅记录并继续启动,可回退尝试默认组合;失败状态记入 AppState,设置面板提示"快捷键被占用,请重新设置"。

### 5. 快捷键可录入无修饰键单键,保存后全系统吞键
[App.vue:658](../src/App.vue#L658) `captureHotkey` 只排除纯修饰键本身,按字母 A 即得 `"A"`;后端 `parse_shortcut`(lib.rs:497-531)对空修饰键照样注册。之后系统任何地方按 "a" 都被本应用抢走,组合还持久化,普通用户难自救。
**改法**:保存前校验必须含 Ctrl/Alt/Meta 之一(F1-F12 可例外);后端 `parse_shortcut` 同步兜底返回 Err。

## P1 · 性能

### 6. 500ms 盲轮询:内容未变也全量读图+哈希
轮询([lib.rs:1761-1837](../src-tauri/src/lib.rs#L1761))每 tick 无条件:锁 DB 读隐私设置、查前台窗口(Win32 三连调用)、剪贴板挂着图片时整张位图拷出再 `to_vec()` 再哈希(4K 截图约 33MB,2Hz 空转)。
**改法**:每 tick 先调 `GetClipboardSequenceNumber()`(极廉价),序列号没变直接 continue;隐私设置缓存进 AppState 而非每 tick 查库;`hash_bytes(&img.rgba().to_vec())` 去掉 `to_vec()`(1780、1818 两处)。彻底方案:message-only 窗口 + `AddClipboardFormatListener` 事件驱动。

### 7. 图片无缩略图 + base64 IPC + imageCache 无淘汰
[lib.rs:596-605](../src-tauri/src/lib.rs#L596) 全尺寸 PNG → base64(膨胀 1.33×)→ IPC;前端 [App.vue:114](../src/App.vue#L114) `imageCache` 只增不减、删除条目也不清理,几十张截图 = 数百 MB 常驻内存;列表 60px 缩略图背着 4K 全图解码。
**改法**:入库时生成最长边 ~320px 缩略图(`{uuid}.thumb.png`,clips 表加 thumb_path),列表用缩略图;进一步开启 assetProtocol(scope 限 images 目录)+ `convertFileSrc`,base64 通道与 imageCache 整体删除。

### 8. clips-changed 全量刷新,窗口隐藏时也照跑
每次捕获 emit → 前端 `refresh()` 全表 `get_items`(无 LIMIT、含完整正文,单条可达 64KB)→ 整列表重渲染([App.vue:1394](../src/App.vue#L1394)、[lib.rs:590](../src-tauri/src/lib.rs#L590))。
**改法**:事件携带增量 payload 就地插入;或至少 SELECT 用 `substr(text,1,500)` 只传预览 + 按需取全文;隐藏时置 dirty 标志、显示时刷一次;refresh 加 300ms debounce。

### 9. 列表渲染:每行十余个派生函数全量重算 + O(n²) 序号查找
每行模板调 `highlightedParts`(每行重编译正则)、`looksJson`(全文 JSON.parse ×3 次/行)、`preview`(全文正则替换)等([App.vue:1646](../src/App.vue#L1646) 起);`quickIndex` 每行 `findIndex` 全表扫描([App.vue:571](../src/App.vue#L571)),200 条一次渲染 ≈ 6-12 万次比较;且每个按键/方向键/30s tick 都触发整列表重渲染。
**改法**:派生元数据做成 `computed Map<id, meta>` 查表;正则用 computed 缓存单实例;序号用 v-for 的 index;行内容套 `v-memo`;最后一步可用已安装的 @vueuse/core `useVirtualList` 只渲染可视行。

### 10. OCR 阻塞 async runtime worker
`extract_text_from_image` 是 async 但内部全是同步重活:逐像素预处理、3200px 重采样、5 处 WinRT `.get()` 阻塞([lib.rs:1586](../src-tauri/src/lib.rs#L1586)、1458-1488、1525-1569)。OCR 期间轮询和其它命令可能被卡。
**改法**:整体包进 `tauri::async_runtime::spawn_blocking`,数行改动。

### 11. 多行写操作无事务、未开 WAL
`update_sort_order` 拖一次 = 200 个独立事务([lib.rs:838-848](../src-tauri/src/lib.rs#L838));import/batch_delete/batch_pin/prune_old 同样逐条 autocommit;持 Mutex 期间全部命令排队。
**改法**:`init_db` 后执行 `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;`;循环写包进单事务;`prune_old` 可换成一条 `DELETE ... WHERE id NOT IN (... LIMIT 200) RETURNING image_path`。

### 12. refresh 串行逐张加载图片,每张触发一次全列表重渲染
[App.vue:333-342](../src/App.vue#L333) for+await 逐张 IPC;imageCache 每新增一个 key 就全组件重渲染一次。
**改法**:并发(限 4-6)+ 结果攒齐后一次性 `Object.assign`;改 asset 协议后此路径整体消失。

### 13. 30 秒定时器:隐藏时空转,隐私状态无谓轮询
[App.vue:1399-1403](../src/App.vue#L1399) `now` tick 触发全列表重渲染,窗口藏在托盘也不停;`loadPrivacyStatus` 只有设置弹窗展示却常年轮询(后端还要查 Win32 前台窗口)。
**改法**:`visibilitychange` 时暂停/恢复;privacy 轮询只在设置弹窗打开期间进行。

## P2 · 可靠性 / 交互

### 14. 自动粘贴:固定 120ms 赌焦点 + 物理 Shift 未松开会触发自家全局热键
[lib.rs:658-663](../src-tauri/src/lib.rs#L658) sleep(120ms) 后 enigo 发 Ctrl+V;用户用 Ctrl+Shift+V 唤起后快速回车时,物理 Shift 仍按着 → 系统收到 Ctrl+Shift+V = 本应用热键,窗口又弹回来。
**改法**:粘贴前用 `GetAsyncKeyState` 等修饰键全部物理松开(带超时,Ditto 的标准做法);以"前台窗口不再是本应用"代替固定 sleep。

### 15. 轮询任务锁中毒即静默死亡;insert 失败内容无声丢失
轮询内 6 处 `.lock().unwrap()`([lib.rs:1769](../src-tauri/src/lib.rs#L1769) 起),任何命令线程持锁 panic 后下一 tick 轮询 panic 且不重启——捕获从此停摆无日志。且先更新 last_hash 再 `let _ = insert_text(...)`,失败既不入库也不重试。
**改法**:`lock().unwrap_or_else(|p| p.into_inner())` 或换 parking_lot;insert 成功后才更新 last_hash;循环体隔离单 tick 失败。

### 16. 托盘「清空未收藏历史」零确认,一次误点物理删除全部
[lib.rs:1691-1696](../src-tauri/src/lib.rs#L1691) 直接 `clear_history`(删记录+删图片文件),与「显示窗口」「退出」相邻;应用内同功能有 confirm,托盘裸奔。
**改法**:用已装的 plugin-dialog 弹 ask 确认;或 show 窗口 + emit 事件由前端弹主题化确认框。

### 17. 小窗模式下所有 toast 不可见
`showToast` 写入的 `dragDebug` 只渲染在 `<footer v-if="!miniMode">` 里([App.vue:1903-1906](../src/App.vue#L1903)),小窗恰是拖拽/快贴最常用形态,操作成败零反馈。
**改法**:toast 拆成独立固定定位浮层挂 .app 根部;dragDebug(调试)与 toast(反馈)分成两个变量。

### 18. 破坏性操作用原生 confirm()/alert()
clearAll/batchDelete/导入导出结果([App.vue:519](../src/App.vue#L519)、552、635-651)弹 WebView2 系统框,与 12 套主题割裂,且 Tauri 不保证各平台可用(confirm 恒 false 时静默失败)。
**改法**:复用现有 modal 体系做应用内确认,或 plugin-dialog 的 ask()/message()。

### 19. 正则搜索语义不对称
正则模式只搜文本条目正文([App.vue:211](../src/App.vue#L211)),图片连标签都搜不到;普通模式搜 text+tags。切换开关结果集突变。
**改法**:正则复用相同 haystack(`text + tags`),去掉 content_type 过滤。

### 20. 导出不带图片、导入产生共享文件的多行,删除互相误伤
export 只写含绝对路径的 JSON;import 时路径存在才收、且直接引用原文件不复制([lib.rs:1346](../src-tauri/src/lib.rs#L1346))——重复导入产生多行指向同一 PNG,`delete_item` 又会物理删文件,删一条把别条的图也删没。
**改法**:导出 zip(data.json + images/)或内嵌 base64 选项;导入把图片复制进 images_dir 换新 uuid,按内容哈希去重。

## P3 · 配置 / 工程卫生

### 21. ~~字体走 Google Fonts CDN~~ ✅ 已完成(2026-07-04)+ CSP 仍为 null
字体已改 @fontsource 本地打包(main.ts 引入 ibm-plex-sans 400-700 / zilla-slab 600-700 / courier-prime 400-700 / noto-serif-sc 700),CDN link 已从 index.html 删除,离线渲染有保证;代价:exe 13MB→21MB(Noto Serif SC 子集占大头,且 fontsource CSS 同时引用 woff+woff2,可再瘦身)。**遗留**:[tauri.conf.json:36](../src-tauri/tauri.conf.json#L36) CSP 仍为 null,建议补显式 CSP(img-src 需含 data: 供缩略图/二维码)。

### 22. 开机自启未实现
剪贴板工具的核心价值依赖常驻,重启后不手动打开就漏采。
**改法**:接 tauri-plugin-autostart,设置面板加开关;主窗口本就 visible:false,自启后天然静默驻留托盘。

### 23. 死依赖清理
npm:`@vueuse/core`(0 处 import,除非按 #9 采用其虚拟列表)、`@tauri-apps/plugin-clipboard-manager`、`@tauri-apps/plugin-global-shortcut`(两插件均纯 Rust 侧使用,前端 JS 包多余);Rust:`window-vibrancy`(Mica 永久关闭后 0 调用点,[Cargo.toml:45](../src-tauri/Cargo.toml#L45))、`tokio` full features 实际只用 sleep([Cargo.toml:36](../src-tauri/Cargo.toml#L36) 改 `features=["time"]`)。

### 24. 杂项修正
- `#[cfg_attr(mobile, tauri::mobile_entry_point)]` 被 OCR 代码段隔开,错误附着在 OCR 函数上而非 `run()`([lib.rs:1446](../src-tauri/src/lib.rs#L1446));
- lib.rs 文件头注释仍写"毛玻璃窗口"已过时(Mica 已按用户要求永久关闭);`get_has_mica` 恒返回 false,可与前端调用一并移除;
- `filtered` computed 里写 `searchError`(副作用,[App.vue:206](../src/App.vue#L206))→ 拆独立 computed;
- `tauri://focus` 监听未收集 unlisten([App.vue:1412](../src/App.vue#L1412)),HMR 时叠加重复处理器;
- `items`/`imageCache` 可改 `shallowRef`,省 200 对象深代理开销。

### 25. (可选)App.vue 拆分
3789 行单文件目前尚可维护,若继续加功能建议按痛点渐进拆:先抽 OCR 弹窗(约 400 行,最独立)、设置弹窗、ClipRow 行组件(配合 #9 的 v-memo),逻辑侧抽 useClipboardList / useKeyboardNav 两个 composable。不为拆而拆。
