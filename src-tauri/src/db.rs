use chrono::{Duration, Utc};
use image::GenericImageView;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::models::ClipboardItem;

/// 显式列名（不依赖表列序）：v1 旧库迁移后 ALTER 新增列位于末尾，SELECT * 按位置取值会错位
/// 带表前缀，供 FTS5 JOIN 查询使用（items_fts 的 content/file_paths/file_name 与之区分）
const ITEM_COLS: &str = "clipboard_items.id, clipboard_items.item_type, clipboard_items.content, clipboard_items.image_path, clipboard_items.thumbnail_path, clipboard_items.file_paths, clipboard_items.file_count, clipboard_items.is_favorite, clipboard_items.is_pinned, clipboard_items.created_at, clipboard_items.source_app, clipboard_items.image_hash";

pub struct Database {
    pub conn: Mutex<Connection>,
    pub data_dir: PathBuf,
    pub images_dir: PathBuf,
    pub thumbs_dir: PathBuf,
    /// 图片抓取缓冲目录：抓取线程把像素图快速落盘到这里，处理线程再慢慢消费（避免大图慢处理阻塞抓取导致丢图）
    pub buffer_dir: PathBuf,
}

impl Database {
    pub fn new(data_dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let images_dir = data_dir.join("images");
        std::fs::create_dir_all(&images_dir).map_err(|e| e.to_string())?;
        let thumbs_dir = data_dir.join("thumbs");
        std::fs::create_dir_all(&thumbs_dir).map_err(|e| e.to_string())?;
        let buffer_dir = data_dir.join("clip_buffer");
        std::fs::create_dir_all(&buffer_dir).map_err(|e| e.to_string())?;
        let conn = Connection::open(data_dir.join("clipboard.db")).map_err(|e| e.to_string())?;
        let db = Self {
            conn: Mutex::new(conn),
            data_dir,
            images_dir,
            thumbs_dir,
            buffer_dir,
        };
        db.init_schema()?;
        // 迁移：为旧图片记录补齐 image_hash（图片文件仍存在时重算，不存在则留空）
        if let Err(e) = db.backfill_image_hashes() {
            log::error!("补齐旧图片指纹失败: {e}");
        }
        // 迁移：为旧图片记录生成缺失的缩略图
        if let Err(e) = db.backfill_thumbnails() {
            log::error!("补齐旧图片缩略图失败: {e}");
        }
        // 迁移：重建 FTS5 全文索引（覆盖已有全部记录）
        if let Err(e) = db.backfill_fts() {
            log::error!("重建 FTS 索引失败: {e}");
        }
        Ok(db)
    }

    /// 建表 + v1 旧库迁移（补齐缺失列）
    fn init_schema(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS clipboard_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item_type TEXT NOT NULL,
                content TEXT,
                image_path TEXT,
                thumbnail_path TEXT,
                file_paths TEXT,
                file_count INTEGER DEFAULT 0,
                is_favorite INTEGER DEFAULT 0,
                is_pinned INTEGER DEFAULT 0,
                created_at TEXT NOT NULL,
                source_app TEXT,
                image_hash TEXT
            );
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_created_at ON clipboard_items(created_at);
            CREATE INDEX IF NOT EXISTS idx_favorite ON clipboard_items(is_favorite);
            CREATE INDEX IF NOT EXISTS idx_pinned ON clipboard_items(is_pinned);
            CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(content, file_paths, file_name, tokenize = 'unicode61');",
        )
        .map_err(|e| e.to_string())?;

        // v1 旧库升级：补齐 v2 新增列
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(clipboard_items)")
            .map_err(|e| e.to_string())?
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for col in ["thumbnail_path", "file_count", "image_hash"] {
            if !cols.iter().any(|c| c == col) {
                let sql = if col == "file_count" {
                    "ALTER TABLE clipboard_items ADD COLUMN file_count INTEGER DEFAULT 0"
                } else {
                    &format!("ALTER TABLE clipboard_items ADD COLUMN {col} TEXT")
                };
                conn.execute_batch(sql).map_err(|e| e.to_string())?;
                log::info!("数据库迁移: clipboard_items 新增列 {col}");
            }
        }
        Ok(())
    }

    pub fn add_item(&self, item: &ClipboardItem) -> Result<i64, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO clipboard_items
                (item_type, content, image_path, thumbnail_path, file_paths, file_count,
                 is_favorite, is_pinned, created_at, source_app, image_hash)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                item.item_type,
                item.content,
                item.image_path,
                item.thumbnail_path,
                item.file_paths.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()),
                item.file_count,
                item.is_favorite as i32,
                item.is_pinned as i32,
                item.created_at,
                item.source_app,
                item.image_hash,
            ],
        )
        .map_err(|e| e.to_string())?;
        let id = tx.last_insert_rowid();
        // 同步维护 FTS5 索引（同一事务内，保证一致）
        let (paths_text, file_name) = fts_paths(item);
        tx.execute(
            "INSERT INTO items_fts(rowid, content, file_paths, file_name) VALUES (?1, ?2, ?3, ?4)",
            params![
                id,
                fts_text(item.content.as_deref().unwrap_or("")),
                fts_text(&paths_text),
                fts_text(&file_name),
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(id)
    }

    pub fn get_items(&self, favorite_only: bool, sort_asc: bool) -> Result<Vec<ClipboardItem>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let order = if sort_asc { "ASC" } else { "DESC" };
        let query = if favorite_only {
            format!("SELECT {ITEM_COLS} FROM clipboard_items WHERE is_favorite = 1 ORDER BY is_pinned DESC, created_at {order}")
        } else {
            format!("SELECT {ITEM_COLS} FROM clipboard_items ORDER BY is_pinned DESC, created_at {order}")
        };
        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], row_to_item)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// 库内去重：文本按 content 精确匹配
    pub fn find_text(&self, content: &str) -> Result<Option<i64>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT id FROM clipboard_items WHERE item_type = 'text' AND content = ?1",
            params![content],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// 库内去重：图片按内容指纹（image_hash）匹配
    pub fn find_image_by_hash(&self, hash: &str) -> Result<Option<i64>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT id FROM clipboard_items WHERE item_type = 'image' AND image_hash = ?1",
            params![hash],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// 按 id 查询单条记录（点击复制用）
    pub fn get_item(&self, id: i64) -> Result<Option<ClipboardItem>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT id, item_type, content, image_path, thumbnail_path, file_paths, file_count, is_favorite, is_pinned, created_at, source_app, image_hash FROM clipboard_items WHERE id = ?1",
            params![id],
            row_to_item,
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// 库内去重：文件按路径列表集合一致
    pub fn find_file(&self, paths: &[String]) -> Result<Option<i64>, String> {
        let mut target: Vec<&String> = paths.iter().collect();
        target.sort();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, file_paths FROM clipboard_items WHERE item_type = 'file'")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))
            .map_err(|e| e.to_string())?;
        for (id, fp) in rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())? {
            if let Some(fp) = fp {
                if let Ok(list) = serde_json::from_str::<Vec<String>>(&fp) {
                    let mut list: Vec<&String> = list.iter().collect();
                    list.sort();
                    if list == target {
                        return Ok(Some(id));
                    }
                }
            }
        }
        Ok(None)
    }

    /// 重复复制时刷新记录时间为当前（拉到列表最顶；置顶恒前不受影响）
    pub fn touch_item(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE clipboard_items SET created_at = ?1 WHERE id = ?2",
            params![
                Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
                id
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 修复迁移后失效的图片/缩略图路径：记录指向的文件不存在，但当前数据目录下有同名文件 → 更新为当前目录路径
    pub fn repair_missing_paths(&self) -> Result<usize, String> {
        let rows: Vec<(i64, Option<String>, Option<String>)> = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare("SELECT id, image_path, thumbnail_path FROM clipboard_items")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        let mut fixed = 0;
        for (id, img, thumb) in rows {
            let new_img = img.as_deref().and_then(|p| {
                if Path::new(p).exists() {
                    None
                } else {
                    Path::new(p)
                        .file_name()
                        .map(|n| self.images_dir.join(n))
                        .filter(|np| np.exists())
                        .map(|np| np.to_string_lossy().to_string())
                }
            });
            let new_thumb = thumb.as_deref().and_then(|p| {
                if Path::new(p).exists() {
                    None
                } else {
                    Path::new(p)
                        .file_name()
                        .map(|n| self.thumbs_dir.join(n))
                        .filter(|np| np.exists())
                        .map(|np| np.to_string_lossy().to_string())
                }
            });
            if new_img.is_some() || new_thumb.is_some() {
                let conn = self.conn.lock().map_err(|e| e.to_string())?;
                conn.execute(
                    "UPDATE clipboard_items SET image_path = ?1, thumbnail_path = ?2 WHERE id = ?3",
                    params![new_img, new_thumb, id],
                )
                .map_err(|e| e.to_string())?;
                fixed += 1;
            }
        }
        Ok(fixed)
    }

    /// 迁移：为旧图片记录补齐 image_hash（图片文件仍存在时重算；不存在则留空）
    pub fn backfill_image_hashes(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let rows: Vec<(i64, Option<String>)> = conn
            .prepare(
                "SELECT id, image_path FROM clipboard_items
                 WHERE item_type = 'image' AND (image_hash IS NULL OR image_hash = '')",
            )
            .map_err(|e| e.to_string())?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut updated = 0;
        for (id, path) in rows {
            let Some(path) = path else { continue };
            if !std::path::Path::new(&path).exists() {
                continue; // 文件已不存在，留空即可
            }
            match image::open(&path) {
                Ok(img) => {
                    let hash = compute_image_hash(&img.to_rgba8());
                    conn.execute(
                        "UPDATE clipboard_items SET image_hash = ?1 WHERE id = ?2",
                        params![hash, id],
                    )
                    .map_err(|e| e.to_string())?;
                    updated += 1;
                }
                Err(e) => log::warn!("重算旧图片指纹失败 {path}: {e}"),
            }
        }
        if updated > 0 {
            log::info!("旧库迁移: 为 {updated} 条旧图片记录补齐 image_hash");
        }
        Ok(updated)
    }

    /// 生成缩略图（最大高度 128px）并更新记录的 thumbnail_path
    pub fn generate_thumbnail(&self, record_id: i64, source_path: &Path) -> Result<Option<String>, String> {
        let dest = self.thumbs_dir.join(format!("thumb_{record_id}.png"));
        let img = image::open(source_path).map_err(|e| format!("读取原图失败 {source_path:?}: {e}"))?;
        let (w, h) = img.dimensions();
        let target_h = 128u32;
        let (nw, nh) = if h > target_h {
            let scale = target_h as f32 / h as f32;
            (((w as f32 * scale).round().max(1.0)) as u32, target_h)
        } else {
            (w, h)
        };
        let thumb = img.resize(nw, nh, image::imageops::FilterType::Triangle);
        thumb.save(&dest).map_err(|e| format!("保存缩略图失败: {e}"))?;
        let path_str = dest.to_string_lossy().to_string();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE clipboard_items SET thumbnail_path = ?1 WHERE id = ?2",
            params![&path_str, record_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(Some(path_str))
    }

    /// 从内存 RGBA 直接生成缩略图（避免重新解码 PNG，大图时省一次解码耗时）
    pub fn generate_thumbnail_from_rgba(&self, record_id: i64, rgba: &image::RgbaImage) -> Result<Option<String>, String> {
        let dest = self.thumbs_dir.join(format!("thumb_{record_id}.png"));
        let (w, h) = rgba.dimensions();
        let target_h = 128u32;
        let (nw, nh) = if h > target_h {
            let scale = target_h as f32 / h as f32;
            (((w as f32 * scale).round().max(1.0)) as u32, target_h)
        } else {
            (w, h)
        };
        let thumb = image::imageops::resize(rgba, nw, nh, image::imageops::FilterType::Triangle);
        thumb.save(&dest).map_err(|e| format!("保存缩略图失败: {e}"))?;
        let path_str = dest.to_string_lossy().to_string();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE clipboard_items SET thumbnail_path = ?1 WHERE id = ?2",
            params![&path_str, record_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(Some(path_str))
    }

    /// 迁移：为缺失缩略图的旧图片记录生成缩略图（原图存在时）
    pub fn backfill_thumbnails(&self) -> Result<usize, String> {
        let rows: Vec<(i64, String)> = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, image_path FROM clipboard_items
                     WHERE item_type = 'image' AND (thumbnail_path IS NULL OR thumbnail_path = '')",
                )
                .map_err(|e| e.to_string())?;
            let collected = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            collected
        }; // conn 锁在此释放，避免下面 generate_thumbnail 再锁时死锁
        let mut updated = 0;
        for (id, path) in rows {
            if path.is_empty() || !Path::new(&path).exists() {
                continue;
            }
            if let Err(e) = self.generate_thumbnail(id, Path::new(&path)) {
                log::warn!("生成缩略图失败 id={id}: {e}");
                continue;
            }
            updated += 1;
        }
        if updated > 0 {
            log::info!("旧库迁移: 为 {updated} 条旧图片记录生成缩略图");
        }
        Ok(updated)
    }

    /// 图片配额：images\ 目录超过 1GB 时，按"最旧且未收藏"清理（收藏永不清理）
    pub fn enforce_image_quota(&self) -> Result<usize, String> {
        const LIMIT_BYTES: u64 = 1024 * 1024 * 1024; // 1GB
        let mut total: u64 = 0;
        if let Ok(rd) = std::fs::read_dir(&self.images_dir) {
            for e in rd.flatten() {
                if let Ok(md) = e.metadata() {
                    total += md.len();
                }
            }
        }
        if total <= LIMIT_BYTES {
            return Ok(0);
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let rows: Vec<(i64, Option<String>, Option<String>)> = conn
            .prepare(
                "SELECT id, image_path, thumbnail_path FROM clipboard_items
                 WHERE item_type = 'image' AND is_favorite = 0 ORDER BY created_at ASC",
            )
            .map_err(|e| e.to_string())?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut deleted = 0;
        for (id, image_path, thumb_path) in rows {
            if total <= LIMIT_BYTES {
                break;
            }
            let size = image_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .unwrap_or(0);
            conn.execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM items_fts WHERE rowid = ?1", params![id])
                .map_err(|e| e.to_string())?;
            for p in [image_path, thumb_path].into_iter().flatten() {
                if let Err(e) = std::fs::remove_file(&p) {
                    log::warn!("配额清理删除文件失败 {p}: {e}");
                }
            }
            total = total.saturating_sub(size);
            deleted += 1;
        }
        if deleted > 0 {
            log::info!("图片配额清理: 删除 {deleted} 条最旧未收藏图片");
        }
        Ok(deleted)
    }

    /// 清空图片抓取缓冲目录（启动时调用，清理上次崩溃残留的临时文件）
    pub fn cleanup_buffer(&self) -> Result<usize, String> {
        let mut deleted = 0;
        if let Ok(rd) = std::fs::read_dir(&self.buffer_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() && std::fs::remove_file(&p).is_ok() {
                    deleted += 1;
                }
            }
        }
        Ok(deleted)
    }

    pub fn toggle_favorite(&self, id: i64) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE clipboard_items SET is_favorite = CASE WHEN is_favorite = 0 THEN 1 ELSE 0 END WHERE id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
        let val: i32 = conn
            .query_row("SELECT is_favorite FROM clipboard_items WHERE id = ?1", params![id], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        Ok(val != 0)
    }

    pub fn toggle_pin(&self, id: i64) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE clipboard_items SET is_pinned = CASE WHEN is_pinned = 0 THEN 1 ELSE 0 END WHERE id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
        let val: i32 = conn
            .query_row("SELECT is_pinned FROM clipboard_items WHERE id = ?1", params![id], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        Ok(val != 0)
    }

    /// 删除记录；图片记录同时清理原图与缩略图文件
    pub fn delete_item(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let image_path: Option<String> = conn
            .query_row("SELECT image_path FROM clipboard_items WHERE id = ?1", params![id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        let thumbnail_path: Option<String> = conn
            .query_row("SELECT thumbnail_path FROM clipboard_items WHERE id = ?1", params![id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        conn.execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM items_fts WHERE rowid = ?1", params![id])
            .map_err(|e| e.to_string())?;
        // 删文件（记录已删，文件清理失败只记日志，不阻塞）
        for path in [image_path, thumbnail_path].into_iter().flatten() {
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!("删除关联文件失败 {path}: {e}");
            }
        }
        Ok(())
    }

    /// 清空全部记录（用户主动"全部删除"）：事务内删记录 + FTS 索引，再删关联的图片原图/缩略图文件
    pub fn clear_all_items(&self) -> Result<usize, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        // 先收集所有关联文件路径（图片原图 + 缩略图），供事务提交后删除
        let files: Vec<(Option<String>, Option<String>)> = conn
            .prepare("SELECT image_path, thumbnail_path FROM clipboard_items")
            .map_err(|e| e.to_string())?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM clipboard_items", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        // 事务内删除记录与 FTS 索引（保持一致）
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM clipboard_items", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM items_fts", [])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        // 删除关联文件（记录已删，文件清理失败仅记日志，不阻塞）
        for (image_path, thumb_path) in &files {
            for p in [image_path, thumb_path].into_iter().flatten() {
                if let Err(e) = std::fs::remove_file(p) {
                    log::warn!("清空删除文件失败 {p}: {e}");
                }
            }
        }
        log::info!("全部删除: 清空 {} 条记录", count);
        Ok(count as usize)
    }

    /// FTS5 全文搜索：文本内容、文件路径、文件名（中文按单字切分，保证子串命中）
    pub fn search_items(&self, query: &str) -> Result<Vec<ClipboardItem>, String> {
        let fts_query = build_fts_query(query);
        if fts_query.is_empty() {
            return self.get_items(false, false);
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let sql = format!(
            "SELECT {ITEM_COLS} FROM clipboard_items
             JOIN items_fts ON items_fts.rowid = clipboard_items.id
             WHERE items_fts MATCH ?1
             ORDER BY clipboard_items.is_pinned DESC, clipboard_items.created_at DESC"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![fts_query], row_to_item)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// 迁移：重建 FTS5 索引（覆盖已有全部记录）
    pub fn backfill_fts(&self) -> Result<usize, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let rows: Vec<(i64, Option<String>, Option<String>)> = {
            let mut stmt = conn
                .prepare("SELECT id, content, file_paths FROM clipboard_items")
                .map_err(|e| e.to_string())?;
            let collected = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            collected
        };
        let total = rows.len();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM items_fts", []).map_err(|e| e.to_string())?;
        for (id, content, paths_json) in rows {
            let paths: Vec<String> = paths_json
                .as_deref()
                .map(|s| serde_json::from_str(s).unwrap_or_default())
                .unwrap_or_default();
            let paths_text = paths.join(" ");
            let file_name = paths
                .iter()
                .filter_map(|p| p.split(['\\', '/']).next_back())
                .collect::<Vec<_>>()
                .join(" ");
            tx.execute(
                "INSERT INTO items_fts(rowid, content, file_paths, file_name) VALUES (?1, ?2, ?3, ?4)",
                params![
                    id,
                    fts_text(content.as_deref().unwrap_or("")),
                    fts_text(&paths_text),
                    fts_text(&file_name),
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        log::info!("FTS 索引已重建（{} 条记录）", total);
        Ok(total)
    }

    /// 清理：删除非收藏且超过保留天数的记录（含关联原图/缩略图）；收藏永不清理。
    /// retention_days = 0 表示用户选择"一直保留"：不做任何自动删除，直接返回
    pub fn cleanup_expired(&self, retention_days: i64) -> Result<usize, String> {
        if retention_days <= 0 {
            return Ok(0);
        }
        let cutoff = (Utc::now() - Duration::days(retention_days))
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        // 先收集要删记录及其关联文件
        let rows: Vec<(i64, Option<String>, Option<String>)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, image_path, thumbnail_path FROM clipboard_items
                     WHERE is_favorite = 0 AND created_at < ?1",
                )
                .map_err(|e| e.to_string())?;
            let collected = stmt
                .query_map(params![cutoff], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            collected
        };
        if rows.is_empty() {
            return Ok(0);
        }
        // 事务内删除记录 + FTS（记录与索引一致）
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (id, _, _) in &rows {
            tx.execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM items_fts WHERE rowid = ?1", params![id])
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        // 删除关联文件（记录已删，文件清理失败仅记日志）
        for (_, image_path, thumb_path) in &rows {
            for p in [image_path, thumb_path].into_iter().flatten() {
                if let Err(e) = std::fs::remove_file(p) {
                    log::warn!("清理删除文件失败 {p}: {e}");
                }
            }
        }
        log::info!("自动清理: 删除 {} 条过期记录", rows.len());
        Ok(rows.len())
    }
}

/// 中文单字切分：CJK 字符间与边界加空格，便于 FTS5 unicode61 单字索引（保证中文子串命中）
fn fts_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.chars().count());
    for c in s.chars() {
        if is_cjk(c) {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push(c);
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2EBEF
    )
}

/// 把查询词预处理为 FTS5 MATCH 表达式（各 token AND 组合，保证子串命中）
fn build_fts_query(q: &str) -> String {
    let processed = fts_text(q);
    let tokens: Vec<&str> = processed.split_whitespace().collect();
    if tokens.is_empty() {
        return String::new();
    }
    tokens
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// 从 file_paths 提取用于 FTS 的路径文本与文件名
fn fts_paths(item: &ClipboardItem) -> (String, String) {
    let paths = item.file_paths.as_deref().unwrap_or(&[]);
    let paths_text = paths.join(" ");
    let file_name = paths
        .iter()
        .filter_map(|p| p.split(['\\', '/']).next_back())
        .collect::<Vec<_>>()
        .join(" ");
    (paths_text, file_name)
}

/// 图片内容指纹（像素 SHA-256）
fn compute_image_hash(rgba: &image::RgbaImage) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(rgba.as_raw());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn row_to_item(row: &Row) -> rusqlite::Result<ClipboardItem> {
    Ok(ClipboardItem {
        id: row.get("id")?,
        item_type: row.get("item_type")?,
        content: row.get("content")?,
        image_path: row.get("image_path")?,
        thumbnail_path: row.get("thumbnail_path")?,
        file_paths: row
            .get::<_, Option<String>>("file_paths")?
            .map(|s| serde_json::from_str(&s).unwrap_or_default()),
        file_count: row.get("file_count")?,
        is_favorite: row.get::<_, i32>("is_favorite")? != 0,
        is_pinned: row.get::<_, i32>("is_pinned")? != 0,
        created_at: row.get("created_at")?,
        source_app: row.get("source_app")?,
        image_hash: row.get("image_hash")?,
    })
}

/// 迁移后：更新新库中图片/缩略图路径（旧数据目录前缀 → 新目录前缀），否则旧记录仍指向旧路径导致失效
pub fn update_migrated_paths(new_dir: &Path, old_dir: &Path) -> Result<usize, String> {
    let db_path = new_dir.join("clipboard.db");
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| format!("打开迁移后数据库失败: {e}"))?;
    let old_prefix = old_dir.to_string_lossy();
    let new_prefix = new_dir.to_string_lossy();
    let n1 = conn
        .execute(
            "UPDATE clipboard_items SET image_path = REPLACE(image_path, ?1, ?2) WHERE image_path LIKE ?1 || '%'",
            params![old_prefix.as_ref(), new_prefix.as_ref()],
        )
        .map_err(|e| format!("更新图片路径失败: {e}"))?;
    let n2 = conn
        .execute(
            "UPDATE clipboard_items SET thumbnail_path = REPLACE(thumbnail_path, ?1, ?2) WHERE thumbnail_path LIKE ?1 || '%'",
            params![old_prefix.as_ref(), new_prefix.as_ref()],
        )
        .map_err(|e| format!("更新缩略图路径失败: {e}"))?;
    log::info!("迁移: 更新路径 image_path {} 条 / thumbnail_path {} 条", n1, n2);
    Ok(n1 + n2)
}

/// 数据迁移辅助：把数据目录内容（clipboard.db + images\ + thumbs\ + logs\）复制到新位置。
/// 复制而非移动——旧库运行中正被占用，只能复制；旧位置待重启后清理（剪切语义）。
/// logs 正在被日志插件写入，复制失败不阻塞迁移（仅告警）。
pub fn copy_data_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目标目录失败: {e}"))?;
    // clipboard.db
    let db_src = src.join("clipboard.db");
    if db_src.exists() {
        std::fs::copy(&db_src, dst.join("clipboard.db"))
            .map_err(|e| format!("复制 clipboard.db 失败: {e}"))?;
    }
    // images\ thumbs\（关键数据，失败即终止）
    for name in ["images", "thumbs"] {
        let s = src.join(name);
        if s.exists() {
            copy_dir_recursive(&s, &dst.join(name))?;
        }
    }
    // logs\（可容忍失败，重启后新位置会重建日志）
    let logs_src = src.join("logs");
    if logs_src.exists() {
        if let Err(e) = copy_dir_recursive(&logs_src, &dst.join("logs")) {
            log::warn!("迁移: 复制 logs 目录失败（可容忍）: {e}");
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目录失败 {dst:?}: {e}"))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读取目录失败 {src:?}: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("复制文件失败 {from:?}: {e}"))?;
        }
    }
    Ok(())
}

/// 校验迁移完整性：源目录的每个文件在目标目录都有对应文件且大小一致。
/// 只单向校验（源 → 目标），不要求目标目录文件数恰好相等——用户可能选了非空目录，
/// 目标里原有文件不应导致误判失败。logs 目录跳过（运行中可容忍复制不完整）。
pub fn verify_data_dir(src: &Path, dst: &Path) -> Result<(), String> {
    let src_files = list_files_with_size(src)?;
    for (sp, ss) in &src_files {
        if rel_is_logs(sp, src) {
            continue;
        }
        let rel = sp.strip_prefix(src).map_err(|e| e.to_string())?;
        // 只校验迁移的数据项：根级仅 clipboard.db；images\ thumbs\ 内文件正常校验。
        // config.json 是固定配置，不随迁移，必须跳过（否则源有而目标无，误判失败）。
        if rel.components().count() == 1 && rel.components().next().unwrap().as_os_str() != "clipboard.db" {
            continue;
        }
        let dp = dst.join(rel);
        let ds = match std::fs::metadata(&dp) {
            Ok(m) => m.len(),
            Err(_) => {
                return Err(format!(
                    "迁移校验失败: 目标缺少文件 {:?}",
                    rel
                ))
            }
        };
        if ds != *ss {
            return Err(format!(
                "迁移校验失败: 文件大小不一致 {:?} 源={} 目标={}",
                rel, ss, ds
            ));
        }
    }
    Ok(())
}

/// 判断文件相对路径是否位于 logs 目录下（迁移校验时跳过 logs）
fn rel_is_logs(path: &Path, base: &Path) -> bool {
    path.strip_prefix(base)
        .map(|r| r.components().next().map(|c| c.as_os_str() == "logs").unwrap_or(false))
        .unwrap_or(false)
}

fn list_files_with_size(dir: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir).map_err(|e| format!("读取目录失败 {dir:?}: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        if p.is_dir() {
            out.extend(list_files_with_size(&p)?);
        } else {
            let size = std::fs::metadata(&p).map_err(|e| e.to_string())?.len();
            out.push((p, size));
        }
    }
    Ok(out)
}

/// 清理旧数据目录的数据项（迁移成功后，重启时调用）。config.json 不在其中，保留。
pub fn cleanup_data_dir(dir: &Path) {
    for name in ["clipboard.db", "images", "thumbs", "logs"] {
        let p = dir.join(name);
        if p.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&p) {
                log::warn!("清理旧数据目录失败 {p:?}: {e}");
            }
        } else if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                log::warn!("清理旧数据文件失败 {p:?}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cmv2_migtest_{name}"));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn copy_and_verify_works_when_dst_has_extra_files() {
        let src = temp_dir("src");
        let dst = temp_dir("dst");
        // 源目录：clipboard.db + images/1.png
        fs::create_dir_all(src.join("images")).unwrap();
        fs::write(src.join("clipboard.db"), vec![1, 2, 3]).unwrap();
        fs::write(src.join("images").join("1.png"), vec![4, 5, 6]).unwrap();
        // 目标目录：预先有一个无关文件（模拟用户选了非空目录）
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("user_notes.txt"), vec![9]).unwrap();

        // 复制
        copy_data_dir(&src, &dst).unwrap();
        // 校验：不应因目标原有文件而失败（回归修复点）
        verify_data_dir(&src, &dst).unwrap();
        // 迁移文件确实存在
        assert!(dst.join("clipboard.db").exists());
        assert!(dst.join("images").join("1.png").exists());

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn verify_detects_missing_file() {
        let src = temp_dir("src2");
        let dst = temp_dir("dst2");
        fs::create_dir_all(src.join("images")).unwrap();
        fs::write(src.join("clipboard.db"), vec![1, 2, 3]).unwrap();
        fs::create_dir_all(&dst).unwrap();
        // 未复制 clipboard.db，校验应失败
        let r = verify_data_dir(&src, &dst);
        assert!(r.is_err(), "缺少文件应校验失败");
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn verify_skips_config_json_in_src() {
        let src = temp_dir("src3");
        let dst = temp_dir("dst3");
        // 源目录：clipboard.db + 根级 config.json（config 不迁移，留在固定位置）
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("clipboard.db"), vec![1, 2, 3]).unwrap();
        fs::write(src.join("config.json"), r#"{"storage_path":"x"}"#).unwrap();
        // 目标目录：只复制了 clipboard.db，没有 config.json（符合迁移语义）
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("clipboard.db"), vec![1, 2, 3]).unwrap();
        // 校验应通过：config.json 不参与校验
        let r = verify_data_dir(&src, &dst);
        assert!(r.is_ok(), "config.json 应被跳过，实际: {r:?}");
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }
}
