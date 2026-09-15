import type { Tab } from "../types";
import { StarIcon } from "./Icons";

interface Props {
  tab: Tab;
  onTab: (t: Tab) => void;
  sortAsc: boolean;
  onToggleSort: () => void;
}

export default function TabBar({ tab, onTab, sortAsc, onToggleSort }: Props) {
  return (
    <div className="flex items-center justify-between px-4 py-2 border-b border-border flex-shrink-0">
      <div className="flex gap-2">
        <button
          onClick={() => onTab("all")}
          className={`px-4 py-1.5 rounded text-sm ${
            tab === "all" ? "bg-accent text-white" : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
          }`}
        >
          全部
        </button>
        <button
          onClick={() => onTab("favorites")}
          className={`px-4 py-1.5 rounded text-sm flex items-center gap-1.5 ${
            tab === "favorites" ? "bg-accent text-white" : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
          }`}
        >
          <StarIcon active={tab === "favorites"} />
          收藏
        </button>
      </div>
      <button
        onClick={onToggleSort}
        className="text-xs text-text-secondary hover:text-text-primary px-2 py-1 rounded hover:bg-bg-hover"
      >
        {sortAsc ? "↑ 旧 → 新" : "↓ 新 → 旧"}
      </button>
    </div>
  );
}
