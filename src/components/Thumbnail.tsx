import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ExpandIcon, ImageIcon } from "./Icons";

// 全局并发控制：限制同时进行的缩略图请求数（缺陷 #6：一次性并发全部图片）
const MAX_CONCURRENT = 4;
let activeCount = 0;
const waitQueue: Array<() => void> = [];

function acquire(): Promise<() => void> {
  return new Promise((resolve) => {
    const tryAcquire = () => {
      if (activeCount < MAX_CONCURRENT) {
        activeCount++;
        let released = false;
        resolve(() => {
          if (released) return;
          released = true;
          activeCount--;
          const next = waitQueue.shift();
          next?.();
        });
      } else {
        waitQueue.push(tryAcquire);
      }
    };
    tryAcquire();
  });
}

type LoadState = "idle" | "loading" | "loaded" | "error";

interface Props {
  thumbPath: string | null;
  imagePath?: string | null; // 原图路径（查看大图用）
  onOpen?: (path: string) => void; // 未传则不显示"查看大图"按钮（多选模式下不传）
}

export default function Thumbnail({ thumbPath, imagePath, onOpen }: Props) {
  const [state, setState] = useState<LoadState>("idle");
  const [src, setSrc] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [hover, setHover] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!thumbPath) return;
    const el = containerRef.current;
    if (!el) return;
    let cancelled = false;
    let releaseFn: (() => void) | undefined;

    // 懒加载：只加载进入可见区域的缩略图（缺陷 #6）
    const io = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          io.disconnect();
          setState("loading");
          acquire().then((release) => {
            releaseFn = release;
            if (cancelled) {
              release();
              return;
            }
            invoke<string>("get_thumbnail_base64", { path: thumbPath })
              .then((data) => {
                if (cancelled) return;
                setSrc(data);
                setState("loaded");
              })
              .catch((e) => {
                console.error("缩略图加载失败", e);
                if (!cancelled) setState("error"); // 失败可重试（缺陷 #5）
              })
              .finally(() => releaseFn?.());
          });
        }
      },
      { rootMargin: "120px" },
    );
    io.observe(el);
    return () => {
      cancelled = true;
      releaseFn?.();
      io.disconnect();
    };
  }, [thumbPath, attempt]);

  if (!thumbPath) {
    return (
      <div className="flex items-center gap-2 text-text-secondary text-sm">
        <ImageIcon size={16} />
        <span>图片</span>
      </div>
    );
  }

  return (
    // width: fit-content 让容器收缩到图片实际尺寸，右上角按钮才能贴着图片而不是整个行宽；
    // maxWidth: 100% 是必需的——宽图的缩略图（如 921×82 原样保留，宽 921px）会超出窗口宽度，
    // 此时容器被父级限制而图片溢出，两者右边缘不一致会导致按钮跑到图片中间（表现为"没有按钮"）
    <div
      ref={containerRef}
      className="min-h-4"
      style={{ position: "relative", width: "fit-content", maxWidth: "100%" }}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      {state === "loaded" ? (
        <>
          <img
            src={src}
            alt=""
            className="max-h-32 max-w-full rounded object-contain bg-bg-primary"
            // 细边框：防止浅色/白色图片与背景融为一体。半透明中性灰在浅色与深色背景下均可见，
            // 且不干扰图片本身的内容判断。
            // maxWidth 用内联样式补上：max-w-full 类名在无 CSS 环境下不生效，
            // 导致宽缩略图溢出容器、右上角按钮错位（用内联样式，项目未加载 CSS 文件）
            style={{ border: "2px solid rgba(128, 128, 128, 0.7)", maxWidth: "100%" }}
          />
          {hover && onOpen && imagePath && (
            <button
              onClick={(e) => {
                e.stopPropagation(); // 阻止触发行点击（复制）
                onOpen(imagePath);
              }}
              title="查看大图"
              style={{
                position: "absolute",
                top: 4,
                right: 4,
                width: 24,
                height: 24,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                padding: 0,
                border: "none",
                borderRadius: 4,
                background: "rgba(0, 0, 0, 0.55)",
                color: "#fff",
                cursor: "pointer",
              }}
            >
              <ExpandIcon />
            </button>
          )}
        </>
      ) : state === "error" ? (
        <button
          onClick={() => {
            setState("loading");
            setAttempt((n) => n + 1);
          }}
          className="text-danger text-sm hover:underline"
        >
          加载失败
        </button>
      ) : (
        <span className="text-text-muted text-sm">加载中...</span>
      )}
    </div>
  );
}
