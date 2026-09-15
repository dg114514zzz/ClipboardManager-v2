import type { ReactNode } from "react";
import type { ClipboardItem } from "../types";
import { formatTime, imageTitle } from "../lib/format";
import { fileLabel } from "../lib/mock";
import Thumbnail from "./Thumbnail";
import { DocIcon, FileIcon, ImageIcon, LocateIcon, PinIcon, StarIcon, TrashIcon } from "./Icons";

function ActionBtn({
  title,
  onClick,
  disabled,
  children,
}: {
  title: string;
  onClick?: () => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      onClick={() => onClick?.()}
      title={title}
      disabled={disabled}
      className={`p-1 rounded hover:bg-bg-active ${disabled ? "opacity-40 cursor-not-allowed" : ""}`}
    >
      {children}
    </button>
  );
}

interface Props {
  item: ClipboardItem;
  invalid: boolean;
  multiSelect: boolean;
  selectIndex: number; // 选择顺序（1 起）；0 = 未选
  onToggleSelect: (item: ClipboardItem) => void;
  onCopy: (id: number) => void;
  onFavorite: (id: number) => void;
  onPin: (id: number) => void;
  onDelete: (id: number) => void;
  onReveal: (paths: string[]) => void;
}

export default function ClipboardItemRow({ item, invalid, multiSelect, selectIndex, onToggleSelect, onCopy, onFavorite, onPin, onDelete, onReveal }: Props) {
  return (
    <div
      onClick={() => (multiSelect ? onToggleSelect(item) : onCopy(item.id))}
      style={{ borderBottom: "2px solid #4a4a4a" }}
      className={`group flex items-start gap-3 px-4 py-3 hover:bg-bg-secondary transition-colors cursor-pointer ${
        item.is_pinned ? "bg-bg-active/50 border-l-2 border-l-accent" : ""
      } ${multiSelect && selectIndex > 0 ? "bg-bg-active/50" : ""}`}
    >
      {/* 多选模式：序号勾选框（选中蓝底白字，未选中空圈）；否则左侧类型图标（置顶优先蓝色图钉） */}
      {multiSelect ? (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onToggleSelect(item);
          }}
          className={`w-5 h-5 rounded-full border flex items-center justify-center text-xs font-medium flex-shrink-0 mt-0.5 transition-colors ${
            selectIndex > 0
              ? "bg-accent border-accent text-white"
              : "border-text-muted text-text-muted hover:border-accent"
          }`}
        >
          {selectIndex > 0 ? selectIndex : ""}
        </button>
      ) : (
        <div className="mt-0.5 text-text-muted flex-shrink-0">
          {item.is_pinned ? (
            <PinIcon active />
          ) : item.item_type === "image" ? (
            <ImageIcon />
          ) : item.item_type === "file" ? (
            <FileIcon />
          ) : (
            <DocIcon />
          )}
        </div>
      )}

      {/* 主体 */}
      <div className="flex-1 min-w-0">
        {item.item_type === "image" ? (
          <>
            <Thumbnail thumbPath={item.thumbnail_path} />
            <p className="text-sm text-text-primary mt-1 truncate">{imageTitle(item.source_app, item.created_at)}</p>
          </>
        ) : item.item_type === "file" ? (
          <>
            <p
              className={`text-sm break-words line-clamp-3 ${
                invalid ? "text-text-muted line-through" : "text-text-primary"
              }`}
            >
              {fileLabel(item)}
            </p>
            {invalid ? (
              <p className="text-xs text-text-muted mt-1 line-through">路径已被删除或修改</p>
            ) : (
              <p className="text-xs text-text-muted mt-1 truncate">{item.file_paths?.[0] ?? ""}</p>
            )}
          </>
        ) : (
          <p className="text-sm text-text-primary whitespace-pre-wrap break-words line-clamp-3">{item.content ?? ""}</p>
        )}
        <p className="text-xs text-text-muted mt-1">{formatTime(item.created_at)}</p>
      </div>

      {/* 右侧操作：定位（文件）+ 收藏 / 置顶 / 删除；多选模式下隐藏，避免与勾选混淆（文档 6.3） */}
      {!multiSelect && (
      <div
        className="flex items-center gap-0.5 flex-shrink-0 opacity-0 group-hover:opacity-100 transition-opacity"
        onClick={(e) => e.stopPropagation()}
      >
        {item.item_type === "file" && (
          <ActionBtn title="定位" disabled={invalid} onClick={() => onReveal(item.file_paths ?? [])}>
            <LocateIcon />
          </ActionBtn>
        )}
        <ActionBtn title={item.is_favorite ? "取消收藏" : "收藏"} onClick={() => onFavorite(item.id)}>
          <StarIcon active={item.is_favorite} />
        </ActionBtn>
        <ActionBtn title={item.is_pinned ? "取消置顶" : "置顶"} onClick={() => onPin(item.id)}>
          <PinIcon active={item.is_pinned} />
        </ActionBtn>
        <ActionBtn title="删除" onClick={() => onDelete(item.id)}>
          <TrashIcon />
        </ActionBtn>
      </div>
      )}
    </div>
  );
}
