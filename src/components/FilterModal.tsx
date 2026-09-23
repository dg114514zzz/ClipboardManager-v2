import { useEffect, type CSSProperties } from "react";
import type { ItemType } from "../types";
import { CheckIcon } from "./Icons";

const OPTIONS: { value: ItemType; label: string }[] = [
  { value: "text", label: "文本" },
  { value: "image", label: "图片" },
  { value: "file", label: "文件" },
];

// 最多同时勾选两项：三项全勾等于不筛选，与"一个都不勾"语义重复
const MAX_SELECTED = 2;

interface Props {
  selected: ItemType[]; // 已勾选类型；空数组 = 显示全部
  onToggle: (v: ItemType) => void;
  onReset: () => void;
  onClose: () => void;
}

/**
 * 筛选弹窗：勾选要显示的类型（最多两项），样式与设置弹窗保持一致。
 * 勾选即时生效：空选 = 显示全部；勾一项 = 只看该类型；勾两项 = 看这两类（等于筛掉第三类）。
 */
export default function FilterModal({ selected, onToggle, onReset, onClose }: Props) {
  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const atLimit = selected.length >= MAX_SELECTED;

  const optionStyle: CSSProperties = {
    display: "flex",
    alignItems: "center",
    gap: 8,
    fontSize: 14,
    color: "#f0f0f0",
    cursor: "pointer",
    marginBottom: 14,
  };

  return (
    <div
      style={{ position: "fixed", top: 0, left: 0, right: 0, bottom: 0, display: "flex", alignItems: "center", justifyContent: "center", background: "rgba(0,0,0,0.7)", zIndex: 50 }}
      onClick={onClose}
    >
      <div
        style={{ background: "#1a1a1a", border: "2px solid #4a4a4a", borderRadius: "8px", padding: "20px", width: "320px", boxShadow: "0 10px 25px rgba(0,0,0,0.5)", color: "#f0f0f0" }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 style={{ fontSize: 16, fontWeight: 500, marginBottom: 6, color: "#ffffff" }}>筛选</h2>
        <p style={{ fontSize: 12, color: "#999", marginBottom: 14, lineHeight: 1.4 }}>
          勾选要显示的类型，最多两项；都不勾则显示全部
        </p>

        {/* 多选：勾选即时生效。已达上限时，未勾选项不可点 */}
        {OPTIONS.map((o) => {
          const checked = selected.includes(o.value);
          const disabled = atLimit && !checked;
          return (
            <label
              key={o.value}
              style={{ ...optionStyle, cursor: disabled ? "not-allowed" : "pointer", opacity: disabled ? 0.45 : 1 }}
              onClick={() => {
                if (disabled) return;
                onToggle(o.value);
              }}
            >
              <div
                style={{
                  width: 16,
                  height: 16,
                  borderRadius: 4,
                  border: checked ? "1px solid #4a9eff" : "1px solid #555",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  background: checked ? "#4a9eff" : "transparent",
                }}
              >
                {checked && <CheckIcon />}
              </div>
              {o.label}
            </label>
          );
        })}

        {atLimit && (
          <p style={{ fontSize: 12, color: "#e0b000", marginTop: -4, marginBottom: 12, lineHeight: 1.4 }}>
            已选满两项，需先取消一项才能选其他类型
          </p>
        )}

        {/* 恢复默认（清空勾选 = 显示全部） / 关闭 */}
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 4 }}>
          <button
            onClick={onReset}
            style={{ padding: "6px 16px", fontSize: 14, color: "#f0f0f0", borderRadius: "4px", background: "transparent", border: "1px solid #2a2a2a", cursor: "pointer" }}
          >
            恢复默认
          </button>
          <button
            onClick={onClose}
            style={{ padding: "6px 16px", fontSize: 14, background: "#4a9eff", color: "#ffffff", borderRadius: "4px", border: "none", cursor: "pointer" }}
          >
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
