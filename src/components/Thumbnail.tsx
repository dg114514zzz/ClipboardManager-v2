import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ImageIcon } from "./Icons";

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

export default function Thumbnail({ thumbPath }: { thumbPath: string | null }) {
  const [state, setState] = useState<LoadState>("idle");
  const [src, setSrc] = useState("");
  const [attempt, setAttempt] = useState(0);
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
    <div ref={containerRef} className="min-h-4">
      {state === "loaded" ? (
        <img src={src} alt="" className="max-h-32 max-w-full rounded object-contain bg-bg-primary" />
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
