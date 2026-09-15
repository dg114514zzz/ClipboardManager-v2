// 文件记录主标题：单文件 → 文件名；多文件 → "xxx 等N个文件"
// 注：第 1 步的 mock 数据（MOCK_ITEMS）在第 2 步接入真实数据后已移除
import type { ClipboardItem } from "../types";

export function fileLabel(item: ClipboardItem): string {
  const paths = item.file_paths ?? [];
  const first = paths[0] ?? "";
  const name = first.split(/[\\/]/).pop() ?? first;
  return paths.length > 1 ? `${name} 等${paths.length}个文件` : name;
}
