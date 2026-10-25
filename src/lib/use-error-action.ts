import { useCallback } from "react";
import type { ErrorActionKind } from "./errors";

/** 设置页里可以被「指路」定位到的区块 */
export type SettingsFocus = "models" | "ai";

/**
 * 错误提示里「下一步」按钮的动作分发。
 *
 * 为什么集中成一处：试跑页四个面板、找图页、表格页的按钮语义完全一样
 * （重试 / 去模型库 / 开 AI / 看服务），各写一份迟早会出现
 * 「同一个按钮在不同页面跳到不同地方」这种事。
 *
 * 放 `lib/` 而不是某个页面里：`ScanPanel` / `TablePanel` / `FindPanel` 都在
 * `components/` 下，从 `pages/Playground` 引用会造成循环依赖。
 */
export function useErrorAction(opts: {
  /** 「重试一次」要做什么。没给就不显示重试按钮 */
  onRetry?: () => void;
  onOpenSettings: (focus?: SettingsFocus) => void;
  onOpenService: () => void;
}) {
  const { onRetry, onOpenSettings, onOpenService } = opts;
  return useCallback(
    (kind: ErrorActionKind) => {
      switch (kind) {
        case "retry":
          onRetry?.();
          break;
        case "open-models":
          onOpenSettings("models");
          break;
        case "open-ai":
          onOpenSettings("ai");
          break;
        case "open-settings":
          onOpenSettings();
          break;
        case "open-service":
          onOpenService();
          break;
      }
    },
    [onRetry, onOpenSettings, onOpenService],
  );
}
