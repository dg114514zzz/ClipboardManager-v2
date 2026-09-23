// 事件驱动剪贴板监听：AddClipboardFormatListener + WM_CLIPBOARDUPDATE
// 消息窗口线程只负责接收事件；工作线程解析存储，生产-消费队列解耦（文档 4.1）
use crate::db::Database;
use crate::models::ClipboardItem;
use chrono::Utc;
use image::RgbaImage;
use sha2::{Digest, Sha256};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

// ---------- Win32 FFI ----------
extern "system" {
    fn RegisterClassW(lpWndClass: *const WNDCLASSW) -> u16;
    fn CreateWindowExW(
        dwExStyle: u32,
        lpClassName: *const u16,
        lpWindowName: *const u16,
        dwStyle: u32,
        x: i32,
        y: i32,
        nWidth: i32,
        nHeight: i32,
        hWndParent: isize,
        hMenu: isize,
        hInstance: isize,
        lpParam: *mut c_void,
    ) -> isize;
    fn AddClipboardFormatListener(hwnd: isize) -> i32;
    fn RemoveClipboardFormatListener(hwnd: isize) -> i32;
    fn GetMessageW(lpMsg: *mut MSG, hWnd: isize, wMsgFilterMin: u32, wMsgFilterMax: u32) -> i32;
    fn TranslateMessage(lpMsg: *const MSG) -> i32;
    fn DispatchMessageW(lpMsg: *const MSG) -> isize;
    fn DefWindowProcW(hWnd: isize, msg: u32, wParam: usize, lParam: isize) -> isize;
    fn GetModuleHandleW(lpModuleName: *const u16) -> isize;
    fn GetWindowLongPtrW(hWnd: isize, nIndex: i32) -> isize;
    fn SetWindowLongPtrW(hWnd: isize, nIndex: i32, dwNewLong: isize) -> isize;
    fn GetLastError() -> u32;
    fn OpenClipboard(hWndNewOwner: isize) -> i32;
    fn CloseClipboard() -> i32;
    fn IsClipboardFormatAvailable(uFormat: u32) -> i32;
    fn GetClipboardData(uFormat: u32) -> isize;
    fn GlobalLock(hMem: isize) -> *mut u8;
    fn GlobalUnlock(hMem: isize) -> i32;
}

// Win32 结构体字段名遵循官方命名（非 snake_case）
#[repr(C)]
#[allow(non_snake_case)]
struct WNDCLASSW {
    style: u32,
    lpfnWndProc: Option<unsafe extern "system" fn(isize, u32, usize, isize) -> isize>,
    cbClsExtra: i32,
    cbWndExtra: i32,
    hInstance: isize,
    hIcon: isize,
    hCursor: isize,
    hbrBackground: isize,
    lpszMenuName: *const u16,
    lpszClassName: *const u16,
}

#[repr(C)]
#[derive(Default)]
#[allow(non_snake_case)]
struct MSG {
    hwnd: isize,
    message: u32,
    wParam: usize,
    lParam: isize,
    time: u32,
    pt: POINT,
}

#[repr(C)]
#[derive(Default)]
struct POINT {
    x: i32,
    y: i32,
}

const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const GWLP_USERDATA: i32 = -21;
const HWND_MESSAGE: isize = -3;
const CF_HDROP: u32 = 15;
const CF_UNICODETEXT: u32 = 13;
const WIDE_NULL: u16 = 0;
/// 图片抓取缓冲的磁盘上限（2GB）：超过即按最旧优先丢弃缓冲文件，避免写满用户磁盘
const BUFFER_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 图片处理队列容量：缓冲落盘后队列只存路径，内存极小，容量远大于 2GB 缓冲能容纳的图片数
const IMAGE_QUEUE_CAP: usize = 2048;

/// 待处理的图片缓冲任务：抓取线程落盘后交给处理线程
struct BufferJob {
    path: PathBuf,
    w: u32,
    h: u32,
    internal_ids: Vec<i64>,
}

/// 窗口过程：WM_CLIPBOARDUPDATE → 向工作线程发信号（非阻塞）
unsafe extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_CLIPBOARDUPDATE {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const mpsc::SyncSender<()>;
        if !ptr.is_null() {
            let _ = (*ptr).try_send(()); // 队列已满时丢弃，避免消息循环阻塞
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 启动监听：消息窗口线程（收事件）+ 图片处理线程（重活）+ 抓取线程（读剪贴板分派）
pub fn start_monitoring(app: Arc<AppHandle>, db: Arc<Database>) -> Result<(), String> {
    // 有界队列（容量 1）：消息线程 try_send 非阻塞，队列满时丢弃旧事件（事件合并）
    let (tx, rx) = mpsc::sync_channel::<()>(1);

    // 图片抓取缓冲：抓取线程把像素图快速落盘到 clip_buffer，处理线程异步消费（见文档 4.1 生产-消费解耦）
    let buffer_bytes = Arc::new(AtomicU64::new(0));
    let (img_tx, img_rx) = mpsc::sync_channel::<BufferJob>(IMAGE_QUEUE_CAP);

    // 消息窗口线程：只负责接收 WM_CLIPBOARDUPDATE，绝不做剪贴板读取
    std::thread::Builder::new()
        .name("clipboard-listener".into())
        .spawn(move || {
            listener_loop(tx);
        })
        .map_err(|e| e.to_string())?;

    // 图片处理线程：从缓冲队列取图，做 hash/去重/入库/缩略图（慢活，独立线程不阻塞抓取）
    {
        let img_app = app.clone();
        let img_db = db.clone();
        let img_buffer_bytes = buffer_bytes.clone();
        std::thread::Builder::new()
            .name("clipboard-image-worker".into())
            .spawn(move || image_worker(img_app, img_db, img_rx, img_buffer_bytes))
            .map_err(|e| e.to_string())?;
    }

    // 抓取线程：从队列取信号，读剪贴板；文本/文件同步处理，图片落盘缓冲后交给处理线程
    {
        let buffer_dir = db.buffer_dir.clone();
        let buffer_bytes = buffer_bytes.clone();
        std::thread::Builder::new()
            .name("clipboard-worker".into())
            .spawn(move || {
                let mut last_text: Option<String> = None;
                while let Ok(_) = rx.recv() {
                    // 合并积压的连续事件（同一批剪贴板变更只处理一次）
                    while rx.try_recv().is_ok() {}
                    if let Err(e) = process_clipboard(
                        &app,
                        &db,
                        &buffer_dir,
                        &buffer_bytes,
                        &img_tx,
                        &mut last_text,
                    ) {
                        log::error!("处理剪贴板内容失败: {e}");
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// 消息窗口线程主循环
fn listener_loop(tx: mpsc::SyncSender<()>) {
    let class_name: Vec<u16> = "ClipboardManagerMsgWindow\0".encode_utf16().collect();

    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };

        if RegisterClassW(&wc) == 0 {
            let code = GetLastError();
            log::error!("注册监听窗口类失败, GetLastError={code}");
            return;
        }

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE, // 仅消息窗口，不可见
            0,
            hinstance,
            std::ptr::null_mut(),
        );
        if hwnd == 0 {
            log::error!("创建剪贴板监听窗口失败, GetLastError={}", GetLastError());
            return;
        }

        // 把发送端存入窗口 user data，供窗口过程取用
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(Box::new(tx)) as isize);

        if AddClipboardFormatListener(hwnd) == 0 {
            log::error!("AddClipboardFormatListener 失败, GetLastError={}", GetLastError());
            return;
        }
        log::info!("剪贴板监听已启动（AddClipboardFormatListener 事件驱动）");

        // 消息循环
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, 0, 0, 0);
            if r <= 0 {
                break; // 0=WM_QUIT, -1=错误
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        RemoveClipboardFormatListener(hwnd);
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut mpsc::SyncSender<()>;
        if !ptr.is_null() {
            drop(Box::from_raw(ptr)); // 释放 sender
        }
    }
}

/// 处理一次剪贴板变更：读内容 → 去重 → 入库 → 推送事件
fn process_clipboard(
    app: &AppHandle,
    db: &Database,
    buffer_dir: &Path,
    buffer_bytes: &AtomicU64,
    img_tx: &mpsc::SyncSender<BufferJob>,
    last_text: &mut Option<String>,
) -> Result<(), String> {
    // 处理开始时：取出"内部复制标记"并清空（本次处理用它判断是否跳过拉顶刷新）
    let internal_ids: Vec<i64> = {
        let state = app.state::<crate::AppState>();
        let mut guard = state.internal_copy.lock().map_err(|e| e.to_string())?;
        guard.drain(..).collect()
    };
    // 1) 文件（CF_HDROP）：解析全部路径；全为图片文件 → 逐张转存图片记录；否则 → 文件记录
    if let Some(paths) = read_drop_files() {
        if paths.is_empty() {
            return Ok(());
        }
        return process_files(app, db, &paths, &internal_ids);
    }

    // 2) 文本（CF_UNICODETEXT）
    if let Some(text) = read_text() {
        if text.is_empty() {
            return Ok(());
        }
        // 时序去重：与上一次内容相同 → 重复复制，同样刷新时间拉到最顶（方案A）；
        // 若是程序内部复制（internal_ids 命中）则跳过刷新，保持位置/时间不变
        if last_text.as_deref() == Some(&text) {
            if let Some(existing_id) = db.find_text(&text)? {
                if !internal_ids.contains(&existing_id) {
                    db.touch_item(existing_id)?;
                    log::info!("重复文本，刷新时间 id={existing_id}");
                    app.emit("clipboard-updated", ()).map_err(|e| e.to_string())?;
                }
            }
            return Ok(());
        }
        *last_text = Some(text.clone());
        // 库内去重：content 精确匹配 → 命中则刷新时间拉到最顶（内部复制跳过）
        if let Some(existing_id) = db.find_text(&text)? {
            if !internal_ids.contains(&existing_id) {
                db.touch_item(existing_id)?;
                log::info!("重复文本，刷新时间 id={existing_id}");
                app.emit("clipboard-updated", ()).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        let item = ClipboardItem {
            id: 0,
            item_type: "text".into(),
            content: Some(text),
            image_path: None,
            thumbnail_path: None,
            file_paths: None,
            file_count: 0,
            is_favorite: false,
            is_pinned: false,
            created_at: now_iso(),
            source_app: None,
            image_hash: None,
        };
        let id = db.add_item(&item)?;
        log::info!("文本记录入库 id={id}");
        app.emit("clipboard-updated", &item).map_err(|e| e.to_string())?;
        return Ok(());
    }

    // 3) 图片（像素图）：读像素 → 快速落盘缓冲 → 交给处理线程（重活不阻塞抓取，避免连续复制丢图）
    if let Some(img) = read_image_raw() {
        let rgba = match RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned()) {
            Some(rgba) => rgba,
            None => return Err("图片像素数据非法（from_raw 返回 None）".into()),
        };
        let ts = Utc::now().format("%Y%m%d_%H%M%S%6f").to_string();
        let buf_path = buffer_dir.join(format!("buf_{ts}.png"));
        save_png_fast(&rgba, &buf_path)?;
        let size = std::fs::metadata(&buf_path).map(|m| m.len()).unwrap_or(0);
        buffer_bytes.fetch_add(size, Ordering::SeqCst);
        // 超过 2GB 上限：删最旧缓冲文件腾空间（仅在处理严重跟不上时触发）
        enforce_buffer_limit(buffer_dir, buffer_bytes);
        let job = BufferJob {
            path: buf_path.clone(),
            w: rgba.width(),
            h: rgba.height(),
            internal_ids: internal_ids.clone(),
        };
        if img_tx.try_send(job).is_err() {
            // 处理队列已满（几乎不会发生）：丢弃本次并回收缓冲文件与计数
            if std::fs::remove_file(&buf_path).is_ok() {
                buffer_bytes.fetch_sub(size, Ordering::SeqCst);
            }
            log::warn!("图片处理队列已满，丢弃 1 张图片");
        }
        return Ok(());
    }

    Ok(())
}

/// 图片处理线程主循环：从缓冲队列取图处理；无论 rename 还是删除，处理完都从缓冲计数扣减
fn image_worker(
    app: Arc<AppHandle>,
    db: Arc<Database>,
    img_rx: mpsc::Receiver<BufferJob>,
    buffer_bytes: Arc<AtomicU64>,
) {
    for job in img_rx {
        let size = std::fs::metadata(&job.path).map(|m| m.len()).unwrap_or(0);
        if let Err(e) = process_image_job(&app, &db, &job) {
            log::error!("处理图片缓冲失败 {}: {e}", job.path.display());
        }
        buffer_bytes.fetch_sub(size, Ordering::SeqCst);
    }
}

/// 处理单个图片缓冲任务：读 PNG → hash → 库内去重（命中拉顶）或 rename 入正式目录 + 入库 + 缩略图
fn process_image_job(app: &AppHandle, db: &Database, job: &BufferJob) -> Result<(), String> {
    let rgba = image::open(&job.path)
        .map_err(|e| format!("读取缓冲图片失败 {}: {e}", job.path.display()))?
        .to_rgba8();
    let hash = hash_bytes(rgba.as_raw());
    // 库内去重：内容指纹匹配 → 命中则刷新时间拉到最顶（内部复制跳过）
    if let Some(existing_id) = db.find_image_by_hash(&hash)? {
        if let Err(e) = std::fs::remove_file(&job.path) {
            log::warn!("删除重复缓冲文件失败 {}: {e}", job.path.display());
        }
        if !job.internal_ids.contains(&existing_id) {
            db.touch_item(existing_id)?;
            log::info!("重复图片，刷新时间 id={existing_id}");
            app.emit("clipboard-updated", ()).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    // 新图片：把缓冲 PNG 移入正式 images 目录（同卷 rename，快，避免二次编码）
    let ts = Utc::now().format("%Y%m%d_%H%M%S%6f").to_string();
    let dest = db.images_dir.join(format!("clip_{ts}.png"));
    std::fs::rename(&job.path, &dest).map_err(|e| format!("移动缓冲图片到正式目录失败: {e}"))?;
    let item = ClipboardItem {
        id: 0,
        item_type: "image".into(),
        content: None,
        image_path: Some(dest.to_string_lossy().to_string()),
        thumbnail_path: None,
        file_paths: None,
        file_count: 0,
        is_favorite: false,
        is_pinned: false,
        created_at: now_iso(),
        source_app: None,
        image_hash: Some(hash),
    };
    let id = db.add_item(&item)?;
    log::info!("图片记录入库 id={id} {}x{}", job.w, job.h);
    // 生成缩略图（用内存 RGBA，避免重新解码 PNG）+ 配额检查
    if let Err(e) = db.generate_thumbnail_from_rgba(id, &rgba) {
        log::error!("生成缩略图失败 id={id}: {e}");
    }
    // "一直保留"时不清理图片（用户选择彻底不删）
    if !retention_forever(app) {
        if let Err(e) = db.enforce_image_quota() {
            log::error!("图片配额清理失败: {e}");
        }
    }
    app.emit("clipboard-updated", &item).map_err(|e| e.to_string())?;
    Ok(())
}

/// 缓冲超过 2GB 上限时，按文件名时间戳（buf_ 前缀，字典序≈时间序）从最旧开始删除，直到低于上限
fn enforce_buffer_limit(buffer_dir: &Path, buffer_bytes: &AtomicU64) {
    if buffer_bytes.load(Ordering::SeqCst) <= BUFFER_LIMIT_BYTES {
        return;
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(buffer_dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    files.sort(); // 同目录下按文件名排序 = 按写入时间排序（旧→新）
    let mut total = buffer_bytes.load(Ordering::SeqCst);
    for p in files {
        if total <= BUFFER_LIMIT_BYTES {
            break;
        }
        let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if std::fs::remove_file(&p).is_ok() {
            total = total.saturating_sub(sz);
            buffer_bytes.fetch_sub(sz, Ordering::SeqCst);
            log::info!("缓冲已满，删除最旧缓冲文件 {}", p.display());
        }
    }
}

/// 处理一组文件路径：全为图片文件 → 逐张转存图片记录；否则 → 文件记录
fn process_files(
    app: &AppHandle,
    db: &Database,
    paths: &[String],
    internal_ids: &[i64],
) -> Result<(), String> {
    // 逐个文件：能按内容解码成图 → 图片记录；否则收集为文件记录（降级）
    let mut non_image: Vec<String> = Vec::new();
    for p in paths {
        match try_process_image(app, db, p, internal_ids)? {
            true => {}
            false => non_image.push(p.clone()),
        }
    }
    if !non_image.is_empty() {
        process_file_record(app, db, &non_image, internal_ids)?;
    }
    Ok(())
}

/// 尝试把文件当图片处理：按内容解码（支持改后缀的情况），成功返回 true；
/// 内容不是图片返回 false（由调用方降级为文件记录）；其他错误返回 Err。
fn try_process_image(
    app: &AppHandle,
    db: &Database,
    path: &str,
    internal_ids: &[i64],
) -> Result<bool, String> {
    // 先按扩展名快速解码；失败则读文件头判断是否图片格式（避免把大视频/文档整个读进内存）
    let img = match image::open(path) {
        Ok(img) => img,
        Err(_) => {
            let head = read_file_head(path)?;
            if !is_image_magic(&head) {
                return Ok(false); // 内容不是图片 → 降级为文件记录
            }
            let bytes = std::fs::read(path).map_err(|e| format!("读取文件失败 {path}: {e}"))?;
            match image::load_from_memory(&bytes) {
                Ok(img) => img,
                Err(e) => {
                    log::warn!("图片内容解码失败 {path}: {e}，按文件处理");
                    return Ok(false);
                }
            }
        }
    };
    let rgba = img.to_rgba8();
    let hash = hash_bytes(rgba.as_raw());
    // 库内去重：内容指纹（与位图/其他来源同画面判重）→ 命中则刷新时间拉到最顶（内部复制跳过）
    if let Some(existing_id) = db.find_image_by_hash(&hash)? {
        if !internal_ids.contains(&existing_id) {
            db.touch_item(existing_id)?;
            log::info!("重复图片文件，刷新时间 id={existing_id}");
            app.emit("clipboard-updated", ()).map_err(|e| e.to_string())?;
        }
        return Ok(true);
    }
    let ts = Utc::now().format("%Y%m%d_%H%M%S%3f").to_string();
    let dest = db.images_dir.join(format!("file_{ts}.png"));
    save_png_fast(&rgba, &dest)?;
    let item = ClipboardItem {
        id: 0,
        item_type: "image".into(),
        content: None,
        image_path: Some(dest.to_string_lossy().to_string()),
        thumbnail_path: None,
        file_paths: None,
        file_count: 0,
        is_favorite: false,
        is_pinned: false,
        created_at: now_iso(),
        source_app: Some(path.to_string()),
        image_hash: Some(hash),
    };
    let id = db.add_item(&item)?;
    log::info!("图片文件转存入库 id={id} {path}");
    // 生成缩略图（用内存 RGBA，避免重新解码 PNG）+ 配额检查
    if let Err(e) = db.generate_thumbnail_from_rgba(id, &rgba) {
        log::error!("生成缩略图失败 id={id}: {e}");
    }
    // "一直保留"时不清理图片（用户选择彻底不删）
    if !retention_forever(app) {
        if let Err(e) = db.enforce_image_quota() {
            log::error!("图片配额清理失败: {e}");
        }
    }
    app.emit("clipboard-updated", &item).map_err(|e| e.to_string())?;
    Ok(true)
}

/// 读取文件开头 12 字节（用于判断内容魔数，避免把大视频/文档整个读进内存）
fn read_file_head(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("打开文件失败 {path}: {e}"))?;
    let mut buf = vec![0u8; 12];
    let n = f.read(&mut buf).map_err(|e| format!("读取文件头失败 {path}: {e}"))?;
    buf.truncate(n);
    Ok(buf)
}

/// 按文件头魔数判断是否为常见图片格式（PNG/JPEG/GIF/BMP/WEBP/TIFF/ICO）
fn is_image_magic(head: &[u8]) -> bool {
    if head.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return true; // PNG
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true; // JPEG
    }
    if head.starts_with(&[0x47, 0x49, 0x46, 0x38]) {
        return true; // GIF
    }
    if head.starts_with(&[0x42, 0x4D]) {
        return true; // BMP
    }
    if head.len() >= 12 && head.starts_with(&[0x52, 0x49, 0x46, 0x46]) && head[8..12] == [0x57, 0x45, 0x42, 0x50] {
        return true; // WEBP
    }
    if head.len() >= 4
        && (head.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || head.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]))
    {
        return true; // TIFF
    }
    if head.len() >= 4 && head.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return true; // ICO
    }
    if head.len() >= 12
        && head[4] == b'f'
        && head[5] == b't'
        && head[6] == b'y'
        && head[7] == b'p'
        && ((head[8] == b'a' && head[9] == b'v' && head[10] == b'i' && head[11] == b'f')
            || (head[8] == b'a' && head[9] == b'v' && head[10] == b'i' && head[11] == b's'))
    {
        return true; // AVIF（ISOBMFF：ftypavif / ftypavis）
    }
    false
}

/// 文件记录：记录全部路径列表与数量
fn process_file_record(
    app: &AppHandle,
    db: &Database,
    paths: &[String],
    internal_ids: &[i64],
) -> Result<(), String> {
    // 库内去重：路径列表集合一致 → 命中则刷新时间拉到最顶（内部复制跳过）
    if let Some(existing_id) = db.find_file(paths)? {
        if !internal_ids.contains(&existing_id) {
            db.touch_item(existing_id)?;
            log::info!("重复文件，刷新时间 id={existing_id}");
            app.emit("clipboard-updated", ()).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let item = ClipboardItem {
        id: 0,
        item_type: "file".into(),
        content: None,
        image_path: None,
        thumbnail_path: None,
        file_paths: Some(paths.to_vec()),
        file_count: paths.len() as i64,
        is_favorite: false,
        is_pinned: false,
        created_at: now_iso(),
        source_app: paths.first().cloned(),
        image_hash: None,
    };
    let id = db.add_item(&item)?;
    log::info!("文件记录入库 id={id} {} 个文件", paths.len());
    app.emit("clipboard-updated", &item).map_err(|e| e.to_string())?;
    Ok(())
}

/// 用户是否选择了"一直保留"（retention_days = 0）。
/// 此时禁用一切自动清理——既包括按天数清理（db::cleanup_expired 内部判断），
/// 也包括图片 1GB 配额清理（调用处判断）
fn retention_forever(app: &AppHandle) -> bool {
    let state = app.state::<crate::AppState>();
    let cfg = match state.config.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    cfg.retention_days == 0
}

fn now_iso() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// 解析 CF_HDROP 全部文件路径（v2 修复 v1 只读第一个的缺陷 #10）
fn read_drop_files() -> Option<Vec<String>> {
    if !open_clipboard_retry() {
        return None;
    }
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP) == 0 {
            CloseClipboard();
            return None;
        }
        let h = GetClipboardData(CF_HDROP);
        if h == 0 {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(h) as *const u8;
        if ptr.is_null() {
            CloseClipboard();
            return None;
        }
        let p_files = u32::from_le_bytes([*ptr, *ptr.add(1), *ptr.add(2), *ptr.add(3)]) as usize;
        let f_wide = u32::from_le_bytes([*ptr.add(16), *ptr.add(17), *ptr.add(18), *ptr.add(19)]) != 0;

        let mut paths = Vec::new();
        if f_wide {
            let base = ptr.add(p_files) as *const u16;
            let mut offset = 0usize;
            loop {
                let mut end = offset;
                while *base.add(end) != WIDE_NULL {
                    end += 1;
                }
                if end == offset {
                    break; // 双 null 结束
                }
                paths.push(String::from_utf16_lossy(std::slice::from_raw_parts(base.add(offset), end - offset)));
                offset = end + 1;
            }
        } else {
            let base = ptr.add(p_files);
            let mut offset = 0usize;
            loop {
                let mut end = offset;
                while *base.add(end) != 0 {
                    end += 1;
                }
                if end == offset {
                    break;
                }
                paths.push(String::from_utf8_lossy(std::slice::from_raw_parts(base.add(offset), end - offset)).to_string());
                offset = end + 1;
            }
        }
        GlobalUnlock(h);
        CloseClipboard();
        Some(paths)
    }
}

/// 尝试打开剪贴板：与写剪贴板进程存在瞬时竞争，失败时短时重试，避免漏记
fn open_clipboard_retry() -> bool {
    for _ in 0..5 {
        unsafe {
            if OpenClipboard(0) != 0 {
                return true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    false
}

/// 读 CF_UNICODETEXT 文本
fn read_text() -> Option<String> {
    if !open_clipboard_retry() {
        return None;
    }
    unsafe {
        let result = (|| {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                return None;
            }
            let h = GetClipboardData(CF_UNICODETEXT);
            if h == 0 {
                return None;
            }
            let ptr = GlobalLock(h) as *const u16;
            if ptr.is_null() {
                return None;
            }
            // UTF-16LE 到第一个 null，上限 64KB
            let mut end = 0usize;
            while end < 32768 && *ptr.add(end) != WIDE_NULL {
                end += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, end));
            GlobalUnlock(h);
            Some(text)
        })();
        CloseClipboard();
        result
    }
}

/// 读像素图片（arboard 内部完成 CF_DIB → RGBA），只返回原始像素，不计算指纹（hash 交给处理线程）
fn read_image_raw() -> Option<arboard::ImageData<'static>> {
    for _ in 0..5 {
        let img = match arboard::Clipboard::new() {
            Ok(mut cb) => match cb.get_image() {
                Ok(img) => img,
                // 剪贴板无图片属于正常情况，不重试
                Err(arboard::Error::ContentNotAvailable) => return None,
                Err(other) => {
                    log::warn!("读取剪贴板图片失败: {other}");
                    std::thread::sleep(std::time::Duration::from_millis(40));
                    continue;
                }
            },
            Err(e) => {
                log::warn!("打开剪贴板失败: {e}");
                std::thread::sleep(std::time::Duration::from_millis(40));
                continue;
            }
        };
        return Some(img);
    }
    None
}

/// 像素字节 SHA-256 → 十六进制指纹
fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 快速编码 PNG：image 默认 PngEncoder 用 Adaptive 滤波 + DEFLATE 级别 6，
/// 4K 大图编码可达十几秒。改用 Fast 压缩 + Sub 滤波大幅提速（代价是文件略大，仍在 1GB 配额内）。
fn save_png_fast(rgba: &RgbaImage, dest: &std::path::Path) -> Result<(), String> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let file = std::fs::File::create(dest).map_err(|e| format!("创建图片文件失败: {e}"))?;
    let writer = std::io::BufWriter::new(file);
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::Sub);
    rgba
        .write_with_encoder(encoder)
        .map_err(|e| format!("保存图片失败: {e}"))
}
