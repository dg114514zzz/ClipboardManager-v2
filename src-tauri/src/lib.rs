mod clipboard;
mod commands;
mod config;
mod db;
mod models;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

/// 共享状态：config.json 路径 + 内存中的配置（命令可读写）
pub struct AppState {
    pub config_path: PathBuf,
    pub config: Arc<Mutex<config::AppConfig>>,
    /// 最近一次"程序内部复制"涉及的记录 id（用于区分内部/外部复制，内部复制不触发拉顶刷新）
    pub internal_copy: Mutex<Vec<i64>>,
}

/// 切换主窗口显隐
fn toggle_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            if let Err(e) = window.hide() {
                log::error!("隐藏窗口失败: {e}");
            } else {
                log::info!("window hidden");
            }
        } else {
            if let Err(e) = window.unminimize() {
                log::error!("取消最小化失败: {e}");
            }
            if let Err(e) = window.show() {
                log::error!("显示窗口失败: {e}");
            } else {
                log::info!("window shown");
            }
            if let Err(e) = window.set_focus() {
                log::error!("窗口聚焦失败: {e}");
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 1) 固定配置目录 + 读 config.json 确定数据目录（此时日志插件尚未初始化）
    let config_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ClipboardManager");
    let (config_path, config) = config::load_config(&config_dir);
    let data_dir = PathBuf::from(&config.storage_path);

    // 2) 建数据目录与 logs 目录
    for d in [&data_dir, &data_dir.join("logs")] {
        if let Err(e) = std::fs::create_dir_all(d) {
            eprintln!("创建目录失败 {}: {e}", d.display());
        }
    }

    // 3) 日志插件 → 数据目录\logs\clipboard.log（随数据目录迁移）
    let log_target = tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
        path: data_dir.join("logs"),
        file_name: Some("clipboard".to_string()),
    });

    let state = AppState {
        config_path,
        config: Arc::new(Mutex::new(config)),
        internal_copy: Mutex::new(Vec::new()),
    };

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .target(log_target)
                .level(log::LevelFilter::Info)
                .max_file_size(5 * 1024 * 1024) // 5MB 滚动
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent, // Windows 上忽略该参数
            None,                                               // Run 值 = exe 路径，无附加参数
        ))
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(move |app| {
            let data_dir = data_dir.clone();

            // 启动日志 + 自启一致性修复（幂等，文档 5.3）
            {
                let state = app.state::<AppState>();
                let cfg = match state.config.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                log::info!("数据目录: {}", data_dir.display());
                log::info!("配置文件: {}", state.config_path.display());
                if cfg.auto_start {
                    use tauri_plugin_autostart::ManagerExt;
                    if let Err(e) = app.autolaunch().enable() {
                        log::error!("修复开机自启失败: {e}");
                    }
                }
            }

            // 数据迁移遗留：清理旧数据目录（重启后执行，旧进程已退出文件解锁；config.json 不在清理范围）
            {
                let state = app.state::<AppState>();
                let cleanup_dir = {
                    let cfg = match state.config.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    cfg.pending_cleanup_dir.clone()
                };
                if let Some(old) = cleanup_dir {
                    let old_path = PathBuf::from(&old);
                    crate::db::cleanup_data_dir(&old_path);
                    let mut cfg = match state.config.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    cfg.pending_cleanup_dir = None;
                    if let Err(e) = crate::config::save_config(&state.config_path, &cfg) {
                        log::error!("移除迁移待清理记录失败: {e}");
                    } else {
                        log::info!("数据迁移: 已清理旧数据目录 {old}");
                    }
                }
            }

            // 初始化数据库 + 启动事件驱动剪贴板监听
            let database = db::Database::new(data_dir.clone())
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let db_arc = std::sync::Arc::new(database);
            let app_handle = std::sync::Arc::new(app.handle().clone());
            clipboard::start_monitoring(app_handle, db_arc.clone())
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let cleanup_db = db_arc.clone();
            app.manage(db_arc);
            // 修复迁移后失效的图片/缩略图路径（迁移改了目录但未更新数据库路径的旧记录，启动时自愈）
            match cleanup_db.repair_missing_paths() {
                Ok(n) => {
                    if n > 0 {
                        log::info!("启动自愈: 修复 {} 条记录的图片/缩略图路径", n);
                    }
                }
                Err(e) => log::error!("启动自愈图片路径失败: {e}"),
            }
            // 清理图片抓取缓冲目录的残留（上次异常退出时未处理完的临时文件）
            match cleanup_db.cleanup_buffer() {
                Ok(n) => {
                    if n > 0 {
                        log::info!("启动清理: 删除 {n} 个缓冲残留文件");
                    }
                }
                Err(e) => log::error!("启动清理缓冲目录失败: {e}"),
            }
            log::info!("数据库已就绪: {}", data_dir.join("clipboard.db").display());

            // 自动清理（文档 5.3）：启动清理一次 + 每小时清理一次；删除非收藏且超保留天数记录
            {
                let state = app.state::<AppState>();
                let retention_arc = state.config.clone();
                let retention = {
                    let cfg = match state.config.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    cfg.retention_days as i64
                };
                match cleanup_db.cleanup_expired(retention) {
                    Ok(n) => log::info!("启动清理完成: 删除 {n} 条过期记录"),
                    Err(e) => log::error!("启动清理失败: {e}"),
                }
                // 每小时清理线程（保留天数取最新 config，用户改设置后生效）
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                    let retention = {
                        let cfg = match retention_arc.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        cfg.retention_days as i64
                    };
                    match cleanup_db.cleanup_expired(retention) {
                        Ok(n) => log::info!("每小时清理完成: 删除 {n} 条过期记录"),
                        Err(e) => log::error!("每小时清理失败: {e}"),
                    }
                });
            }

            // 托盘：左键显隐；右键菜单"显示窗口 / 退出"
            let show_item = MenuItemBuilder::with_id("show", "显示窗口").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "退出").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show_item)
                .separator()
                .item(&quit_item)
                .build()?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("历史剪贴板")
                .menu(&menu)
                .show_menu_on_left_click(false) // 关键：左键不弹菜单，触发 Click 事件
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => toggle_window(app),
                    "quit" => {
                        log::info!("托盘退出");
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_window(tray.app_handle()); // 左键单击显隐
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            // 全局快捷键（唯一注册）Alt+Shift+V 切换窗口
            use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
            let handle = app.handle().clone();
            if let Err(e) = app
                .global_shortcut()
                .on_shortcut("Alt+Shift+V", move |_a, _s, ev| {
                    if ev.state() == ShortcutState::Pressed {
                        toggle_window(&handle);
                    }
                })
            {
                log::error!("注册全局快捷键 Alt+Shift+V 失败: {e}");
            }

            // 点 X 隐藏到托盘（程序继续运行）。
            // 注意：CloseRequested 回调内立即 hide() 会失效（返回 Ok 但窗口不隐藏，Tauri 2 已知行为，
            // 日志已证实：同一点 X 的 hide 失效，而 toggle_window 里的 hide 正常）。故 prevent_close 后
            // 延迟到事件循环之外再 hide，绕过该限制；不用 Win32 直接操作 HWND（会绕过 Tauri 状态管理导致窗口销毁）。
            if let Some(window) = app.get_webview_window("main") {
                let w = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let w2 = w.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            if let Err(e) = w2.hide() {
                                log::error!("点X隐藏到托盘失败: {e}");
                            } else {
                                log::info!("窗口关闭 → 隐藏到托盘");
                            }
                        });
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::get_items,
            commands::search_items,
            commands::toggle_favorite,
            commands::toggle_pin,
            commands::delete_item,
            commands::copy_item,
            commands::batch_copy,
            commands::clear_all,
            commands::migrate_storage,
            commands::relaunch_app,
            commands::check_files_exist,
            commands::reveal_file,
            commands::get_thumbnail_base64,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
