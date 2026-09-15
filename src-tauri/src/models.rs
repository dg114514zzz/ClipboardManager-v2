use serde::{Deserialize, Serialize};

/// 剪贴板记录，对应文档 7.1 表 clipboard_items。
/// file_paths 对外是路径列表（数组）；存储到 DB 时序列化为 JSON 字符串。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub id: i64,
    pub item_type: String, // "text" | "image" | "file"
    pub content: Option<String>,
    pub image_path: Option<String>,
    pub thumbnail_path: Option<String>,
    pub file_paths: Option<Vec<String>>,
    pub file_count: i64,
    pub is_favorite: bool,
    pub is_pinned: bool,
    pub created_at: String, // ISO 8601 字符串
    pub source_app: Option<String>,
    /// 图片内容指纹（像素 SHA-256），用于库内去重。仅图片类型使用。
    pub image_hash: Option<String>,
}
