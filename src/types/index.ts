// 数据模型，与需求文档「7.1 表 clipboard_items」「7.2 表 settings」对应
export type ItemType = "text" | "image" | "file";
export type Tab = "all" | "favorites";

export interface ClipboardItem {
  id: number;
  item_type: ItemType;
  content: string | null;
  image_path: string | null; // 图片原图 PNG 路径
  thumbnail_path: string | null; // 图片缩略图路径
  file_paths: string[] | null; // 文件路径列表（JSON 数组）
  file_count: number;
  is_favorite: boolean;
  is_pinned: boolean;
  created_at: string; // ISO 8601
  source_app: string | null;
}

export interface Settings {
  storage_path: string;
  retention_days: number;
  auto_start: boolean;
}
