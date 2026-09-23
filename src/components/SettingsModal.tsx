import { useState, type CSSProperties } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { Settings } from "../types";
import { CheckIcon } from "./Icons";

// 保留天数选项；0 = 一直保留（不做任何自动清理，含图片 1GB 配额）
const RETENTION_OPTIONS = [1, 3, 5, 7, 10, 15, 30, 0];

interface Props {
  settings: Settings;
  onCancel: () => void;
  onSave: (s: Settings) => void;
  onRequestMove: (path: string) => void; // 选择新数据目录后回调
}

export default function SettingsModal({ settings, onCancel, onSave, onRequestMove }: Props) {
  const [draft, setDraft] = useState(settings);

  // 点"更改"→ 弹目录选择框 → 选中后更新 draft.storage_path 供"保存"时迁移
  const pickStorageDir = async () => {
    try {
      const dir = await open({ directory: true, title: "选择数据保存位置" });
      if (typeof dir === "string" && dir.trim()) {
        setDraft({ ...draft, storage_path: dir });
        onRequestMove(dir); // 告知父组件已选新位置（父组件可在"保存"时迁移）
      }
    } catch (e) {
      console.error("选择目录失败", e);
    }
  };

  const labelStyle: CSSProperties = {
    display: "block",
    fontSize: 14,
    color: "#f0f0f0",
    marginBottom: 6,
  };

  return (
    <div
      style={{ position: "fixed", top: 0, left: 0, right: 0, bottom: 0, display: "flex", alignItems: "center", justifyContent: "center", background: "rgba(0,0,0,0.7)", zIndex: 50 }}
      onClick={onCancel}
    >
      <div
        style={{ background: "#1a1a1a", border: "2px solid #4a4a4a", borderRadius: "8px", padding: "20px", width: "320px", boxShadow: "0 10px 25px rgba(0,0,0,0.5)", color: "#f0f0f0" }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 style={{ fontSize: 16, fontWeight: 500, marginBottom: 16, color: "#ffffff" }}>设置</h2>

        {/* 保留天数 */}
        <label style={labelStyle}>保留天数</label>
        <select
          value={draft.retention_days}
          onChange={(e) => setDraft({ ...draft, retention_days: Number(e.target.value) })}
          style={{ width: "100%", background: "#0d0d0d", color: "#f0f0f0", padding: "6px 12px", borderRadius: "4px", border: "1px solid #2a2a2a", outline: "none", marginBottom: 16, fontSize: 14 }}
        >
          {RETENTION_OPTIONS.map((d) => (
            <option key={d} value={d} style={{ color: "#f0f0f0", background: "#0d0d0d" }}>
              {d === 0 ? "一直保留（不自动删除）" : `${d} 天`}
            </option>
          ))}
        </select>

        {/* 开机自启（自绘开关） */}
        <label
          style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 14, color: "#f0f0f0", cursor: "pointer", marginBottom: 16 }}
          onClick={() => setDraft({ ...draft, auto_start: !draft.auto_start })}
        >
          <div
            style={{ width: 16, height: 16, borderRadius: 4, border: draft.auto_start ? "1px solid #4a9eff" : "1px solid #555", display: "flex", alignItems: "center", justifyContent: "center", background: draft.auto_start ? "#4a9eff" : "transparent" }}
          >
            {draft.auto_start && <CheckIcon />}
          </div>
          开机自启
        </label>

        {/* 数据保存位置 */}
        <label style={labelStyle}>数据保存位置</label>
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 20 }}>
          <span style={{ flex: 1, fontSize: 12, color: "#f0f0f0", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }} title={draft.storage_path}>
            {draft.storage_path}
          </span>
          <button
            onClick={pickStorageDir}
            style={{ fontSize: 12, color: "#4a9eff", border: "1px solid #2a2a2a", borderRadius: "4px", padding: "4px 8px", background: "transparent", cursor: "pointer" }}
          >
            更改
          </button>
        </div>
        {draft.storage_path !== settings.storage_path && (
          <p style={{ fontSize: 12, color: "#e0b000", marginBottom: 16, lineHeight: 1.4 }}>
            保存后将把数据迁移到新位置（程序会自动重启）
          </p>
        )}

        {/* 取消 / 保存 */}
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          <button
            onClick={onCancel}
            style={{ padding: "6px 16px", fontSize: 14, color: "#f0f0f0", borderRadius: "4px", background: "transparent", border: "none", cursor: "pointer" }}
          >
            取消
          </button>
          <button
            onClick={() => onSave(draft)}
            style={{ padding: "6px 16px", fontSize: 14, background: "#4a9eff", color: "#ffffff", borderRadius: "4px", border: "none", cursor: "pointer" }}
          >
            保存
          </button>
        </div>
      </div>
    </div>
  );
}
