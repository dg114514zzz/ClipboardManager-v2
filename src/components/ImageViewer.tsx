import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  path: string; // 原图绝对路径
  onClose: () => void;
}

/**
 * 大图查看器：全屏覆盖层展示原图（基础版——图片自适应窗口完整显示，不做缩放平移）。
 * 关闭方式：Esc 键 / 点击图片外围区域 / 右上角关闭按钮。
 * 用原图而非缩略图：查看器存在的意义就是看清细节，缩略图放大只有马赛克。
 */
export default function ImageViewer({ path, onClose }: Props) {
  const [src, setSrc] = useState("");
  const [failed, setFailed] = useState(false);

  // 加载原图（base64 data URI）；大图传输需几百毫秒，期间显示"加载中"
  useEffect(() => {
    let cancelled = false;
    setSrc("");
    setFailed(false);
    invoke<string>("get_image_base64", { path })
      .then((data) => {
        if (!cancelled) setSrc(data);
      })
      .catch((e) => {
        console.error("原图加载失败", e);
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      onClick={onClose}
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        zIndex: 200,
        background: "rgba(0, 0, 0, 0.85)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: 28,
      }}
    >
      <button
        onClick={onClose}
        title="关闭（Esc）"
        style={{
          position: "absolute",
          top: 8,
          right: 12,
          width: 28,
          height: 28,
          border: "none",
          borderRadius: 4,
          background: "rgba(255, 255, 255, 0.16)",
          color: "#fff",
          fontSize: 18,
          lineHeight: "24px",
          cursor: "pointer",
        }}
      >
        ×
      </button>

      {failed ? (
        <span style={{ color: "#ddd", fontSize: 14 }}>图片加载失败</span>
      ) : src ? (
        <img
          src={src}
          alt=""
          onClick={(e) => e.stopPropagation()} // 点图片本身不关闭
          style={{ maxWidth: "100%", maxHeight: "100%", display: "block" }}
        />
      ) : (
        <span style={{ color: "#ddd", fontSize: 14 }}>加载中…</span>
      )}
    </div>
  );
}
