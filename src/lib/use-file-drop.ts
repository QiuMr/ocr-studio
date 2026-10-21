import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "./tauri";
import { imageSize } from "./api";

/** 拖入并已解码好的一张图片。 */
export interface DroppedImage {
  /** 文件名（不含路径），用于提示 */
  name: string;
  /** 原始 base64，不含 data URL 前缀 */
  b64: string;
  /** 可直接给 <img src> 的地址 */
  url: string;
  width: number;
  height: number;
}

const IMAGE_EXT = /\.(png|jpe?g|webp|bmp|gif)$/i;

/**
 * 全窗口拖拽接收图片。
 *
 * 为什么不能只用 HTML5 的 `drop` 事件：Tauri 2 默认把文件拖放接管成原生事件
 * （`dragDropEnabled` 默认 true），DOM 的 `dragover` / `drop` **一次都不会触发**，
 * 必须监听 `onDragDropEvent` 拿真实路径，再让后端读文件。
 * 纯浏览器调试时没有这条路，回退到 HTML5。
 *
 * 两边的回调格式统一成 `DroppedImage`，页面代码不用判断环境。
 */
export function useImageDrop(onImages: (images: DroppedImage[]) => void) {
  const [dragging, setDragging] = useState(false);
  // 回调放 ref：页面每次渲染换一个新函数不该导致重新注册监听
  const cbRef = useRef(onImages);
  cbRef.current = onImages;

  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: (() => void) | undefined;
    let disposed = false;

    void (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const off = await getCurrentWebview().onDragDropEvent(async (event) => {
          const p = event.payload;
          if (p.type === "over" || p.type === "enter") {
            setDragging(true);
            return;
          }
          if (p.type === "leave") {
            setDragging(false);
            return;
          }
          if (p.type !== "drop") return;
          setDragging(false);

          const paths = p.paths.filter((path) => IMAGE_EXT.test(path));
          if (paths.length === 0) return;

          const loaded: DroppedImage[] = [];
          for (const path of paths.slice(0, 8)) {
            try {
              const b64 = await invoke<string>("read_image_file", { path });
              const url = `data:image/png;base64,${b64}`;
              const dim = await imageSize(url);
              loaded.push({
                name: path.split(/[\\/]/).pop() ?? path,
                b64,
                url,
                width: dim.width,
                height: dim.height,
              });
            } catch {
              /* 单张失败不影响其它，后端日志里有原因 */
            }
          }
          if (loaded.length > 0) cbRef.current(loaded);
        });
        if (disposed) off();
        else unlisten = off;
      } catch {
        /* 拿不到 webview 就当作不支持拖拽，不阻塞页面 */
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // 浏览器降级：用 DOM 拖放
  useEffect(() => {
    if (isTauri()) return;

    let depth = 0;
    const hasFiles = (e: DragEvent) =>
      Array.from(e.dataTransfer?.types ?? []).includes("Files");

    const onEnter = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth += 1;
      setDragging(true);
    };
    const onOver = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault(); // 不阻止的话浏览器会直接把图片当页面打开
    };
    const onLeave = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      depth = Math.max(0, depth - 1);
      if (depth === 0) setDragging(false);
    };
    const onDrop = async (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth = 0;
      setDragging(false);
      const files = Array.from(e.dataTransfer?.files ?? []).filter((f) =>
        f.type.startsWith("image/"),
      );
      if (files.length === 0) return;

      const loaded: DroppedImage[] = [];
      for (const f of files.slice(0, 8)) {
        const buf = await f.arrayBuffer();
        // FileReader 更省事：直接拿 data URL 再切掉前缀
        const b64 = await new Promise<string>((resolve, reject) => {
          const r = new FileReader();
          r.onload = () => {
            const s = String(r.result);
            const i = s.indexOf(",");
            resolve(i >= 0 ? s.slice(i + 1) : s);
          };
          r.onerror = () => reject(r.error);
          r.readAsDataURL(new Blob([buf], { type: f.type }));
        });
        const url = `data:${f.type || "image/png"};base64,${b64}`;
        const dim = await imageSize(url);
        loaded.push({ name: f.name, b64, url, width: dim.width, height: dim.height });
      }
      if (loaded.length > 0) cbRef.current(loaded);
    };

    window.addEventListener("dragenter", onEnter);
    window.addEventListener("dragover", onOver);
    window.addEventListener("dragleave", onLeave);
    window.addEventListener("drop", onDrop);
    return () => {
      window.removeEventListener("dragenter", onEnter);
      window.removeEventListener("dragover", onOver);
      window.removeEventListener("dragleave", onLeave);
      window.removeEventListener("drop", onDrop);
    };
  }, []);

  return { dragging };
}
