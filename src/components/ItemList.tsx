import type { ClipboardItem } from "../types";
import ClipboardItemRow from "./ClipboardItemRow";
import EmptyState from "./EmptyState";

interface Props {
  items: ClipboardItem[];
  invalidIds: Set<number>;
  multiSelect: boolean;
  selectIndex: Map<number, number>; // id → 选择顺序（1 起），未选不在映射
  onToggleSelect: (item: ClipboardItem) => void;
  onCopy: (id: number) => void;
  onFavorite: (id: number) => void;
  onPin: (id: number) => void;
  onDelete: (id: number) => void;
  onReveal: (paths: string[]) => void;
  onOpenImage: (path: string) => void;
}

export default function ItemList({ items, invalidIds, multiSelect, selectIndex, onToggleSelect, onCopy, onFavorite, onPin, onDelete, onReveal, onOpenImage }: Props) {
  if (items.length === 0) return <EmptyState />;
  return (
    <div className="h-full overflow-y-auto">
      {items.map((item) => (
        <ClipboardItemRow
          key={item.id}
          item={item}
          invalid={invalidIds.has(item.id)}
          multiSelect={multiSelect}
          selectIndex={selectIndex.get(item.id) ?? 0}
          onToggleSelect={onToggleSelect}
          onCopy={onCopy}
          onFavorite={onFavorite}
          onPin={onPin}
          onDelete={onDelete}
          onReveal={onReveal}
          onOpenImage={onOpenImage}
        />
      ))}
    </div>
  );
}
