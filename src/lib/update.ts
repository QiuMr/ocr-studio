/**
 * 更新检查结果的本地缓存。
 *
 * 为什么需要它：**主窗口静默查、托盘菜单负责显示**。
 * 托盘菜单是「点一下就该立刻出来」的东西，不能每次打开都联网等 15 秒；
 * 而静默检查又不该打扰正在用的人。于是两边通过一个 localStorage 条目交接。
 */

import type { UpdateCheck } from "../types";

export const UPDATE_CACHE_KEY = "ocr.update.check";

/** 6 小时内的缓存才管用：太久没开菜单的话，版本号可能早就变了。 */
const CACHE_TTL = 6 * 60 * 60 * 1000;

export interface CachedUpdate {
  has_update: boolean;
  latest: string;
  current: string;
  ts: number;
}

/**
 * 读出缓存。两种情况下判定失效并当「没查过」：
 * - 超过 TTL
 * - `current` 跟当前程序版本对不上 —— 说明这是**更新之前**写的，
 *   「有更新」的结论已经不成立了（用户已经升上来了）。
 */
export function readCachedUpdate(currentVersion: string): CachedUpdate | null {
  try {
    const raw = localStorage.getItem(UPDATE_CACHE_KEY);
    if (!raw) return null;
    const c = JSON.parse(raw) as CachedUpdate;
    if (typeof c?.ts !== "number" || Date.now() - c.ts > CACHE_TTL) return null;
    if (c.current !== currentVersion) return null;
    return c;
  } catch {
    return null;
  }
}

export function writeCachedUpdate(r: UpdateCheck): void {
  try {
    localStorage.setItem(
      UPDATE_CACHE_KEY,
      JSON.stringify({
        has_update: r.has_update,
        latest: r.latest,
        current: r.current,
        ts: Date.now(),
      } satisfies CachedUpdate),
    );
  } catch {
    // localStorage 不可用（隐私模式 / 配额满）时无所谓 ——
    // 缓存只是「让托盘菜单快一点」，丢了顶多多等一次联网检查。
  }
}
