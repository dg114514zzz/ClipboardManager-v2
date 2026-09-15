import { ClipboardOutlineIcon } from "./Icons";

export default function EmptyState() {
  return (
    <div className="flex flex-col items-center justify-center h-full text-text-muted gap-2">
      <ClipboardOutlineIcon />
      <p className="text-sm">暂无记录，去复制点文字或图片吧</p>
    </div>
  );
}
