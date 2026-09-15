interface Props {
  onCancel: () => void;
  onConfirm: () => void;
}

// "全部删除"确认弹窗：居中模态 + 确定/取消，防误删（内联样式，与设置弹窗一致）
export default function ClearConfirmModal({ onCancel, onConfirm }: Props) {
  return (
    <div
      style={{ position: "fixed", top: 0, left: 0, right: 0, bottom: 0, display: "flex", alignItems: "center", justifyContent: "center", background: "rgba(0,0,0,0.7)", zIndex: 60 }}
      onClick={onCancel}
    >
      <div
        style={{ background: "#1a1a1a", border: "2px solid #4a4a4a", borderRadius: "8px", padding: "20px", width: "300px", boxShadow: "0 10px 25px rgba(0,0,0,0.5)" }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 style={{ fontSize: 16, fontWeight: 500, marginBottom: 12, color: "#ffffff" }}>确认删除</h2>
        <p style={{ fontSize: 14, color: "#f0f0f0", marginBottom: 20, lineHeight: 1.5 }}>
          你真的要全部删除复制记录吗？
        </p>
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          <button
            onClick={onCancel}
            style={{ padding: "6px 16px", fontSize: 14, color: "#f0f0f0", borderRadius: "4px", background: "transparent", border: "1px solid #2a2a2a", cursor: "pointer" }}
          >
            取消
          </button>
          <button
            onClick={onConfirm}
            style={{ padding: "6px 16px", fontSize: 14, background: "#e05555", color: "#ffffff", borderRadius: "4px", border: "none", cursor: "pointer" }}
          >
            确定
          </button>
        </div>
      </div>
    </div>
  );
}
