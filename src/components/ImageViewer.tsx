import { useEffect, useRef } from "react";
import type { Box4, FindHit, LineResult } from "../types";

interface Props {
  /** 已解码的原图（data URL / object URL / http URL 均可） */
  src: string | null;
  width: number;
  height: number;
  lines: LineResult[];
  /** 是否叠加显示检测框 */
  showBoxes: boolean;
  /** 找图命中框（与识别框用不同颜色区分） */
  hits?: FindHit[];
  /** 找图命中坐标是否已经是图内坐标；`screen`/`window` 来源需要减去截图偏移 */
  hitsOffset?: { x: number; y: number };
}

/**
 * 图像 + 检测框叠加显示。
 *
 * 用 canvas 而不是绝对定位 div：框坐标是原图像素坐标，canvas 可以按同一比例
 * 缩放，定位天然一致；div 方案要自己算 CSS 缩放，容易在 devicePixelRatio 上跑偏。
 */
export function ImageViewer({
  src,
  width,
  height,
  lines,
  showBoxes,
  hits,
  hitsOffset,
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);

  // 重绘：图换、框变、开关变都要重画
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    if (!src || width === 0 || height === 0) {
      canvas.width = 0;
      canvas.height = 0;
      ctx.clearRect(0, 0, 0, 0);
      return;
    }

    const img = new Image();
    img.onload = () => {
      imgRef.current = img;
      canvas.width = width;
      canvas.height = height;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(img, 0, 0, width, height);

      if (!showBoxes) return;
      lines.forEach((line, i) => {
        drawBox(ctx, line.box, i + 1);
      });

      const ox = hitsOffset?.x ?? 0;
      const oy = hitsOffset?.y ?? 0;
      (hits ?? []).forEach((h, i) => {
        drawHit(ctx, h, i + 1, ox, oy);
      });
    };
    img.src = src;
  }, [src, width, height, lines, showBoxes, hits, hitsOffset]);

  if (!src) {
    return (
      <div className="flex h-full min-h-[320px] items-center justify-center rounded-lg border-2 border-dashed border-neutral-200 bg-neutral-50/60 text-sm text-neutral-400">
        拖入图片，或按 Ctrl+V 粘贴
      </div>
    );
  }

  return (
    <div className="flex h-full items-center justify-center overflow-auto rounded-lg border border-neutral-200 bg-neutral-50/60">
      <canvas
        ref={canvasRef}
        data-testid="ocr-canvas"
        className="max-h-full max-w-full object-contain"
        style={{ aspectRatio: `${width} / ${height}` }}
      />
    </div>
  );
}

/** 画一个找图命中框（琥珀色）+ 序号与分数标签。 */
function drawHit(
  ctx: CanvasRenderingContext2D,
  hit: FindHit,
  index: number,
  offsetX: number,
  offsetY: number,
) {
  ctx.save();
  const x = hit.x - offsetX;
  const y = hit.y - offsetY;
  const w = hit.width;
  const h = hit.height;

  ctx.fillStyle = "rgba(217, 119, 6, 0.12)";
  ctx.fillRect(x, y, w, h);
  ctx.strokeStyle = "#D97706";
  const lw = Math.max(1.5, ctx.canvas.width / 600);
  ctx.lineWidth = lw;
  // 画在边框内侧，避免粗线溢出图像
  ctx.strokeRect(x + lw / 2, y + lw / 2, Math.max(0, w - lw), Math.max(0, h - lw));

  const fs = Math.max(11, ctx.canvas.width / 70);
  ctx.font = `600 ${fs}px "MiSans", sans-serif`;
  const label = `${index} · ${hit.score.toFixed(2)}`;
  const tw = ctx.measureText(label).width;
  const padX = fs * 0.35;
  const bx = Math.max(0, Math.min(x, ctx.canvas.width - tw - 2 * padX));
  const by = Math.max(0, y - fs - 2 * padX);
  ctx.fillStyle = "#D97706";
  ctx.beginPath();
  ctx.roundRect(bx, by, tw + 2 * padX, fs + 2 * padX, 4);
  ctx.fill();
  ctx.fillStyle = "#FFFFFF";
  ctx.textBaseline = "top";
  ctx.fillText(label, bx + padX, by + padX);
  ctx.restore();
}

/** 画一个四点框 + 序号标签。颜色用蓝色（点缀色），低透明度填充。 */
function drawBox(ctx: CanvasRenderingContext2D, box: Box4, index: number) {
  ctx.save();
  ctx.beginPath();
  ctx.moveTo(box[0][0], box[0][1]);
  for (let i = 1; i < 4; i++) ctx.lineTo(box[i][0], box[i][1]);
  ctx.closePath();

  ctx.fillStyle = "rgba(37, 99, 235, 0.10)";
  ctx.fill();
  ctx.strokeStyle = "#2563EB";
  ctx.lineWidth = Math.max(1.5, ctx.canvas.width / 600);
  ctx.stroke();

  // 序号标签：取框最上沿的角点
  let top = box[0];
  for (const p of box) if (p[1] < top[1]) top = p;
  const fs = Math.max(11, ctx.canvas.width / 70);
  ctx.font = `600 ${fs}px "MiSans", sans-serif`;
  const label = String(index);
  const tw = ctx.measureText(label).width;
  const padX = fs * 0.35;
  const bx = top[0];
  const by = Math.max(0, top[1] - fs - 2 * padX);
  ctx.fillStyle = "#2563EB";
  ctx.beginPath();
  ctx.roundRect(bx, by, tw + 2 * padX, fs + 2 * padX, 4);
  ctx.fill();
  ctx.fillStyle = "#FFFFFF";
  ctx.textBaseline = "top";
  ctx.fillText(label, bx + padX, by + padX);
  ctx.restore();
}
