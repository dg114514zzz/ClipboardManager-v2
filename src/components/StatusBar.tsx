export default function StatusBar({ count }: { count: number }) {
  return (
    <div className="px-4 py-1.5 border-t border-border text-xs text-text-muted flex items-center justify-between flex-shrink-0">
      <span>共 {count} 条记录</span>
      <span>Alt+Shift+V 切换窗口</span>
    </div>
  );
}
