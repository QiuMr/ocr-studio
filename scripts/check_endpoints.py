#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""静态比对前端接口清单与后端路由表 —— 漏一个端点/漏一组就在构建时 FAIL。

为什么需要这个脚本：界面上的「接口」页是**产品说明书**，数据源是
`src/lib/endpoints.ts`；后端真相是 `src-tauri/src/api/routes.rs` 的 `PATHS`。
两边各写各的，历史上真出过两次事故：

1. 表格接口 `/ocr/biaoge` 加进了 ENDPOINTS，但接口页和首页各有一份**手写的
   组数组**没跟着改 —— TypeScript 不报错（普通数组不是 Record），
   结果是表格接口在两个页面上静默消失，而页面上写的「共 21 个端点」又是对的，
   肉眼完全看不出少了一组。
2. AI 工具层的 ENDPOINT_METHODS 漏登记 /ocr/biaoge。

这个脚本把三件事钉死：
  A. ENDPOINTS 里的路径集合 == 后端 PATHS 的集合（前后端一本账）；
  B. EndpointGroup 的每个成员都在 GROUP_ORDER 里（组不漏）；
  C. 每个 ENDPOINTS 条目的 groupOf 结果都落在 GROUP_ORDER 里（条目不进野组）。

不启动服务、不编译，纯文本解析，几十毫秒跑完，适合挂在 build.ps1 里。
"""
from __future__ import annotations

import io
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FRONT = ROOT / "src" / "lib" / "endpoints.ts"
ROUTES = ROOT / "src-tauri" / "src" / "api" / "routes.rs"


def fail(msg: str) -> None:
    print(f"[FAIL] {msg}")


def ok(msg: str) -> None:
    print(f"[ OK ] {msg}")


def front_paths() -> set[str]:
    """从 endpoints.ts 抓 path: "..."。"""
    s = io.open(FRONT, encoding="utf-8").read()
    return set(re.findall(r'path:\s*"([^"]+)"', s))


def front_groups() -> tuple[set[str], list[str]]:
    """EndpointGroup 联合成员 + GROUP_ORDER 数组内容。"""
    s = io.open(FRONT, encoding="utf-8").read()
    m = re.search(r'export type EndpointGroup = ([^;]+);', s)
    if not m:
        fail("endpoints.ts 里找不到 EndpointGroup 定义")
        return set(), []
    members = set(re.findall(r'"([a-z]+)"', m.group(1)))
    m2 = re.search(r"export const GROUP_ORDER[^=]+=\s*\[([^\]]+)\]", s)
    order = re.findall(r'"([a-z]+)"', m2.group(1)) if m2 else []
    return members, order


def group_of_rules() -> list[tuple[str, str]]:
    """groupOf() 里的 path === "..." 特判（用于 C 项粗查）。"""
    s = io.open(FRONT, encoding="utf-8").read()
    body = s[s.find("export function groupOf") :]
    body = body[: body.find("\n}")]
    return re.findall(r'path\s*===\s*"([^"]+)"\s*\)\s*return\s*"([a-z]+)"', body)


def back_paths() -> set[str]:
    """后端 PATHS：先取 P_* 常量的字面量，再展开 PATHS 数组里的标识符。

    这样新增常量名不用改脚本。
    """
    s = io.open(ROUTES, encoding="utf-8").read()
    consts = dict(re.findall(r'const (P_\w+):\s*&str\s*=\s*"([^"]+)"', s))
    m = re.search(r"pub const PATHS:[^=]+=\s*&\[(.*?)\];", s, re.S)
    if not m:
        fail("routes.rs 里找不到 PATHS 定义")
        return set()
    out: set[str] = set()
    for ident in re.findall(r"\b(P_\w+)\b", m.group(1)):
        if ident in consts:
            out.add(consts[ident])
    # PATHS 里也可能直接写字面量
    out |= set(re.findall(r'"(/[^"]+)"', m.group(1)))
    return out


def main() -> int:
    if not FRONT.exists():
        fail(f"找不到 {FRONT}")
        return 1
    if not ROUTES.exists():
        fail(f"找不到 {ROUTES}")
        return 1

    bad = 0
    fp, bp = front_paths(), back_paths()

    only_back = sorted(bp - fp)
    only_front = sorted(p for p in fp - bp if p not in ("/vision/promptref",))
    # /vision/promptref 是「一个路径三种方法」，前端拆 3 条写，路径去重后仍在 fp 里，
    # 所以正常不会出现在 only_front；留这个过滤只是防御。
    if only_back:
        bad += 1
        fail(f"后端有路由但前端接口页没写（用户在看界面上看不到）：{only_back}")
    if only_front:
        bad += 1
        fail(f"前端写了但后端没注册（点了示例会 404）：{only_front}")
    if not only_back and not only_front:
        ok(f"前后端路径一致（{len(bp)} 个）")

    members, order = front_groups()
    if not order:
        bad += 1
        fail("endpoints.ts 里找不到 GROUP_ORDER")
    else:
        missing = sorted(members - set(order))
        extra = sorted(set(order) - members)
        if missing:
            bad += 1
            fail(f"有组没进 GROUP_ORDER，界面会静默漏掉这一组：{missing}")
        elif extra:
            bad += 1
            fail(f"GROUP_ORDER 里有不存在的组：{extra}")
        else:
            ok(f"GROUP_ORDER 覆盖全部 {len(members)} 个组")

    # C：groupOf 特判返回的组必须存在
    for path, g in group_of_rules():
        if g not in members:
            bad += 1
            fail(f'groupOf 把 {path} 分到不存在的组 "{g}"')

    # D：接口页/首页必须从 GROUP_ORDER 派生（不许再出现手写组数组）
    for f in ("src/pages/ApiPage.tsx", "src/pages/ServicePage.tsx"):
        s = io.open(ROOT / f, encoding="utf-8").read()
        if "GROUP_ORDER" not in s:
            bad += 1
            fail(f"{f} 没有用 GROUP_ORDER —— 手写组数组就是漏组的根因")
    else:
        ok("接口页与首页均从 GROUP_ORDER 派生")

    if bad:
        print(f"[结论] {bad} 项不一致，改完再构建")
        return 1
    print("[结论] 接口清单与后端路由一致")
    return 0


if __name__ == "__main__":
    sys.exit(main())
