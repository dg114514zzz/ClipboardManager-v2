// 具体时间：2026-08-26 17:27（本地时区，直接展示数据库存的时间，不做相对换算）
export function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${y}-${m}-${day} ${hh}:${mm}`;
}

// 图片记录主标题（文档 6.2.1）：
// 文件来源（source_app 有原始路径）→ 原始文件名；像素来源 → "图片 · 具体时间"
export function imageTitle(sourceApp: string | null, createdAt: string): string {
  if (sourceApp) {
    const name = sourceApp.split(/[\\/]/).pop() ?? sourceApp;
    return name;
  }
  return `图片 · ${formatTime(createdAt)}`;
}
