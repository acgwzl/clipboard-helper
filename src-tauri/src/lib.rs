// 剪贴板助手 - 完整版后端（v2 含 8 新功能）
//
// 功能：
// - SQLite 持久化（rusqlite，bundled 模式）
// - 文本 + 图片双格式支持（图片存为 PNG 文件，DB 存路径）
// - 全局快捷键（默认 Ctrl+Shift+V，可在前端自定义）唤起窗口
// - 系统托盘（左键切换窗口，右键菜单含最近 5 条）
// - 自动粘贴（enigo 模拟 Ctrl+V）
// - 收藏置顶
// - 纯实色窗口(Mica/毛玻璃已按用户要求永久关闭)
// - 拖拽入库（监听 tauri://drag-drop 后由前端调命令入库）
// - 文本工具栏（大小写、trim、URL/Base64/MD5）
// - 导入导出（JSON）
// - 批量操作（删除 / 收藏）
// - 使用统计（copy_count 字段）

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{
    LogicalSize, Size,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Listener, Manager, State,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const POLL_INTERVAL_MS: u64 = 500;
const MAX_UNPINNED_HISTORY: i64 = 200; // 未置顶的最大保留条数
const DEFAULT_HOTKEY: &str = "Ctrl+Shift+V";

// ================== 数据模型 ==================

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClipItem {
    pub id: i64,
    pub content_type: String, // "text" | "image"
    pub text: Option<String>,
    pub image_path: Option<String>,
    pub pinned: bool,
    pub created_at: i64, // unix ms
    pub copy_count: i64,
    pub tags: Vec<String>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub image_bytes: Option<i64>,
    // 后加的列:旧导出 JSON 里没有,导入时按缺省处理
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub thumb_path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Stats {
    pub total: i64,
    pub text_count: i64,
    pub image_count: i64,
    pub pinned_count: i64,
    pub total_copies: i64,
    pub top_items: Vec<ClipItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ExportPayload {
    version: u32,
    exported_at: i64,
    items: Vec<ClipItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrivacySettings {
    pub enabled: bool,
    pub ignore_incognito: bool,
    pub sensitive_title_keywords: Vec<String>,
    pub ignored_apps: Vec<String>,
    pub paused_until: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrivacyStatus {
    pub settings: PrivacySettings,
    pub paused_remaining_ms: i64,
    pub active_app: Option<String>,
    pub active_title: Option<String>,
    pub blocking_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ActiveWindowInfo {
    title: String,
    process_path: String,
    process_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct OcrCrop {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

struct AppState {
    db: Mutex<Connection>,
    last_hash: Mutex<u64>,
    images_dir: PathBuf,
    current_hotkey: Mutex<Option<Shortcut>>,
}

// ================== 数据库 ==================

fn init_db(db_path: &PathBuf) -> rusqlite::Result<Connection> {
    let conn = Connection::open(db_path)?;
    // WAL + NORMAL:写操作不再锁整库,轮询线程与 UI 命令并发更顺畅
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS clips (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content_type TEXT NOT NULL CHECK(content_type IN ('text','image')),
            text TEXT,
            image_path TEXT,
            pinned INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            copy_count INTEGER NOT NULL DEFAULT 0,
            tags TEXT NOT NULL DEFAULT '[]',
            image_width INTEGER,
            image_height INTEGER,
            image_bytes INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_clips_created ON clips(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_pinned  ON clips(pinned DESC, created_at DESC);

        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )?;
    // 平滑迁移：若旧版无 copy_count 列就加上
    let has_copy_count: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('clips') WHERE name='copy_count'")?
        .exists([])
        .unwrap_or(false);
    if !has_copy_count {
        let _ = conn.execute("ALTER TABLE clips ADD COLUMN copy_count INTEGER NOT NULL DEFAULT 0", []);
    }
    ensure_column(&conn, "clips", "tags", "TEXT NOT NULL DEFAULT '[]'")?;
    ensure_column(&conn, "clips", "sort_order", "INTEGER")?; // nullable,手动排序用
    ensure_column(&conn, "clips", "thumb_path", "TEXT")?; // 列表缩略图(最长边 320px)
    ensure_column(&conn, "clips", "image_width", "INTEGER")?;
    ensure_column(&conn, "clips", "image_height", "INTEGER")?;
    ensure_column(&conn, "clips", "image_bytes", "INTEGER")?;
    Ok(conn)
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!(
        "SELECT 1 FROM pragma_table_info('{}') WHERE name=?1",
        table.replace('\'', "''")
    ))?;
    let exists = stmt.exists(params![column]).unwrap_or(false);
    if !exists {
        conn.execute(
            &format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                table, column, definition
            ),
            [],
        )?;
    }
    Ok(())
}

fn parse_tags(raw: Option<String>) -> Vec<String> {
    let Some(raw) = raw else { return vec![]; };
    if raw.trim().is_empty() {
        return vec![];
    }
    serde_json::from_str::<Vec<String>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .map(|t| normalize_tag(&t))
        .filter(|t| !t.is_empty())
        .collect()
}

fn normalize_tag(tag: &str) -> String {
    tag.trim()
        .trim_start_matches('#')
        .chars()
        .take(24)
        .collect::<String>()
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for tag in tags {
        let t = normalize_tag(&tag);
        if !t.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
            out.push(t);
        }
    }
    out
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<ClipItem> {
    Ok(ClipItem {
        id: row.get(0)?,
        content_type: row.get(1)?,
        text: row.get(2)?,
        image_path: row.get(3)?,
        pinned: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        copy_count: row.get::<_, i64>(6).unwrap_or(0),
        tags: parse_tags(row.get::<_, Option<String>>(7).ok().flatten()),
        image_width: row.get::<_, Option<i64>>(8).ok().flatten().map(|v| v as u32),
        image_height: row.get::<_, Option<i64>>(9).ok().flatten().map(|v| v as u32),
        image_bytes: row.get::<_, Option<i64>>(10).ok().flatten(),
        sort_order: row.get::<_, Option<i64>>(11).ok().flatten(),
        thumb_path: row.get::<_, Option<String>>(12).ok().flatten(),
    })
}

fn query_all_items(conn: &Connection) -> rusqlite::Result<Vec<ClipItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, content_type, text, image_path, pinned, created_at, copy_count, tags, image_width, image_height, image_bytes, sort_order, thumb_path \
         FROM clips ORDER BY created_at DESC",
    )?;
    let items = stmt
        .query_map([], row_to_item)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
}

fn query_recent(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<ClipItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, content_type, text, image_path, pinned, created_at, copy_count, tags, image_width, image_height, image_bytes, sort_order, thumb_path \
         FROM clips ORDER BY created_at DESC LIMIT ?1",
    )?;
    let items = stmt
        .query_map(params![limit], row_to_item)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
}

fn insert_text(conn: &Connection, text: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO clips (content_type, text, image_path, pinned, created_at, copy_count, tags, image_width, image_height, image_bytes) \
         VALUES ('text', ?1, NULL, 0, ?2, 0, '[]', NULL, NULL, NULL)",
        params![text, Utc::now().timestamp_millis()],
    )?;
    Ok(conn.last_insert_rowid())
}

fn insert_image(
    conn: &Connection,
    path: &str,
    thumb_path: Option<&str>,
    width: u32,
    height: u32,
    bytes: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO clips (content_type, text, image_path, thumb_path, pinned, created_at, copy_count, tags, image_width, image_height, image_bytes) \
         VALUES ('image', NULL, ?1, ?2, 0, ?3, 0, '[]', ?4, ?5, ?6)",
        params![path, thumb_path, Utc::now().timestamp_millis(), width as i64, height as i64, bytes],
    )?;
    Ok(conn.last_insert_rowid())
}

fn prune_old(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id, image_path, thumb_path FROM clips \
         WHERE pinned = 0 \
         ORDER BY created_at DESC \
         LIMIT -1 OFFSET ?1",
    )?;
    let to_delete: Vec<(i64, Option<String>, Option<String>)> = stmt
        .query_map(params![MAX_UNPINNED_HISTORY], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut image_paths = vec![];
    for (id, path, thumb) in to_delete {
        conn.execute("DELETE FROM clips WHERE id = ?1", params![id])?;
        // 行删除的同时删除对应 PNG(含缩略图),否则被挤出历史的图片文件会永久残留磁盘
        for p in [path, thumb].into_iter().flatten() {
            let _ = std::fs::remove_file(&p);
            image_paths.push(p);
        }
    }
    Ok(image_paths)
}

/// 启动时清扫 images 目录中已无数据库记录引用的孤儿 PNG(回收旧版本泄漏的空间)
fn cleanup_orphan_images(conn: &Connection, images_dir: &std::path::Path) {
    use std::collections::HashSet;
    let mut referenced: HashSet<String> = HashSet::new();
    if let Ok(mut stmt) = conn.prepare("SELECT image_path, thumb_path FROM clips WHERE image_path IS NOT NULL") {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        }) {
            for (p, t) in rows.flatten() {
                for path in std::iter::once(p).chain(t) {
                    if let Some(name) = std::path::Path::new(&path).file_name() {
                        referenced.insert(name.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(images_dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_png = path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("png"))
            .unwrap_or(false);
        if !is_png {
            continue;
        }
        let orphan = path
            .file_name()
            .map(|n| !referenced.contains(&n.to_string_lossy().to_string()))
            .unwrap_or(false);
        if orphan {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn read_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

fn write_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

// ================== 工具函数 ==================

fn hash_bytes(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

/// 锁中毒(某持锁线程 panic)时取回内部数据继续运行,
/// 轮询与托盘等常驻路径不因一次 panic 永久停摆
fn lock_ok<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn toggle_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

/// 为图片生成列表缩略图(最长边 320px);原图本身足够小则返回 None(前端直接用原图)
fn make_thumbnail(src_path: &str) -> Option<String> {
    const THUMB_MAX: u32 = 320;
    let img = image::open(src_path).ok()?;
    if img.width() <= THUMB_MAX && img.height() <= THUMB_MAX {
        return None;
    }
    let thumb = img.thumbnail(THUMB_MAX, THUMB_MAX);
    let thumb_path = format!("{}.thumb.png", src_path.trim_end_matches(".png"));
    thumb
        .save_with_format(&thumb_path, image::ImageFormat::Png)
        .ok()?;
    Some(thumb_path)
}

fn save_clipboard_image(
    raw: &tauri::image::Image<'_>,
    images_dir: &PathBuf,
) -> Result<(String, Option<String>, u32, u32, i64), String> {
    use image::{ImageBuffer, Rgba};
    let width = raw.width();
    let height = raw.height();
    let rgba: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(width, height, raw.rgba().to_vec())
            .ok_or_else(|| "raw image size mismatch".to_string())?;

    let filename = format!("{}.png", uuid::Uuid::new_v4());
    let path = images_dir.join(&filename);
    rgba.save_with_format(&path, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let bytes = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
    let path_str = path.to_string_lossy().to_string();
    let thumb = make_thumbnail(&path_str);
    Ok((path_str, thumb, width, height, bytes))
}

fn load_image_for_clipboard(
    path: &str,
) -> Result<tauri::image::Image<'static>, String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Ok(tauri::image::Image::new_owned(rgba.into_raw(), w, h))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn default_privacy_settings() -> PrivacySettings {
    PrivacySettings {
        enabled: true,
        ignore_incognito: true,
        sensitive_title_keywords: vec![
            "password".into(),
            "passcode".into(),
            "signin".into(),
            "login".into(),
            "登录".into(),
            "密码".into(),
            "无痕".into(),
            "隐身".into(),
            "incognito".into(),
            "private browsing".into(),
        ],
        ignored_apps: vec![],
        paused_until: None,
    }
}

fn read_privacy_settings(conn: &Connection) -> PrivacySettings {
    read_setting(conn, "privacy")
        .and_then(|s| serde_json::from_str::<PrivacySettings>(&s).ok())
        .unwrap_or_else(default_privacy_settings)
}

fn write_privacy_settings(conn: &Connection, settings: &PrivacySettings) -> rusqlite::Result<()> {
    let json = serde_json::to_string(settings).unwrap_or_else(|_| "{}".to_string());
    write_setting(conn, "privacy", &json)
}

fn active_window_info() -> Option<ActiveWindowInfo> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, HWND};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        };

        unsafe {
            let hwnd: HWND = GetForegroundWindow();
            if hwnd.is_null() {
                return None;
            }

            let title_len = GetWindowTextLengthW(hwnd);
            let mut title_buf = vec![0u16; (title_len.max(0) as usize) + 1];
            let read = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
            let title = String::from_utf16_lossy(&title_buf[..read.max(0) as usize]);

            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let mut process_path = String::new();
            if pid != 0 {
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if !handle.is_null() {
                    let mut proc_buf = vec![0u16; 1024];
                    let mut size = proc_buf.len() as u32;
                    if QueryFullProcessImageNameW(handle, 0, proc_buf.as_mut_ptr(), &mut size) != 0 {
                        process_path = String::from_utf16_lossy(&proc_buf[..size as usize]);
                    }
                    CloseHandle(handle);
                }
            }
            let process_name = std::path::Path::new(&process_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            Some(ActiveWindowInfo {
                title,
                process_path,
                process_name,
            })
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

fn privacy_block_reason(settings: &PrivacySettings) -> Option<(String, Option<ActiveWindowInfo>)> {
    let now = now_ms();
    if settings.paused_until.unwrap_or(0) > now {
        return Some(("已暂停监听".to_string(), active_window_info()));
    }
    if !settings.enabled {
        return None;
    }

    let info = active_window_info();
    let Some(info_ref) = info.as_ref() else {
        return None;
    };
    let title_l = info_ref.title.to_lowercase();
    let process_l = info_ref.process_name.to_lowercase();
    let path_l = info_ref.process_path.to_lowercase();

    for app in &settings.ignored_apps {
        let app_l = app.trim().to_lowercase();
        if !app_l.is_empty() && (process_l.contains(&app_l) || path_l.contains(&app_l)) {
            return Some((format!("已忽略应用 {}", info_ref.process_name), info));
        }
    }

    if settings.ignore_incognito {
        let incognito_terms = ["incognito", "inprivate", "private browsing", "无痕", "隐身"];
        if incognito_terms.iter().any(|term| title_l.contains(term)) {
            return Some(("浏览器隐私窗口".to_string(), info));
        }
    }

    for keyword in &settings.sensitive_title_keywords {
        let kw = keyword.trim().to_lowercase();
        if !kw.is_empty() && title_l.contains(&kw) {
            return Some((format!("敏感标题 {}", keyword.trim()), info));
        }
    }

    None
}

/// 解析 "Ctrl+Shift+V" 这种字符串到 Shortcut
fn parse_shortcut(combo: &str) -> Result<Shortcut, String> {
    let mut mods = Modifiers::empty();
    let mut code: Option<Code> = None;
    for part in combo.split('+') {
        let p = part.trim();
        if p.is_empty() { continue; }
        match p.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "shift"            => mods |= Modifiers::SHIFT,
            "alt" | "option"   => mods |= Modifiers::ALT,
            "meta" | "super" | "win" | "cmd" | "command" => mods |= Modifiers::META,
            other => {
                // 单字符按键
                if other.len() == 1 {
                    let c = other.chars().next().unwrap().to_ascii_uppercase();
                    let key = format!("Key{}", c);
                    code = Code::from_str(&key).ok();
                    if code.is_none() {
                        // 数字
                        let digit = format!("Digit{}", c);
                        code = Code::from_str(&digit).ok();
                    }
                } else {
                    // 命名按键 F1, Space, Enter ...
                    let cap: String = other
                        .chars()
                        .enumerate()
                        .map(|(i, ch)| if i == 0 { ch.to_ascii_uppercase() } else { ch.to_ascii_lowercase() })
                        .collect();
                    code = Code::from_str(&cap).ok();
                }
            }
        }
    }
    let c = code.ok_or_else(|| format!("无法解析按键: {}", combo))?;
    // 无强修饰键的裸键注册后会吞掉全系统同名按键;仅功能键 F1-F24 允许单独使用
    let is_fn_key = {
        let name = format!("{:?}", c);
        name.starts_with('F') && name.len() >= 2 && name[1..].chars().all(|ch| ch.is_ascii_digit())
    };
    if !mods.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META) && !is_fn_key {
        return Err("快捷键需包含 Ctrl / Alt / Win 中至少一个修饰键(功能键 F1-F24 可单独使用)".to_string());
    }
    Ok(Shortcut::new(Some(mods), c))
}

fn rebuild_tray_menu(app: &AppHandle) -> tauri::Result<()> {
    let recent = {
        let state = app.state::<AppState>();
        let conn = lock_ok(&state.db);
        query_recent(&conn, 5).unwrap_or_default()
    };

    let show_item = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
    let clear_item = MenuItem::with_id(app, "clear", "清空未收藏历史", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;

    let menu = if recent.is_empty() {
        Menu::with_items(app, &[&show_item, &clear_item, &sep, &quit_item])?
    } else {
        let mut recent_items: Vec<MenuItem<tauri::Wry>> = Vec::new();
        for it in &recent {
            let label = match it.content_type.as_str() {
                "text" => {
                    let t = it.text.clone().unwrap_or_default();
                    let single = t.replace(['\n', '\r', '\t'], " ");
                    let mut s: String = single.chars().take(36).collect();
                    if single.chars().count() > 36 { s.push('…'); }
                    if s.trim().is_empty() { "(空文本)".to_string() } else { s }
                }
                _ => "[图片]".to_string(),
            };
            let id = format!("recent:{}", it.id);
            recent_items.push(MenuItem::with_id(app, &id, &label, true, None::<&str>)?);
        }
        let recent_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
            recent_items.iter().map(|x| x as &dyn tauri::menu::IsMenuItem<tauri::Wry>).collect();
        let recent_sub = Submenu::with_items(app, "最近 5 条", true, &recent_refs)?;

        Menu::with_items(
            app,
            &[
                &show_item,
                &sep,
                &recent_sub,
                &sep,
                &clear_item,
                &quit_item,
            ],
        )?
    };

    if let Some(tray) = app.tray_by_id("main-tray") {
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

// ================== Tauri Commands ==================

#[tauri::command]
fn get_items(state: State<'_, AppState>) -> Result<Vec<ClipItem>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    query_all_items(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
fn pick_item(
    id: i64,
    auto_paste: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let item = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, content_type, text, image_path, pinned, created_at, copy_count, tags, image_width, image_height, image_bytes, sort_order, thumb_path \
                 FROM clips WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        stmt.query_row(params![id], row_to_item)
            .map_err(|e| e.to_string())?
    };

    match item.content_type.as_str() {
        "text" => {
            let text = item.text.clone().unwrap_or_default();
            app.clipboard()
                .write_text(text.clone())
                .map_err(|e| e.to_string())?;
            *state.last_hash.lock().unwrap() = hash_bytes(text.as_bytes());
        }
        "image" => {
            let path = item.image_path.clone().ok_or("image_path missing")?;
            let img = load_image_for_clipboard(&path)?;
            app.clipboard().write_image(&img).map_err(|e| e.to_string())?;
            // 必须与轮询同数据源(解码后 RGBA)计算哈希;此前哈希 PNG 文件字节,
            // 与轮询的 RGBA 哈希永不相等 → 每次选图都被再次入库
            *state.last_hash.lock().unwrap() = hash_bytes(img.rgba());
        }
        _ => return Err("unknown content type".to_string()),
    }

    // 增加使用次数
    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let _ = conn.execute(
            "UPDATE clips SET copy_count = copy_count + 1 WHERE id = ?1",
            params![id],
        );
    }

    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }

    if auto_paste {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(120));
            let _ = simulate_paste();
        });
    }

    Ok(())
}

/// 等待物理修饰键全部松开(带超时)。用户用 Ctrl+Shift+V 唤起后快速回车时,
/// Shift 若仍按着,发出的 Ctrl+V 会变成 Ctrl+Shift+V —— 恰好是自家全局热键,窗口会被弹回来
#[cfg(target_os = "windows")]
fn wait_modifiers_released(timeout_ms: u64) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    let start = std::time::Instant::now();
    loop {
        let pressed = unsafe {
            [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
                .iter()
                .any(|&vk| (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0)
        };
        if !pressed || start.elapsed().as_millis() as u64 >= timeout_ms {
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn simulate_paste() -> Result<(), String> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    #[cfg(target_os = "windows")]
    wait_modifiers_released(1500);
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.key(Key::Control, Direction::Press).map_err(|e| e.to_string())?;
    enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string())?;
    enigo.key(Key::Control, Direction::Release).map_err(|e| e.to_string())?;
    Ok(())
}

/// 把任意文本写入剪贴板（用于「合并选中」「转换并粘贴」等），存入历史并可选自动粘贴
#[tauri::command]
fn paste_text(
    text: String,
    auto_paste: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if text.is_empty() {
        return Err("空文本".to_string());
    }
    app.clipboard()
        .write_text(text.clone())
        .map_err(|e| e.to_string())?;
    // 记录 hash，避免轮询把它当成新内容重复捕获
    *state.last_hash.lock().unwrap() = hash_bytes(text.as_bytes());

    // 合并/转换产生的新内容值得保留、可复用
    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let _ = insert_text(&conn, &text);
        let _ = prune_old(&conn);
    }

    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
    if auto_paste {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(120));
            let _ = simulate_paste();
        });
    }
    Ok(())
}

/// 按给定顺序逐条写入剪贴板并模拟粘贴，每条之间间隔 delay_ms（用于「依次粘贴选中」填表）
#[tauri::command]
fn paste_sequence(
    ids: Vec<i64>,
    delay_ms: u64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // 隐藏窗口前先把内容读好
    let items: Vec<ClipItem> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, content_type, text, image_path, pinned, created_at, copy_count, tags, image_width, image_height, image_bytes, sort_order, thumb_path \
                 FROM clips WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for id in &ids {
            if let Ok(item) = stmt.query_row(params![id], row_to_item) {
                out.push(item);
            }
        }
        out
    };
    if items.is_empty() {
        return Err("没有可粘贴的条目".to_string());
    }

    // 使用次数 +1
    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        for id in &ids {
            let _ = conn.execute(
                "UPDATE clips SET copy_count = copy_count + 1 WHERE id = ?1",
                params![id],
            );
        }
    }

    // 预记录最后一项 hash，减少序列结束后轮询重复入库
    if let Some(last) = items.last() {
        match last.content_type.as_str() {
            "text" => {
                if let Some(t) = &last.text {
                    *state.last_hash.lock().unwrap() = hash_bytes(t.as_bytes());
                }
            }
            "image" => {
                if let Some(p) = &last.image_path {
                    if let Ok(img) = load_image_for_clipboard(p) {
                        *state.last_hash.lock().unwrap() = hash_bytes(img.rgba());
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }

    // 命令运行在独立工作线程，阻塞睡眠不会冻结 UI；先等焦点回到目标应用
    std::thread::sleep(Duration::from_millis(200));
    let delay = delay_ms.clamp(50, 5000);
    for (idx, item) in items.iter().enumerate() {
        match item.content_type.as_str() {
            "text" => {
                if let Some(t) = &item.text {
                    let _ = app.clipboard().write_text(t.clone());
                }
            }
            "image" => {
                if let Some(path) = &item.image_path {
                    if let Ok(img) = load_image_for_clipboard(path) {
                        let _ = app.clipboard().write_image(&img);
                    }
                }
            }
            _ => continue,
        }
        std::thread::sleep(Duration::from_millis(70)); // 等剪贴板写入生效
        let _ = simulate_paste();
        if idx + 1 < items.len() {
            std::thread::sleep(Duration::from_millis(delay));
        }
    }
    Ok(())
}

/// 在系统文件管理器中定位文件（Windows: explorer /select）
#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    if !std::path::Path::new(&path).exists() {
        return Err("路径不存在".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path))
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("当前平台暂不支持".to_string())
    }
}

#[tauri::command]
fn toggle_pin(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    // 取消收藏时清掉 sort_order,避免残留的手动排序值影响后续再收藏时的顺序
    conn.execute(
        "UPDATE clips SET sort_order = CASE WHEN pinned = 1 THEN NULL ELSE sort_order END, \
         pinned = 1 - pinned WHERE id = ?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 增加使用次数(片段替换后调用)
#[tauri::command]
fn increment_copy_count(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE clips SET copy_count = copy_count + 1 WHERE id = ?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 批量更新排序(拖拽后调用,传 id→sort_order 映射)
#[tauri::command]
fn update_sort_order(orders: Vec<(i64, i64)>, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    // 单事务提交:拖一次可能写 200 行,逐条 autocommit 太慢
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for (id, order) in orders {
        tx.execute(
            "UPDATE clips SET sort_order = ?1 WHERE id = ?2",
            params![order, id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_item(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let paths: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT image_path, thumb_path FROM clips WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    conn.execute("DELETE FROM clips WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    drop(conn);
    if let Some((img, thumb)) = paths {
        for p in [img, thumb].into_iter().flatten() {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(())
}

#[tauri::command]
fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT image_path, thumb_path FROM clips WHERE pinned = 0 AND image_path IS NOT NULL")
        .map_err(|e| e.to_string())?;
    let paths: Vec<(String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    conn.execute("DELETE FROM clips WHERE pinned = 0", [])
        .map_err(|e| e.to_string())?;
    drop(conn);
    for (img, thumb) in paths {
        for p in std::iter::once(img).chain(thumb) {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(())
}

#[tauri::command]
fn hide_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn set_window_pin(pin: bool, app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        win.set_always_on_top(pin).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn set_window_mode(mini: bool, app: AppHandle) -> Result<(), String> {
    let Some(win) = app.get_webview_window("main") else {
        return Ok(());
    };

    if mini {
        if win.is_maximized().unwrap_or(false) {
            let _ = win.unmaximize();
        }
        win.set_min_size(Some(Size::Logical(LogicalSize {
            width: 380.0,
            height: 260.0,
        }))).map_err(|e| e.to_string())?;
        win.set_max_size(Some(Size::Logical(LogicalSize {
            width: 560.0,
            height: 460.0,
        }))).map_err(|e| e.to_string())?;
        win.set_size(Size::Logical(LogicalSize {
            width: 430.0,
            height: 330.0,
        })).map_err(|e| e.to_string())?;
    } else {
        win.set_min_size(Some(Size::Logical(LogicalSize {
            width: 900.0,
            height: 600.0,
        }))).map_err(|e| e.to_string())?;
        win.set_max_size(Some(Size::Logical(LogicalSize {
            width: 2200.0,
            height: 1600.0,
        }))).map_err(|e| e.to_string())?;
        win.set_size(Size::Logical(LogicalSize {
            width: 1120.0,
            height: 720.0,
        })).map_err(|e| e.to_string())?;
    }
    let _ = win.set_focus();
    Ok(())
}

#[tauri::command]
fn set_tags(id: i64, tags: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    let clean = normalize_tags(tags);
    let json = serde_json::to_string(&clean).map_err(|e| e.to_string())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("UPDATE clips SET tags = ?1 WHERE id = ?2", params![json, id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_privacy_settings(settings: PrivacySettings, state: State<'_, AppState>) -> Result<PrivacyStatus, String> {
    let mut clean = settings;
    clean.sensitive_title_keywords = normalize_tags(clean.sensitive_title_keywords);
    clean.ignored_apps = normalize_tags(clean.ignored_apps);
    if clean.paused_until.unwrap_or(0) <= now_ms() {
        clean.paused_until = None;
    }
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    write_privacy_settings(&conn, &clean).map_err(|e| e.to_string())?;
    Ok(build_privacy_status(clean))
}

#[tauri::command]
fn pause_clipboard(minutes: i64, state: State<'_, AppState>) -> Result<PrivacyStatus, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut settings = read_privacy_settings(&conn);
    if minutes <= 0 {
        settings.paused_until = None;
    } else {
        settings.paused_until = Some(now_ms() + minutes * 60_000);
    }
    write_privacy_settings(&conn, &settings).map_err(|e| e.to_string())?;
    Ok(build_privacy_status(settings))
}

#[tauri::command]
fn get_privacy_status(state: State<'_, AppState>) -> Result<PrivacyStatus, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    Ok(build_privacy_status(read_privacy_settings(&conn)))
}

fn build_privacy_status(settings: PrivacySettings) -> PrivacyStatus {
    let now = now_ms();
    let paused_remaining_ms = (settings.paused_until.unwrap_or(0) - now).max(0);
    let block = privacy_block_reason(&settings);
    let active = block
        .as_ref()
        .and_then(|(_, info)| info.clone())
        .or_else(active_window_info);
    PrivacyStatus {
        settings,
        paused_remaining_ms,
        active_app: active.as_ref().map(|i| i.process_name.clone()).filter(|s| !s.is_empty()),
        active_title: active.map(|i| i.title).filter(|s| !s.is_empty()),
        blocking_reason: block.map(|(reason, _)| reason),
    }
}

// ---------- 拖拽入库 ----------

#[tauri::command]
fn add_text(text: String, state: State<'_, AppState>) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM clips WHERE content_type='text' AND text = ?1",
            params![&text],
            |r| r.get(0),
        )
        .ok();
    if let Some(id) = existing {
        let _ = conn.execute(
            "UPDATE clips SET created_at = ?1 WHERE id = ?2",
            params![Utc::now().timestamp_millis(), id],
        );
    } else {
        let _ = insert_text(&conn, &text);
        let _ = prune_old(&conn);
    }
    Ok(())
}

/// 手动创建片段(不依赖剪贴板),pinned=true 便于筛选
#[tauri::command]
fn create_snippet(
    text: String,
    tags: Vec<String>,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    if text.trim().is_empty() {
        return Err("片段内容不能为空".to_string());
    }
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let tags_json = serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string());
    let now = Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO clips (content_type, text, pinned, created_at, copy_count, tags) VALUES (?1, ?2, 1, ?3, 0, ?4)",
        params!["text", &text, now, &tags_json],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    let _ = prune_old(&conn);
    Ok(id)
}

#[tauri::command]
fn add_file_path(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err("文件不存在".to_string());
    }
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let is_image = matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tiff" | "ico"
    );
    let is_text = matches!(
        ext.as_str(),
        "txt" | "md" | "log" | "json" | "xml" | "yaml" | "yml" | "toml" | "ini"
            | "csv" | "rs" | "ts" | "js" | "tsx" | "jsx" | "vue" | "py" | "go"
            | "java" | "c" | "cpp" | "h" | "hpp" | "html" | "css" | "scss" | "sh"
    );

    if is_image {
        // 转码为 PNG 存到 images_dir
        let img = image::open(&path).map_err(|e| e.to_string())?;
        let width = img.width();
        let height = img.height();
        let rgba = img.to_rgba8();
        let filename = format!("{}.png", uuid::Uuid::new_v4());
        let dest = state.images_dir.join(&filename);
        rgba.save_with_format(&dest, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        let bytes = std::fs::metadata(&dest).map(|m| m.len() as i64).unwrap_or(0);
        let dest_str = dest.to_string_lossy().to_string();
        let thumb = make_thumbnail(&dest_str);
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let _ = insert_image(&conn, &dest_str, thumb.as_deref(), width, height, bytes);
        let _ = prune_old(&conn);
        Ok(())
    } else if is_text {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        // 限制太长就截断（避免一次性塞入巨型 log）
        let max_len = 64 * 1024;
        let cropped = if content.len() > max_len {
            format!("{}\n... (截断, 共 {} 字节)", &content[..max_len], content.len())
        } else {
            content
        };
        add_text(cropped, state)
    } else {
        // 其它文件：把路径作为文本存
        add_text(format!("[文件] {}", path), state)
    }
}

// ---------- 自定义快捷键 ----------

#[tauri::command]
fn get_hotkey(state: State<'_, AppState>) -> Result<String, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    Ok(read_setting(&conn, "hotkey").unwrap_or_else(|| DEFAULT_HOTKEY.to_string()))
}

#[tauri::command]
fn set_hotkey(combo: String, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let new_shortcut = parse_shortcut(&combo)?;

    // 先注销旧的
    if let Some(old) = state.current_hotkey.lock().unwrap().take() {
        let _ = app.global_shortcut().unregister(old);
    }

    app.global_shortcut()
        .register(new_shortcut.clone())
        .map_err(|e| e.to_string())?;

    *state.current_hotkey.lock().unwrap() = Some(new_shortcut);

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    write_setting(&conn, "hotkey", &combo).map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- 文本工具栏 ----------

#[tauri::command]
fn transform_text(op: String, text: String) -> Result<String, String> {
    let out = match op.as_str() {
        "upper"    => text.to_uppercase(),
        "lower"    => text.to_lowercase(),
        "title"    => text
            .split_whitespace()
            .map(|w| {
                let mut chars = w.chars();
                match chars.next() {
                    None    => String::new(),
                    Some(c) => c.to_uppercase().collect::<String>()
                                + chars.as_str().to_lowercase().as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        "trim"     => text.trim().to_string(),
        "url_enc"  => urlencoding::encode(&text).into_owned(),
        "url_dec"  => urlencoding::decode(&text)
            .map_err(|e| e.to_string())?
            .into_owned(),
        "b64_enc"  => base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
        "b64_dec"  => {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(text.trim())
                .map_err(|e| e.to_string())?;
            String::from_utf8(decoded).map_err(|e| e.to_string())?
        }
        "md5"      => {
            use md5::{Digest, Md5};
            let mut hasher = Md5::new();
            hasher.update(text.as_bytes());
            format!("{:x}", hasher.finalize())
        }

        // ---- JSON ----
        "json_pretty" => {
            let v: serde_json::Value = serde_json::from_str(text.trim())
                .map_err(|e| format!("JSON 解析失败: {}", e))?;
            serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
        }
        "json_min" => {
            let v: serde_json::Value = serde_json::from_str(text.trim())
                .map_err(|e| format!("JSON 解析失败: {}", e))?;
            serde_json::to_string(&v).map_err(|e| e.to_string())?
        }

        // ---- 行操作 ----
        "lines_sort" => {
            let mut lines: Vec<&str> = text.lines().collect();
            lines.sort();
            lines.join("\n")
        }
        "lines_sort_desc" => {
            let mut lines: Vec<&str> = text.lines().collect();
            lines.sort();
            lines.reverse();
            lines.join("\n")
        }
        "lines_dedup" => {
            let mut seen = std::collections::HashSet::new();
            text.lines()
                .filter(|l| seen.insert(l.to_string()))
                .collect::<Vec<_>>()
                .join("\n")
        }
        "lines_reverse" => {
            text.lines().rev().collect::<Vec<_>>().join("\n")
        }
        "lines_strip_empty" => {
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        }

        // ---- 编码 ----
        "hex_enc" => {
            text.bytes().map(|b| format!("{:02x}", b)).collect::<String>()
        }
        "hex_dec" => {
            let s: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            if s.len() % 2 != 0 {
                return Err("十六进制长度必须为偶数".to_string());
            }
            let bytes: Result<Vec<u8>, _> = (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
                .collect();
            let bytes = bytes.map_err(|e| e.to_string())?;
            String::from_utf8(bytes).map_err(|e| format!("非 UTF-8 数据: {}", e))?
        }
        "html_enc" => {
            let mut out = String::with_capacity(text.len());
            for c in text.chars() {
                match c {
                    '&'  => out.push_str("&amp;"),
                    '<'  => out.push_str("&lt;"),
                    '>'  => out.push_str("&gt;"),
                    '"'  => out.push_str("&quot;"),
                    '\'' => out.push_str("&#39;"),
                    _    => out.push(c),
                }
            }
            out
        }
        "html_dec" => {
            // 顺序很重要：先解 &amp; 之外的，最后 &amp; → &
            text.replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&#39;", "'")
                .replace("&nbsp;", " ")
                .replace("&lt;",  "<")
                .replace("&gt;",  ">")
                .replace("&amp;", "&")
        }

        // ---- 统计（特殊：返回多行摘要）----
        "stats" => {
            let chars = text.chars().count();
            let bytes = text.len();
            let lines = text.lines().count();
            let words = text.split_whitespace().count();
            let blanks = text.lines().filter(|l| l.trim().is_empty()).count();
            format!(
                "字符:  {}\n字节:  {}\n词数:  {}\n行数:  {}\n空行:  {}",
                chars, bytes, words, lines, blanks
            )
        }

        other => return Err(format!("未知操作: {}", other)),
    };
    Ok(out)
}

#[tauri::command]
fn replace_text(id: i64, new_text: String, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE clips SET text = ?1, created_at = ?2 WHERE id = ?3 AND content_type='text'",
        params![new_text, Utc::now().timestamp_millis(), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- 导入 / 导出 ----------

#[tauri::command]
fn export_history(path: String, state: State<'_, AppState>) -> Result<i64, String> {
    let items = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        query_all_items(&conn).map_err(|e| e.to_string())?
    };
    let payload = ExportPayload {
        version: 1,
        exported_at: Utc::now().timestamp_millis(),
        items: items.clone(),
    };
    let json = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(items.len() as i64)
}

#[tauri::command]
fn import_history(path: String, state: State<'_, AppState>) -> Result<i64, String> {
    let s = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let payload: ExportPayload = serde_json::from_str(&s).map_err(|e| e.to_string())?;

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut imported = 0i64;
    for it in payload.items {
        match it.content_type.as_str() {
            "text" => {
                if let Some(t) = &it.text {
                    let exists: Option<i64> = tx
                        .query_row(
                            "SELECT id FROM clips WHERE content_type='text' AND text = ?1",
                            params![t],
                            |r| r.get(0),
                        )
                        .ok();
                    if exists.is_none() {
                        tx.execute(
                            "INSERT INTO clips (content_type, text, image_path, pinned, created_at, copy_count, tags, image_width, image_height, image_bytes) \
                             VALUES ('text', ?1, NULL, ?2, ?3, ?4, ?5, NULL, NULL, NULL)",
                            params![
                                t,
                                if it.pinned { 1 } else { 0 },
                                it.created_at,
                                it.copy_count,
                                serde_json::to_string(&normalize_tags(it.tags.clone())).unwrap_or_else(|_| "[]".to_string())
                            ],
                        ).map_err(|e| e.to_string())?;
                        imported += 1;
                    }
                }
            }
            "image" => {
                // 图片只导入仍存在于磁盘的路径
                if let Some(p) = &it.image_path {
                    if std::path::Path::new(p).exists() {
                        tx.execute(
                            "INSERT INTO clips (content_type, text, image_path, pinned, created_at, copy_count, tags, image_width, image_height, image_bytes) \
                             VALUES ('image', NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                            params![
                                p,
                                if it.pinned { 1 } else { 0 },
                                it.created_at,
                                it.copy_count,
                                serde_json::to_string(&normalize_tags(it.tags.clone())).unwrap_or_else(|_| "[]".to_string()),
                                it.image_width.map(|v| v as i64),
                                it.image_height.map(|v| v as i64),
                                it.image_bytes
                            ],
                        ).map_err(|e| e.to_string())?;
                        imported += 1;
                    }
                }
            }
            _ => {}
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(imported)
}

// ---------- 批量操作 ----------

#[tauri::command]
fn batch_delete(ids: Vec<i64>, state: State<'_, AppState>) -> Result<(), String> {
    if ids.is_empty() { return Ok(()); }
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut paths_to_remove: Vec<String> = vec![];
    for id in &ids {
        if let Ok((img, thumb)) = tx.query_row(
            "SELECT image_path, thumb_path FROM clips WHERE id = ?1",
            params![id],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
        ) {
            paths_to_remove.extend([img, thumb].into_iter().flatten());
        }
        let _ = tx.execute("DELETE FROM clips WHERE id = ?1", params![id]);
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(conn);
    for p in paths_to_remove {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

#[tauri::command]
fn batch_pin(ids: Vec<i64>, pinned: bool, state: State<'_, AppState>) -> Result<(), String> {
    if ids.is_empty() { return Ok(()); }
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for id in ids {
        let _ = tx.execute(
            "UPDATE clips SET pinned = ?1, \
             sort_order = CASE WHEN ?1 = 0 THEN NULL ELSE sort_order END WHERE id = ?2",
            params![if pinned { 1 } else { 0 }, id],
        );
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- 使用统计 ----------

#[tauri::command]
fn get_stats(state: State<'_, AppState>) -> Result<Stats, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM clips", [], |r| r.get(0))
        .unwrap_or(0);
    let text_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM clips WHERE content_type='text'", [], |r| r.get(0))
        .unwrap_or(0);
    let image_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM clips WHERE content_type='image'", [], |r| r.get(0))
        .unwrap_or(0);
    let pinned_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM clips WHERE pinned=1", [], |r| r.get(0))
        .unwrap_or(0);
    let total_copies: i64 = conn
        .query_row("SELECT COALESCE(SUM(copy_count),0) FROM clips", [], |r| r.get(0))
        .unwrap_or(0);

    let mut stmt = conn.prepare(
        "SELECT id, content_type, text, image_path, pinned, created_at, copy_count, tags, image_width, image_height, image_bytes, sort_order, thumb_path \
         FROM clips WHERE copy_count > 0 ORDER BY copy_count DESC LIMIT 5",
    ).map_err(|e| e.to_string())?;
    let top_items: Vec<ClipItem> = stmt
        .query_map([], row_to_item)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Stats {
        total, text_count, image_count, pinned_count, total_copies, top_items,
    })
}

// ================== 入口 ==================

// ===================== OCR 文字识别 =====================
#[cfg(target_os = "windows")]
fn recognize_text_from_image_path(image_path: &str) -> Result<String, String> {
    use windows::{
        Media::Ocr::OcrEngine,
        Graphics::Imaging::BitmapDecoder,
        Storage::{StorageFile, FileAccessMode},
        Globalization::Language,
    };

    let path_hstring = windows::core::HSTRING::from(image_path);
    let file = StorageFile::GetFileFromPathAsync(&path_hstring)
        .map_err(|e| format!("无法打开图片: {}", e))?
        .get()
        .map_err(|e| format!("文件访问失败: {}", e))?;

    let stream = file.OpenAsync(FileAccessMode::Read)
        .map_err(|e| format!("打开流失败: {}", e))?
        .get()
        .map_err(|e| format!("获取流失败: {}", e))?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|e| format!("解码失败: {}", e))?
        .get()
        .map_err(|e| format!("创建解码器失败: {}", e))?;

    let bitmap = decoder.GetSoftwareBitmapAsync()
        .map_err(|e| format!("获取位图失败: {}", e))?
        .get()
        .map_err(|e| format!("位图读取失败: {}", e))?;

    // 使用中文 OCR 引擎(zh-Hans),如果失败回退到英文
    let engine = Language::CreateLanguage(&windows::core::HSTRING::from("zh-Hans"))
        .ok()
        .and_then(|lang| OcrEngine::TryCreateFromLanguage(&lang).ok())
        .or_else(|| OcrEngine::TryCreateFromUserProfileLanguages().ok())
        .ok_or_else(|| "OCR 引擎初始化失败(需要 Windows 10 1903+)".to_string())?;

    let result = engine.RecognizeAsync(&bitmap)
        .map_err(|e| format!("OCR 识别失败: {}", e))?
        .get()
        .map_err(|e| format!("获取识别结果失败: {}", e))?;

    let text = result.Text()
        .map_err(|e| format!("提取文本失败: {}", e))?
        .to_string();

    if text.trim().is_empty() {
        return Err("未识别到文字".to_string());
    }

    Ok(text)
}

#[cfg(target_os = "windows")]
fn prepare_ocr_image(image_path: &str, crop: Option<OcrCrop>) -> Result<PathBuf, String> {
    use image::{ImageBuffer, Luma};

    let mut img = image::open(image_path).map_err(|e| format!("图片读取失败: {}", e))?;
    let img_w = img.width();
    let img_h = img.height();
    if img_w == 0 || img_h == 0 {
        return Err("图片尺寸无效".to_string());
    }

    if let Some(crop) = crop {
        let x = crop.x.min(img_w.saturating_sub(1));
        let y = crop.y.min(img_h.saturating_sub(1));
        let max_w = img_w.saturating_sub(x);
        let max_h = img_h.saturating_sub(y);
        let w = crop.width.min(max_w);
        let h = crop.height.min(max_h);
        if w < 2 || h < 2 {
            return Err("选区太小，无法识别".to_string());
        }
        img = img.crop_imm(x, y, w, h);
    }

    let mut gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    let mut min_v = u8::MAX;
    let mut max_v = u8::MIN;
    let mut sum: u64 = 0;
    for p in gray.pixels() {
        let v = p.0[0];
        min_v = min_v.min(v);
        max_v = max_v.max(v);
        sum += v as u64;
    }

    let total = (w as u64).saturating_mul(h as u64).max(1);
    let avg = (sum / total) as u8;
    let range = max_v.saturating_sub(min_v);
    for p in gray.pixels_mut() {
        let mut v = p.0[0];
        if range > 16 {
            v = (((v.saturating_sub(min_v)) as u32 * 255) / range as u32) as u8;
        }
        if avg < 128 {
            v = 255u8.saturating_sub(v);
        }
        p.0[0] = v;
    }

    let max_dim = w.max(h).max(1);
    let mut target_max = if max_dim < 700 {
        max_dim.saturating_mul(4)
    } else if max_dim < 1200 {
        max_dim.saturating_mul(3)
    } else if max_dim < 1800 {
        max_dim.saturating_mul(2)
    } else {
        max_dim
    };
    target_max = target_max.min(3200);
    let scale = (target_max as f32 / max_dim as f32).max(1.0);
    let prepared = if scale > 1.05 {
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        image::imageops::resize(&gray, nw, nh, image::imageops::FilterType::CatmullRom)
    } else {
        gray
    };

    let (pw, ph) = prepared.dimensions();
    let border = 24u32;
    let mut canvas: ImageBuffer<Luma<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(pw + border * 2, ph + border * 2, Luma([255]));
    image::imageops::overlay(&mut canvas, &prepared, border as i64, border as i64);

    let path = std::env::temp_dir().join(format!("clipboard-helper-ocr-{}.png", uuid::Uuid::new_v4()));
    canvas
        .save_with_format(&path, image::ImageFormat::Png)
        .map_err(|e| format!("OCR 临时图片保存失败: {}", e))?;
    Ok(path)
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn extract_text_from_image(image_path: String, crop: Option<OcrCrop>) -> Result<String, String> {
    // WinRT 识别与图片预处理全是同步阻塞调用,放进阻塞线程池,避免占死 async worker
    tauri::async_runtime::spawn_blocking(move || {
        let prepared_path = prepare_ocr_image(&image_path, crop)?;
        let result = recognize_text_from_image_path(&prepared_path.to_string_lossy());
        let _ = std::fs::remove_file(prepared_path);
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(not(target_os = "windows"))]
#[tauri::command]
async fn extract_text_from_image(_image_path: String, _crop: Option<OcrCrop>) -> Result<String, String> {
    Err("OCR 仅支持 Windows 10+".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");
            std::fs::create_dir_all(&app_data_dir).ok();
            let images_dir = app_data_dir.join("images");
            std::fs::create_dir_all(&images_dir).ok();
            let db_path = app_data_dir.join("clips.db");

            let conn = init_db(&db_path).expect("failed to init db");

            // 清理旧版本 prune 泄漏的孤儿图片
            cleanup_orphan_images(&conn, &images_dir);

            // 读出保存过的快捷键
            let saved_hotkey = read_setting(&conn, "hotkey").unwrap_or_else(|| DEFAULT_HOTKEY.to_string());

            // Mica/毛玻璃永久关闭:用户要求纯实色无模糊
            let state = AppState {
                db: Mutex::new(conn),
                last_hash: Mutex::new(0),
                images_dir: images_dir.clone(),
                current_hotkey: Mutex::new(None),
            };
            app.manage(state);

            let app_handle = app.handle().clone();

            // ---------- 全局快捷键 ----------
            let initial_shortcut = parse_shortcut(&saved_hotkey)
                .unwrap_or_else(|_| Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyV));

            let stored_shortcut = initial_shortcut.clone();
            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |app, shortcut, event| {
                        if event.state() == ShortcutState::Pressed {
                            let state = app.state::<AppState>();
                            let current = state.current_hotkey.lock().unwrap().clone();
                            if let Some(c) = current {
                                if shortcut == &c {
                                    toggle_window(app);
                                }
                            } else if shortcut == &stored_shortcut {
                                toggle_window(app);
                            }
                        }
                    })
                    .build(),
            )?;
            // 快捷键可能被其它程序抢注:注册失败只提示,不能让应用启动即崩溃
            match app.global_shortcut().register(initial_shortcut.clone()) {
                Ok(()) => {
                    let st = app.state::<AppState>();
                    *st.current_hotkey.lock().unwrap() = Some(initial_shortcut);
                }
                Err(e) => eprintln!("注册全局快捷键失败(可能已被其它程序占用): {e}"),
            }

            // ---------- 系统托盘 ----------
            let show_item = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let clear_item = MenuItem::with_id(app, "clear", "清空未收藏历史", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&show_item, &sep, &clear_item, &quit_item])?;
            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("剪贴板助手")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    let id = event.id.as_ref();
                    if let Some(rest) = id.strip_prefix("recent:") {
                        if let Ok(clip_id) = rest.parse::<i64>() {
                            let app2 = app.clone();
                            std::thread::spawn(move || {
                                let st = app2.state::<AppState>();
                                let _ = pick_item(clip_id, true, app2.clone(), st);
                            });
                        }
                        return;
                    }
                    match id {
                        "show" => {
                            if let Some(win) = app.get_webview_window("main") {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                        "clear" => {
                            // 破坏性操作:先弹原生确认框(在独立线程,blocking_show 不能占主线程)
                            let app = app.clone();
                            std::thread::spawn(move || {
                                use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
                                let confirmed = app
                                    .dialog()
                                    .message("确定清空所有未收藏的历史吗?\n对应图片文件会一并删除,不可恢复。")
                                    .title("清空未收藏历史")
                                    .kind(MessageDialogKind::Warning)
                                    .buttons(MessageDialogButtons::OkCancelCustom("清空".to_string(), "取消".to_string()))
                                    .blocking_show();
                                if confirmed {
                                    let state = app.state::<AppState>();
                                    let _ = clear_history(state);
                                    let _ = app.emit("clips-changed", ());
                                    let _ = rebuild_tray_menu(&app);
                                }
                            });
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // ---------- 窗口关闭→隐藏 ----------
            if let Some(win) = app.get_webview_window("main") {
                let win_clone = win.clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = win_clone.hide();
                    }
                });
            }

            // ---------- Rust 端兜底拖拽事件转发（前端 listen 直接收到原生事件） ----------
            // 大部分情况 tauri v2 已经自动 emit `tauri://drag-drop` 等事件到 webview，
            // 这里再保险一层：手动注册并 emit 一个我们自己的事件名。
            if let Some(win) = app.get_webview_window("main") {
                let win_for_event = win.clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::DragDrop(drag) = event {
                        match drag {
                            tauri::DragDropEvent::Enter { paths, .. } => {
                                let ps: Vec<String> = paths.iter().map(|p| p.to_string_lossy().to_string()).collect();
                                let _ = win_for_event.emit("app-drag-enter", ps);
                            }
                            tauri::DragDropEvent::Over { .. } => {
                                let _ = win_for_event.emit("app-drag-over", ());
                            }
                            tauri::DragDropEvent::Drop { paths, .. } => {
                                let ps: Vec<String> = paths.iter().map(|p| p.to_string_lossy().to_string()).collect();
                                let _ = win_for_event.emit("app-drag-drop", ps);
                            }
                            tauri::DragDropEvent::Leave => {
                                let _ = win_for_event.emit("app-drag-leave", ());
                            }
                            _ => {}
                        }
                    }
                });
            }

            // ---------- 监听 clips-changed 刷新托盘菜单 ----------
            let tray_handle = app_handle.clone();
            app.listen("clips-changed", move |_| {
                let _ = rebuild_tray_menu(&tray_handle);
            });

            // ---------- 旧图片后台补生成缩略图(一次性) ----------
            let backfill_handle = app_handle.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let state = backfill_handle.state::<AppState>();
                let todo: Vec<(i64, String)> = {
                    let conn = lock_ok(&state.db);
                    let mut stmt = match conn.prepare(
                        "SELECT id, image_path FROM clips \
                         WHERE content_type = 'image' AND image_path IS NOT NULL AND thumb_path IS NULL",
                    ) {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    let rows: Vec<(i64, String)> =
                        match stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))) {
                            Ok(rows) => rows.flatten().collect(),
                            Err(_) => return,
                        };
                    rows
                };
                let mut changed = false;
                for (id, path) in todo {
                    // 编码在锁外进行,只有写回时短暂持锁
                    if let Some(tp) = make_thumbnail(&path) {
                        let conn = lock_ok(&state.db);
                        let _ = conn.execute(
                            "UPDATE clips SET thumb_path = ?1 WHERE id = ?2",
                            params![tp, id],
                        );
                        changed = true;
                    }
                }
                if changed {
                    let _ = backfill_handle.emit("clips-changed", ());
                }
            });

            // ---------- 后台轮询剪贴板 ----------
            let poll_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                // Windows 剪贴板序列号:内容未变时跳过整个 tick,避免空转读图/查库
                #[cfg(target_os = "windows")]
                let mut last_seq: u32 = 0;
                loop {
                    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;

                    #[cfg(target_os = "windows")]
                    {
                        let seq = unsafe {
                            windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
                        };
                        // seq==0 表示查询失败,此时退回逐 tick 检查
                        if seq != 0 && seq == last_seq {
                            continue;
                        }
                        last_seq = seq;
                    }

                    let clipboard = poll_handle.clipboard();
                    let state = poll_handle.state::<AppState>();

                    let privacy = {
                        let conn = lock_ok(&state.db);
                        read_privacy_settings(&conn)
                    };
                    if privacy_block_reason(&privacy).is_some() {
                        if let Ok(text) = clipboard.read_text() {
                            if !text.is_empty() {
                                *lock_ok(&state.last_hash) = hash_bytes(text.as_bytes());
                                continue;
                            }
                        }
                        if let Ok(img) = clipboard.read_image() {
                            *lock_ok(&state.last_hash) = hash_bytes(img.rgba());
                        }
                        continue;
                    }

                    if let Ok(text) = clipboard.read_text() {
                        if !text.is_empty() {
                            let h = hash_bytes(text.as_bytes());
                            if *lock_ok(&state.last_hash) != h {
                                // 先落库、成功后才更新 last_hash:写入失败的内容不会被无声吞掉
                                let ok = {
                                    let conn = lock_ok(&state.db);
                                    let existing: Option<i64> = conn
                                        .query_row(
                                            "SELECT id FROM clips WHERE content_type='text' AND text = ?1",
                                            params![text],
                                            |r| r.get(0),
                                        )
                                        .ok();
                                    if let Some(id) = existing {
                                        conn.execute(
                                            "UPDATE clips SET created_at = ?1 WHERE id = ?2",
                                            params![Utc::now().timestamp_millis(), id],
                                        )
                                        .is_ok()
                                    } else {
                                        let inserted = insert_text(&conn, &text).is_ok();
                                        let _ = prune_old(&conn);
                                        inserted
                                    }
                                };
                                if ok {
                                    *lock_ok(&state.last_hash) = h;
                                    let _ = poll_handle.emit("clips-changed", ());
                                }
                            }
                            continue;
                        }
                    }

                    if let Ok(img) = clipboard.read_image() {
                        let h = hash_bytes(img.rgba());
                        if *lock_ok(&state.last_hash) != h {
                            match save_clipboard_image(&img, &state.images_dir) {
                                Ok((path, thumb, width, height, bytes)) => {
                                    let inserted = {
                                        let conn = lock_ok(&state.db);
                                        let r = insert_image(&conn, &path, thumb.as_deref(), width, height, bytes).is_ok();
                                        let _ = prune_old(&conn);
                                        r
                                    };
                                    if inserted {
                                        *lock_ok(&state.last_hash) = h;
                                        let _ = poll_handle.emit("clips-changed", ());
                                    }
                                }
                                Err(e) => eprintln!("save image failed: {}", e),
                            }
                        }
                    }
                }
            });

            // 启动时也刷新一次托盘
            let _ = rebuild_tray_menu(&app_handle);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_items,
            pick_item,
            paste_text,
            paste_sequence,
            reveal_path,
            toggle_pin,
            increment_copy_count,
            update_sort_order,
            delete_item,
            clear_history,
            hide_window,
            set_window_pin,
            set_window_mode,
            set_tags,
            get_privacy_status,
            set_privacy_settings,
            pause_clipboard,
            add_text,
            create_snippet,
            add_file_path,
            get_hotkey,
            set_hotkey,
            transform_text,
            replace_text,
            extract_text_from_image,
            export_history,
            import_history,
            batch_delete,
            batch_pin,
            get_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
