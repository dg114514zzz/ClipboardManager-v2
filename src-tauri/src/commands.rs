use crate::config::{save_config, AppConfig};
use crate::db;
use crate::db::Database;
use crate::models::ClipboardItem;
use crate::AppState;
use base64::Engine;
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, State};

// ---------- Win32 FFI（写 CF_HDROP 用） ----------
extern "system" {
    fn OpenClipboard(hWndNewOwner: isize) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(uFormat: u32, hMem: isize) -> isize;
    fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> isize;
    fn GlobalFree(hMem: isize) -> isize;
    fn GlobalLock(hMem: isize) -> *mut u8;
    fn GlobalUnlock(hMem: isize) -> i32;
}

// ---------- Win32 FFI（Shell 多选定位用） ----------
#[link(name = "shell32")]
extern "system" {
    fn SHParseDisplayName(
        pszName: *const u16,
        pbc: isize,
        ppidl: *mut isize,
        sfgaoIn: u32,
        psfgaoOut: *mut u32,
    ) -> i32;
    fn SHOpenFolderAndSelectItems(pidlFolder: isize, cidl: u32, apidl: *const isize, dwFlags: u32) -> i32;
    fn ILFindChild(pidlFolder: isize, pidlItem: isize) -> isize;
}
#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(pv: isize);
}

const CF_HDROP: u32 = 15;
const CF_DIB: u32 = 8;
// GHND = GMEM_MOVEABLE(0x2) | GMEM_ZEROINIT(0x40)
const GMEM_MOVEABLE: u32 = 0x0002;
const GMEM_ZEROINIT: u32 = 0x0040;

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppConfig, String> {
    let cfg = state.config.lock().map_err(|e| e.to_string())?;
    Ok(cfg.clone())
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppConfig,
) -> Result<(), String> {
    // 1) 持久化到固定位置的 config.json
    let config_path = state.config_path.clone();
    {
        let mut cfg = state.config.lock().map_err(|e| e.to_string())?;
        *cfg = settings.clone();
        save_config(&config_path, &cfg)?;
        log::info!(
            "设置已保存: retention_days={}, auto_start={}",
            settings.retention_days,
            settings.auto_start
        );
    }

    // 2) 开机自启写 HKCU\...\Run（tauri-plugin-autostart）
    //    先比对系统实际状态，一致则跳过：disable() 实际是"删除注册表项"，删一个本就不存在的项
    //    会返回 os error 2（ERROR_FILE_NOT_FOUND），导致"上次已关闭、本次只改了别的设置"也误报失败
    use tauri_plugin_autostart::ManagerExt;
    let auto = app.autolaunch();
    // 系统当前实际状态；查询失败时按"与目标不一致"处理，退化为照常执行（失败原因记日志，不静默吞掉）
    let actual = match auto.is_enabled() {
        Ok(v) => v,
        Err(e) => {
            log::warn!("读取开机自启状态失败: {e}，改为直接执行变更");
            !settings.auto_start
        }
    };
    if actual == settings.auto_start {
        log::info!("开机自启无需变更（系统当前状态已为 {}）", settings.auto_start);
        return Ok(());
    }
    if settings.auto_start {
        auto.enable().map_err(|e| format!("开机自启启用失败: {e}"))?;
        log::info!("开机自启已启用");
    } else {
        auto.disable().map_err(|e| format!("开机自启禁用失败: {e}"))?;
        log::info!("开机自启已禁用");
    }
    Ok(())
}

#[tauri::command]
pub fn get_items(
    db: State<'_, Arc<Database>>,
    favorite_only: bool,
    sort_asc: bool,
) -> Result<Vec<ClipboardItem>, String> {
    db.get_items(favorite_only, sort_asc)
}

#[tauri::command]
pub fn search_items(db: State<'_, Arc<Database>>, query: String) -> Result<Vec<ClipboardItem>, String> {
    db.search_items(&query)
}

#[tauri::command]
pub fn toggle_favorite(db: State<'_, Arc<Database>>, id: i64) -> Result<bool, String> {
    db.toggle_favorite(id)
}

#[tauri::command]
pub fn toggle_pin(db: State<'_, Arc<Database>>, id: i64) -> Result<bool, String> {
    db.toggle_pin(id)
}

#[tauri::command]
pub fn delete_item(db: State<'_, Arc<Database>>, id: i64) -> Result<(), String> {
    db.delete_item(id)?;
    log::info!("删除记录 id={id}");
    Ok(())
}

/// 前端失效检测：批量检查文件是否存在
#[tauri::command]
pub fn check_files_exist(paths: Vec<String>) -> Result<Vec<bool>, String> {
    Ok(paths.iter().map(|p| Path::new(p).exists()).collect())
}

/// 定位文件：打开所在文件夹并选中全部文件（跨文件夹时只选第一个）
#[tauri::command]
pub fn reveal_file(paths: Vec<String>) -> Result<(), String> {
    let existing: Vec<String> = paths
        .iter()
        .filter(|p| Path::new(p).exists())
        .cloned()
        .collect();
    if existing.is_empty() {
        return Err("源文件不存在".into());
    }
    // 全部同文件夹 → 全选；跨文件夹 → 只选第一个
    let same_dir = existing
        .iter()
        .all(|p| norm_dir(Path::new(p)) == norm_dir(Path::new(&existing[0])));
    let to_select: Vec<&String> = if same_dir {
        existing.iter().collect()
    } else {
        vec![&existing[0]]
    };
    reveal_in_explorer(&to_select)?;
    log::info!("定位文件 {} 个", to_select.len());
    Ok(())
}

/// 规范化所在文件夹路径（统一小写、去尾部斜杠），用于判断是否同一文件夹
fn norm_dir(p: &Path) -> Option<String> {
    p.parent().map(|d| {
        d.to_string_lossy()
            .to_lowercase()
            .trim_end_matches(['\\', '/'])
            .to_string()
    })
}

/// 用 Shell API 打开文件夹并同时选中多个文件（SHOpenFolderAndSelectItems）
fn reveal_in_explorer(files: &[&String]) -> Result<(), String> {
    if files.is_empty() {
        return Err("无有效文件".into());
    }
    let folder = Path::new(files[0])
        .parent()
        .ok_or_else(|| "无法确定所在文件夹".to_string())?;
    unsafe {
        // 文件夹 PIDL
        let mut folder_pidl: isize = 0;
        let folder_w: Vec<u16> = folder
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        if SHParseDisplayName(folder_w.as_ptr(), 0, &mut folder_pidl, 0, std::ptr::null_mut()) != 0
            || folder_pidl == 0
        {
            return Err("解析文件夹失败".into());
        }
        // 各文件 PIDL + 相对文件夹的子 PIDL（ILFindChild 返回的 child 指向 item PIDL 内部，无需单独释放）
        let mut item_pidls: Vec<isize> = Vec::new();
        let mut child_pidls: Vec<isize> = Vec::new();
        for f in files {
            let mut pidl: isize = 0;
            let w: Vec<u16> = f.encode_utf16().chain(std::iter::once(0)).collect();
            if SHParseDisplayName(w.as_ptr(), 0, &mut pidl, 0, std::ptr::null_mut()) == 0 && pidl != 0
            {
                let child = ILFindChild(folder_pidl, pidl);
                if child != 0 {
                    child_pidls.push(child);
                }
                item_pidls.push(pidl);
            }
        }
        if child_pidls.is_empty() {
            for p in &item_pidls {
                CoTaskMemFree(*p);
            }
            CoTaskMemFree(folder_pidl);
            return Err("无法解析文件路径".into());
        }
        SHOpenFolderAndSelectItems(folder_pidl, child_pidls.len() as u32, child_pidls.as_ptr(), 0);
        for p in &item_pidls {
            CoTaskMemFree(*p);
        }
        CoTaskMemFree(folder_pidl);
    }
    Ok(())
}

/// 读取缩略图 → base64 data URI（供前端列表显示；禁止走整张原图，文档 4.2）
#[tauri::command]
pub fn get_thumbnail_base64(db: State<'_, Arc<Database>>, path: String) -> Result<String, String> {
    // 路径校验：必须位于 thumbs 目录内（大小写不敏感前缀），防止任意文件读取
    let thumbs = db.thumbs_dir.to_string_lossy().to_lowercase();
    let p = path.to_lowercase();
    if !p.starts_with(&thumbs) {
        log::warn!("非法缩略图路径: {path}");
        return Err("invalid path".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("读取缩略图失败: {e}"))?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// 读取原图 → base64 data URI（供大图查看器使用）。与缩略图命令分开：查看器需要完整分辨率。
/// 路径校验限死在 images 目录内（大小写不敏感前缀），防止任意文件读取
#[tauri::command]
pub fn get_image_base64(db: State<'_, Arc<Database>>, path: String) -> Result<String, String> {
    let images = db.images_dir.to_string_lossy().to_lowercase();
    let p = path.to_lowercase();
    if !p.starts_with(&images) {
        log::warn!("非法图片路径: {path}");
        return Err("invalid path".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("读取图片失败: {e}"))?;
    log::info!("加载原图查看: {path}（{} 字节）", bytes.len());
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// 点击记录 → 放回剪贴板（文本写文本；图片写位图；文件写 CF_HDROP）
/// 写剪贴板会触发 WM_CLIPBOARDUPDATE，库内去重保证不产生重复记录。
/// 返回 Ok(提示消息)，空串表示无额外提示；Err 为复制失败。
#[tauri::command]
pub fn copy_item(
    db: State<'_, Arc<Database>>,
    state: State<'_, AppState>,
    id: i64,
) -> Result<String, String> {
    let item = db.get_item(id)?.ok_or_else(|| format!("记录不存在 id={id}"))?;
    // 标记本次为"程序内部复制"（写剪贴板前设置，供 worker 区分，避免内部复制触发拉顶刷新）
    {
        let mut guard = state.internal_copy.lock().map_err(|e| e.to_string())?;
        *guard = vec![id];
    }
    match item.item_type.as_str() {
        "text" => {
            let text = item.content.ok_or("文本记录缺少 content")?;
            let mut cb = arboard::Clipboard::new().map_err(|e| format!("打开剪贴板失败: {e}"))?;
            cb.set_text(text.clone()).map_err(|e| format!("写入文本失败: {e}"))?;
            log::info!("已复制文本记录 id={id}");
            Ok(String::new())
        }
        "image" => {
            let path = item.image_path.ok_or("图片记录缺少 image_path")?;
            // 读取本地 PNG（程序存储的图片）
            let rgba = image::open(&path)
                .map_err(|e| format!("读取图片失败 {path}: {e}"))?
                .to_rgba8();
            let (w, h) = (rgba.width() as usize, rgba.height() as usize);
            // 同时写位图（CF_DIB，聊天框粘贴图片）+ 文件路径（CF_HDROP，文件管理器粘贴出图片文件）
            write_image_clipboard(rgba.as_raw(), w, h, &[path.clone()])
                .map_err(|e| format!("写入剪贴板失败: {e}"))?;
            log::info!("已复制图片记录 id={id}（位图 + 文件路径）");
            Ok(String::new())
        }
        "file" => {
            let paths = item.file_paths.ok_or("文件记录缺少 file_paths")?;
            // 检查存在性：全部失效则拒绝；部分失效则复制存在的并提示缺失
            let existing: Vec<String> = paths
                .iter()
                .filter(|p| Path::new(p.as_str()).exists())
                .cloned()
                .collect();
            let missing = paths.len() - existing.len();
            if existing.is_empty() {
                return Err("源文件不存在".into());
            }
            write_drop_files(&existing)?;
            log::info!("已复制文件记录 id={id} ({} 个文件)", paths.len());
            if missing > 0 {
                Ok(format!("{missing} 个文件已不存在，已复制剩余文件"))
            } else {
                Ok(String::new())
            }
        }
        other => Err(format!("不支持的记录类型: {other}")),
    }
}

/// 把文件路径列表写入剪贴板（CF_HDROP，UTF-16LE）
fn write_drop_files(paths: &[String]) -> Result<(), String> {
    let mut data: Vec<u8> = Vec::with_capacity(
        20 + paths.iter().map(|p| p.len() * 2 + 2).sum::<usize>() + 2,
    );
    // DROPFILES 头（20 字节）：pFiles=20, pt=(0,0), fNC=0, fWide=1
    data.extend_from_slice(&20u32.to_le_bytes());
    data.extend_from_slice(&0i32.to_le_bytes());
    data.extend_from_slice(&0i32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    for p in paths {
        for u in p.encode_utf16() {
            data.extend_from_slice(&u.to_le_bytes());
        }
        data.extend_from_slice(&0u16.to_le_bytes());
    }
    data.extend_from_slice(&0u16.to_le_bytes()); // 双 null 结尾

    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, data.len());
        if hmem == 0 {
            return Err("分配剪贴板内存失败".into());
        }
        let ptr = GlobalLock(hmem);
        if ptr.is_null() {
            GlobalFree(hmem);
            return Err("锁定剪贴板内存失败".into());
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        GlobalUnlock(hmem);

        // 打开剪贴板（与其他写剪贴板进程瞬时竞争，重试）
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(0) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        if !opened {
            GlobalFree(hmem);
            return Err("打开剪贴板失败".into());
        }
        EmptyClipboard();
        let ok = SetClipboardData(CF_HDROP, hmem);
        CloseClipboard();
        if ok == 0 {
            return Err("写入剪贴板文件列表失败".into());
        }
        // 成功：hmem 所有权移交系统，不 GlobalFree
        Ok(())
    }
}

/// 把一个位图（CF_DIB）+ 文件路径列表（CF_HDROP）同时写入剪贴板（同一 Empty 周期内）。
/// 聊天框可粘贴图片；文件管理器右键可粘贴出图片文件（文件名 = 传入路径的文件名）。
fn write_image_clipboard(rgba: &[u8], w: usize, h: usize, paths: &[String]) -> Result<(), String> {
    // 1) 构造 CF_DIB（BITMAPINFOHEADER 40 字节 + BGRA 像素，自底向上）
    let mut dib: Vec<u8> = Vec::with_capacity(40 + w * h * 4);
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(w as u32).to_le_bytes()); // biWidth
    dib.extend_from_slice(&(h as u32).to_le_bytes()); // biHeight（正 = 自底向上）
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes()); // biSizeImage
    dib.extend_from_slice(&[0u8; 16]); // 其余字段
    for row in (0..h).rev() {
        for col in 0..w {
            let i = (row * w + col) * 4;
            dib.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }

    // 2) 构造 CF_HDROP（DROPFILES 头 + UTF-16LE 路径）
    let mut drop: Vec<u8> =
        Vec::with_capacity(20 + paths.iter().map(|p| p.len() * 2 + 2).sum::<usize>() + 2);
    drop.extend_from_slice(&20u32.to_le_bytes()); // pFiles
    drop.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    drop.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    drop.extend_from_slice(&0u32.to_le_bytes()); // fNC
    drop.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for p in paths {
        for u in p.encode_utf16() {
            drop.extend_from_slice(&u.to_le_bytes());
        }
        drop.extend_from_slice(&0u16.to_le_bytes());
    }
    drop.extend_from_slice(&0u16.to_le_bytes()); // 双 null 结尾

    unsafe {
        let hmem_dib = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, dib.len());
        if hmem_dib == 0 {
            return Err("分配位图内存失败".into());
        }
        let ptr = GlobalLock(hmem_dib);
        if ptr.is_null() {
            GlobalFree(hmem_dib);
            return Err("锁定位图内存失败".into());
        }
        std::ptr::copy_nonoverlapping(dib.as_ptr(), ptr, dib.len());
        GlobalUnlock(hmem_dib);

        let hmem_drop = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, drop.len());
        if hmem_drop == 0 {
            GlobalFree(hmem_dib);
            return Err("分配文件列表内存失败".into());
        }
        let ptr2 = GlobalLock(hmem_drop);
        if ptr2.is_null() {
            GlobalFree(hmem_drop);
            GlobalFree(hmem_dib);
            return Err("锁定文件列表内存失败".into());
        }
        std::ptr::copy_nonoverlapping(drop.as_ptr(), ptr2, drop.len());
        GlobalUnlock(hmem_drop);

        // 打开剪贴板（与其他写剪贴板进程瞬时竞争，重试）
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(0) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        if !opened {
            GlobalFree(hmem_drop);
            GlobalFree(hmem_dib);
            return Err("打开剪贴板失败".into());
        }
        EmptyClipboard();
        let ok_dib = SetClipboardData(CF_DIB, hmem_dib);
        let ok_drop = SetClipboardData(CF_HDROP, hmem_drop);
        CloseClipboard();
        if ok_dib == 0 || ok_drop == 0 {
            return Err("写入剪贴板失败".into());
        }
        Ok(())
    }
}

/// 清空全部记录（含关联图片/缩略图文件与 FTS 索引）。返回删除条数。
#[tauri::command]
pub fn clear_all(db: State<'_, Arc<Database>>) -> Result<usize, String> {
    let n = db.clear_all_items()?;
    Ok(n)
}

/// 数据迁移（文档 6.6）：把数据目录剪切移动到新位置，成功后更新 config 并返回提示。
/// 实际流程：锁库（暂停 worker 写入）→ 复制到新位置 → 校验完整性 → 更新 config.json
/// （storage_path 指向新位置 + pending_cleanup_dir 记录旧目录）→ 旧目录待重启后清理。
#[tauri::command]
pub fn migrate_storage(
    db: State<'_, Arc<Database>>,
    state: State<'_, AppState>,
    settings: AppConfig,
) -> Result<String, String> {
    let new_path_str = settings.storage_path.trim().to_string();
    let old_data_dir = db.data_dir.clone();
    let new_path = Path::new(&new_path_str);
    log::info!("迁移: 开始，目标={new_path_str}，来源={}", old_data_dir.display());

    // ---- 1) 校验目标路径 ----
    validate_storage_path(&new_path, &old_data_dir)?;
    log::info!("迁移: 路径校验通过");

    // ---- 2) 锁数据库（阻塞 worker 写入，保证复制期间数据库稳定）----
    let conn_guard = db.conn.lock().map_err(|e| format!("锁定数据库失败: {e}"))?;
    log::info!("迁移: 数据库已锁定，开始复制");
    // ---- 3) 复制到新位置 ----
    if let Err(e) = db::copy_data_dir(&old_data_dir, new_path) {
        drop(conn_guard);
        db::cleanup_data_dir(new_path); // 回滚：清理新位置半成品，旧位置保持可用
        log::error!("迁移: 复制失败，已回滚: {e}");
        return Err(e);
    }
    log::info!("迁移: 复制完成，开始校验");
    // ---- 4) 校验完整性 ----
    if let Err(e) = db::verify_data_dir(&old_data_dir, new_path) {
        drop(conn_guard);
        db::cleanup_data_dir(new_path); // 回滚：清理新位置半成品，旧位置保持可用
        log::error!("迁移: 校验失败，已回滚: {e}");
        return Err(e);
    }
    log::info!("迁移: 校验通过");
    // 释放锁（迁移命令后续不再访问旧库；worker 恢复写旧库，重启前新记录进旧库，重启后完整迁移）
    drop(conn_guard);

    // ---- 4.5) 更新新库中图片/缩略图路径（旧目录前缀 → 新目录前缀），否则迁移后旧记录仍指向旧路径 ----
    if let Err(e) = db::update_migrated_paths(new_path, &old_data_dir) {
        db::cleanup_data_dir(new_path); // 回滚：清理新位置半成品，旧位置保持可用
        log::error!("迁移: 更新新库路径失败，已回滚: {e}");
        return Err(e);
    }

    // ---- 5) 更新 config.json（storage_path 指向新位置 + 待清理旧目录 + 保留其余设置）----
    let mut new_cfg = settings.clone();
    new_cfg.storage_path = new_path_str.clone();
    new_cfg.pending_cleanup_dir = Some(old_data_dir.to_string_lossy().into_owned());
    {
        let mut cfg = state.config.lock().map_err(|e| e.to_string())?;
        *cfg = new_cfg.clone();
        save_config(&state.config_path, &cfg)?;
    }
    log::info!(
        "数据迁移完成: 来源 {} → 目标 {}",
        old_data_dir.display(),
        new_path.display()
    );
    Ok(format!("数据已迁移到 {new_path_str}，程序将重启"))
}

/// 迁移前校验目标路径：绝对路径、本地磁盘、非系统目录、非程序安装目录、与当前数据目录不同
fn validate_storage_path(new_path: &Path, old_data_dir: &Path) -> Result<(), String> {
    if new_path.as_os_str().is_empty() {
        return Err("请选择数据保存位置".into());
    }
    if !new_path.is_absolute() {
        return Err("目标必须为绝对路径".into());
    }
    // 与当前数据目录相同则无需迁移
    if new_path == old_data_dir {
        return Err("目标位置与当前数据目录相同，无需迁移".into());
    }
    // 本地磁盘路径（Windows）：首组件必须是盘符前缀（如 D:\，排除相对路径）
    // 注意：不能用 ancestors().last().has_root()——对 "D:\xxx" 其 last 是 "D:"，has_root 恒 false
    use std::path::Component;
    if !matches!(new_path.components().next(), Some(Component::Prefix(_))) {
        return Err("目标必须为本地磁盘路径".into());
    }
    // 可写性：尝试创建目录 + 写临时文件
    if let Err(e) = std::fs::create_dir_all(new_path) {
        return Err(format!("目标目录不可创建: {e}"));
    }
    let probe = new_path.join(".cm_write_test");
    if let Err(e) = std::fs::write(&probe, b"ok") {
        return Err(format!("目标目录不可写: {e}"));
    }
    let _ = std::fs::remove_file(&probe);
    // 不得为系统目录或程序安装目录（常见系统目录黑名单 + 当前 exe 所在目录）
    let sys_dirs = [
        std::env::var("WINDIR").unwrap_or_default(),
        std::env::var("ProgramFiles").unwrap_or_default(),
        "C:\\Program Files (x86)".into(),
        std::env::var("ProgramData").unwrap_or_default(),
    ];
    for sd in sys_dirs.iter().filter(|s| !s.is_empty()) {
        let sp = Path::new(sd);
        if new_path == sp || new_path.starts_with(sp) {
            return Err("目标不得为系统目录".into());
        }
    }
    // 程序自身安装目录（当前 exe 所在目录）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if new_path == exe_dir || new_path.starts_with(exe_dir) {
                return Err("目标不得为程序安装目录".into());
            }
        }
    }
    Ok(())
}

/// 迁移成功后重启程序（前端收到提示后调用）
#[tauri::command]
pub fn relaunch_app(app: AppHandle) {
    log::info!("数据迁移后重启程序");
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // 回归测试：修复前 ancestors().last().has_root() 对所有盘符路径恒为 false，
    // 导致合法目标（D:\xxx 等）一律被拒。现改用 components().next() 判断盘符前缀。
    #[test]
    fn validate_accepts_disk_drive_path() {
        // D:\ 盘符路径应通过（不在系统/安装目录黑名单内）
        let r = validate_storage_path(Path::new("D:\\some_data"), Path::new("C:\\old_data"));
        assert!(r.is_ok(), "D:\\ 盘符路径应通过校验，实际: {r:?}");
    }

    #[test]
    fn validate_rejects_relative_path() {
        let r = validate_storage_path(Path::new("some/relative"), Path::new("C:\\old_data"));
        assert!(r.is_err(), "相对路径应被拒绝");
    }

    #[test]
    fn validate_rejects_same_dir() {
        let r = validate_storage_path(Path::new("C:\\old_data"), Path::new("C:\\old_data"));
        assert!(r.is_err(), "与当前目录相同应被拒绝");
    }
}

/// 多选批量复制（文档 6.3）：ids 按选择顺序传入。
/// 文本组 → 按序号顺序换行拼接，一次性写入剪贴板；
/// 文件/图片组 → 收集所有路径（图片=原图 PNG 路径，文件=全部路径），CF_HDROP 一次性写入。
/// 写剪贴板会触发 WM_CLIPBOARDUPDATE，库内去重保证不产生重复记录。
#[tauri::command]
pub fn batch_copy(
    db: State<'_, Arc<Database>>,
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<String, String> {
    if ids.is_empty() {
        return Err("未选择任何记录".into());
    }
    // 标记本次为"程序内部复制"（写剪贴板前设置）
    {
        let mut guard = state.internal_copy.lock().map_err(|e| e.to_string())?;
        *guard = ids.clone();
    }
    // 按传入顺序取记录；任一缺失即报错，防止静默丢项
    let mut items = Vec::with_capacity(ids.len());
    for id in &ids {
        match db.get_item(*id)? {
            Some(item) => items.push(item),
            None => return Err(format!("记录不存在 id={id}")),
        }
    }
    // 组校验：文本 与 文件/图片 不可混选（前端已拦截，此处后端防御）
    let has_text = items.iter().any(|i| i.item_type == "text");
    let has_file_image = items.iter().any(|i| i.item_type == "image" || i.item_type == "file");
    if has_text && has_file_image {
        return Err("文本不能与文件/图片混选".into());
    }

    let mut cb = arboard::Clipboard::new().map_err(|e| format!("打开剪贴板失败: {e}"))?;

    if has_text {
        // 文本组：按序号顺序换行拼接（保留各条原始换行），一次性写入
        let joined = items
            .iter()
            .map(|i| i.content.as_deref().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        cb.set_text(joined).map_err(|e| format!("写入文本失败: {e}"))?;
        log::info!("批量复制文本 {} 条", items.len());
        return Ok(String::new());
    }

    // 文件/图片组：收集所有路径（图片=原图 PNG 路径；文件=全部路径）
    let mut paths: Vec<String> = Vec::new();
    for item in &items {
        match item.item_type.as_str() {
            "image" => {
                if let Some(p) = &item.image_path {
                    paths.push(p.clone());
                }
            }
            "file" => {
                if let Some(ps) = &item.file_paths {
                    paths.extend(ps.iter().cloned());
                }
            }
            _ => {}
        }
    }
    // 失效处理与单条一致：全部失效拒绝；部分失效复制存在的并提示缺失
    let existing: Vec<String> = paths
        .iter()
        .filter(|p| Path::new(p.as_str()).exists())
        .cloned()
        .collect();
    if existing.is_empty() {
        return Err("源文件不存在".into());
    }
    write_drop_files(&existing)?;
    let missing = paths.len() - existing.len();
    log::info!(
        "批量复制文件/图片 {} 条（{} 个文件）",
        items.len(),
        existing.len()
    );
    if missing > 0 {
        Ok(format!("{missing} 个文件已不存在，已复制剩余文件"))
    } else {
        Ok(String::new())
    }
}
