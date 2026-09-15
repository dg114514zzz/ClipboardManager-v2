use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// config.json 结构（固定配置目录 %APPDATA%\ClipboardManager\config.json）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub storage_path: String, // 数据目录绝对路径；默认=配置目录本身
    pub retention_days: u32,  // 默认 7
    pub auto_start: bool,     // 默认 false
    /// 数据迁移后待清理的旧数据目录（重启后由新进程清理，清理完移除该字段）
    #[serde(default)]
    pub pending_cleanup_dir: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            storage_path: String::new(),
            retention_days: 7,
            auto_start: false,
            pending_cleanup_dir: None,
        }
    }
}

/// 读取固定位置的 config.json；缺失/损坏/storage_path 为空 → 回退默认值。
/// 返回 (config_path, config)。
///
/// 注意：本函数在日志插件初始化之前调用，错误无法走 log 宏，用 eprintln 提示。
pub fn load_config(config_dir: &Path) -> (PathBuf, AppConfig) {
    let config_path = config_dir.join("config.json");
    let default_config = AppConfig {
        storage_path: config_dir.to_string_lossy().into_owned(),
        ..AppConfig::default()
    };
    let config = match std::fs::read_to_string(&config_path) {
        Ok(s) => match serde_json::from_str::<AppConfig>(&s) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("config.json 解析失败, 使用默认值: {e}");
                default_config.clone()
            }
        },
        Err(_) => {
            eprintln!("config.json 不存在, 使用默认值");
            // 文档 8 章：config.json 缺失时回退默认值；启动时若缺失自动创建默认文件，确保数据目录可定位
            if let Err(e) = save_config(&config_path, &default_config) {
                eprintln!("创建默认 config.json 失败: {e}");
            }
            default_config.clone()
        }
    };
    let config = if config.storage_path.trim().is_empty() {
        default_config
    } else {
        config
    };
    (config_path, config)
}

/// 原子写：同目录临时文件 + rename（同盘 rename 原子，避免写一半损坏）。
pub fn save_config(config_path: &Path, config: &AppConfig) -> Result<(), String> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let tmp = config_path.with_extension("tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, config_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("写入 config.json 失败: {e}")
    })?;
    Ok(())
}
