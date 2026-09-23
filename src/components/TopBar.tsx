import { FilterIcon, GearIcon, SearchIcon, TrashIcon } from "./Icons";

interface Props {
  search: string;
  onSearch: (v: string) => void;
  multiSelect: boolean;
  selectedCount: number;
  onToggleMultiSelect: () => void;
  onOpenSettings: () => void;
  onOpenFilter: () => void;
  filterActive: boolean; // 筛选生效中（非"全部"）时图标高亮，避免用户困惑"记录怎么少了"
  onClearAll: () => void;
}

export default function TopBar({ search, onSearch, multiSelect, selectedCount, onToggleMultiSelect, onOpenSettings, onOpenFilter, filterActive, onClearAll }: Props) {
  return (
    <header className="flex items-center gap-3 px-4 py-3 border-b border-border bg-bg-primary flex-shrink-0">
      {/* 搜索框（占主） */}
      <div className="relative flex-1">
        <span className="absolute left-3 top-1/2 -translate-y-1/2 text-text-muted">
          <SearchIcon />
        </span>
        <input
          value={search}
          onChange={(e) => onSearch(e.target.value)}
          placeholder="搜索复制的历史..."
          className="w-full bg-bg-secondary text-text-primary pl-9 pr-3 py-2 rounded-md border border-border outline-none focus:border-accent placeholder:text-text-muted text-sm"
        />
      </div>

      {/* 多选按钮：进入多选；多选模式下变为"完成(N)"（再点=批量复制并退出，文档 6.3） */}
      <button
        onClick={onToggleMultiSelect}
        title={multiSelect ? "完成批量复制" : "进入多选"}
        className={`px-3 py-1.5 text-sm rounded-md border transition-colors ${
          multiSelect
            ? "bg-accent text-white border-accent"
            : "text-text-secondary border-border hover:text-text-primary hover:bg-bg-hover"
        }`}
      >
        {multiSelect ? `完成(${selectedCount})` : "多选"}
      </button>

      {/* 全部删除 */}
      <button
        onClick={onClearAll}
        title="清空全部记录"
        className="p-1.5 rounded-md hover:bg-bg-hover text-text-secondary hover:text-text-primary"
      >
        <TrashIcon />
      </button>

      {/* 筛选：按记录类型过滤；生效中时图标变蓝提示 */}
      <button
        onClick={onOpenFilter}
        title={filterActive ? "筛选（已生效）" : "筛选"}
        className="p-1.5 rounded-md hover:bg-bg-hover text-text-secondary hover:text-text-primary"
        style={{ color: filterActive ? "#4a9eff" : undefined }}
      >
        <FilterIcon />
      </button>

      {/* 设置齿轮 */}
      <button
        onClick={onOpenSettings}
        title="设置"
        className="p-1.5 rounded-md hover:bg-bg-hover text-text-secondary hover:text-text-primary"
      >
        <GearIcon />
      </button>
    </header>
  );
}
