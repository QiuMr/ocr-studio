import { useEffect, useRef, useState } from "react";
import { Maximize2, Minus, Plus } from "lucide-react";
import { cn } from "../lib/utils";
import type { Box4, CharResult, FindHit, LineResult } from "../types";

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
  /** 逐字框（单字 JSON）。与行框叠加时行框只画描边，避免糊成一团 */
  chars?: CharResult[];
  /** 没有图时显示什么。默认给一句提示，页面可以传更完整的引导 */
  empty?: React.ReactNode;
}

/**
 * 图像 + 检测框叠加显示。
 *
 * 用 canvas 而不是绝对定位 div：框坐标是原图像素坐标，canvas 可以按同一比例
 * 缩放，定位天然一致；div 方案要自己算 CSS 缩放，容易在 devicePixelRatio 上跑偏。
 *
 * 交互：鼠标移上来右下角出现缩放控件；滚轮可缩放（`+/-` 与双击复位同理）。
 */
export function ImageViewer({
  src,
  width,
  height,
  lines,
  showBoxes,
  hits,
  hitsOffset,
  chars,
  empty,
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);
  /** 1 = 适应容器；其它值为实际 CSS 像素宽度倍数 */
  const [zoom, setZoom] = useState(1);

  useEffect(() => {
    setZoom(1);
  }, [src]);

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
      // 有字框时行框只画描边：两层都填充会糊成一片
      const hasChars = (chars?.length ?? 0) > 0;
      lines.forEach((line, i) => {
        drawBox(ctx, line.box, i + 1, hasChars);
      });

      const ox = hitsOffset?.x ?? 0;
      const oy = hitsOffset?.y ?? 0;
      (hits ?? []).forEach((h, i) => {
        drawHit(ctx, h, i + 1, ox, oy);
      });

      (chars ?? []).forEach((c) => {
        drawChar(ctx, c.box);
      });
    };
    img.src = src;
  }, [src, width, height, lines, showBoxes, hits, hitsOffset, chars]);

  if (!src) {
    return (
      <>
        {empty ?? (
          <div className="flex h-full min-h-[280px] items-center justify-center rounded-lg bg-neutral-50/60 text-[12.5px] text-neutral-400">
            拖入图片，或按 Ctrl+V 粘贴
          </div>
        )}
      </>
    );
  }

  const fit = zoom === 1;

  return (
    <div className="group relative flex h-full items-center justify-center overflow-auto rounded-lg bg-neutral-50/60">
      <canvas
        ref={canvasRef}
        data-testid="ocr-canvas"
        onWheel={(e) => {
          // 只有按住 Ctrl 才缩放，否则留给容器滚动，避免页面被「锁住」
          if (!e.ctrlKey) return;
          e.preventDefault();
          setZoom((z) => Math.min(4, Math.max(0.25, z - e.deltaY * 0.001)));
        }}
        onDoubleClick={() => setZoom(1)}
        className={cn(fit ? "max-h-full max-w-full object-contain" : "shrink-0 cursor-zoom-out")}
        style={
          fit
            ? { aspectRatio: `${width} / ${height}` }
            : { width: Math.round(width * zoom), height: Math.round(height * zoom) }
        }
      />

      {/* 缩放控件：默认隐身，鼠标进区域才出现 */}
      <div className="pointer-events-none absolute bottom-2 right-2 flex items-center gap-0.5 rounded-lg bg-white/95 px-1 py-0.5 opacity-0 shadow-sm transition-opacity group-hover:pointer-events-auto group-hover:opacity-100">
        <ZoomBtn label="缩小" onClick={() => setZoom((z) => Math.max(0.25, z - 0.25))}>
          <Minus className="h-3 w-3" />
        </ZoomBtn>
        <span className="min-w-[42px] text-center font-mono text-[10.5px] text-neutral-500">
          {fit ? "适应" : `${Math.round(zoom * 100)}%`}
        </span>
        <ZoomBtn label="放大" onClick={() => setZoom((z) => Math.min(4, z + 0.25))}>
          <Plus className="h-3 w-3" />
        </ZoomBtn>
        <ZoomBtn label="恢复适应窗口" onClick={() => setZoom(1)}>
          <Maximize2 className="h-3 w-3" />
        </ZoomBtn>
      </div>
    </div>
  );
}

function ZoomBtn({
  children,
  label,
  onClick,
}: {
  children: React.ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className="flex h-5 w-5 items-center justify-center rounded text-neutral-500 hover:bg-neutral-100 hover:text-neutral-800"
    >
      {children}
    </button>
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

/** 画单个字的框（细线、不填充），避免遮住笔画。 */
function drawChar(ctx: CanvasRenderingContext2D, box: Box4) {
  ctx.save();
  ctx.beginPath();
  ctx.moveTo(box[0][0], box[0][1]);
  for (let i = 1; i < 4; i++) ctx.lineTo(box[i][0], box[i][1]);
  ctx.closePath();
  ctx.strokeStyle = "#0EA5E9";
  ctx.lineWidth = Math.max(1, ctx.canvas.width / 900);
  ctx.stroke();
  ctx.restore();
}

/** 画一个四点框 + 序号标签。颜色用蓝色（点缀色），低透明度填充。 */
function drawBox(ctx: CanvasRenderingContext2D, box: Box4, index: number, strokeOnly = false) {
  ctx.save();
  ctx.beginPath();
  ctx.moveTo(box[0][0], box[0][1]);
  for (let i = 1; i < 4; i++) ctx.lineTo(box[i][0], box[i][1]);
  ctx.closePath();

  if (!strokeOnly) {
    ctx.fillStyle = "rgba(37, 99, 235, 0.10)";
    ctx.fill();
  }
  ctx.strokeStyle = "#2563EB";
  ctx.lineWidth = Math.max(1.5, ctx.canvas.width / 600);
  if (strokeOnly) ctx.globalAlpha = 0.45;
  ctx.stroke();
  ctx.globalAlpha = 1;

  // 字框模式下不画行的序号，否则和字框挤在一起看不清
  if (strokeOnly) {
    ctx.restore();
    return;
  }

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
