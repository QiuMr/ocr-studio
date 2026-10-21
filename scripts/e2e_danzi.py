"""端到端验证 /ocr/danzijson：逐字框必须落在行框内部，且数量与文本长度一致。

用法：python scripts/e2e_danzi.py <图片路径>
必须绕系统代理：本机开着代理时 127.0.0.1 也会被转发，拿到 502 会误判成服务没起来。
"""

import base64
import json
import sys
import urllib.request

ENDPOINT = "http://127.0.0.1:8848/ocr/danzijson"


def opener():
    return urllib.request.build_opener(urllib.request.ProxyHandler({}))


def main() -> int:
    if len(sys.argv) < 2:
        print("用法: python scripts/e2e_danzi.py <图片路径>", file=sys.stderr)
        return 2
    path = sys.argv[1]
    with open(path, "rb") as f:
        b64 = base64.b64encode(f.read()).decode()

    req = urllib.request.Request(
        ENDPOINT,
        data=json.dumps({"image": b64, "use_cls": True}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with opener().open(req, timeout=120) as resp:
        payload = json.loads(resp.read().decode())

    if payload.get("code") != 0:
        print(f"[FAIL] 接口返回错误: {payload}")
        return 1

    data = payload["data"]
    lines = data["lines"]
    print(f"[INFO] {data['width']}x{data['height']} 耗时 {data['elapsed_ms']}ms")
    print(f"[INFO] 行数 {len(lines)} 字数 {data['char_count']}")

    bad = 0
    for li, line in enumerate(lines):
        text = line["text"]
        chars = line["chars"]
        # 文本长度必须等于字数：说明 CTC 帧对齐没有漏字/多字
        if len(text) != len(chars):
            print(f"[FAIL] 第{li + 1}行 文本 {len(text)} 字，但给了 {len(chars)} 个字框")
            bad += 1
        lx0 = min(p[0] for p in line["box"])
        lx1 = max(p[0] for p in line["box"])
        ly0 = min(p[1] for p in line["box"])
        ly1 = max(p[1] for p in line["box"])
        for c in chars:
            cx0 = min(p[0] for p in c["box"])
            cx1 = max(p[0] for p in c["box"])
            cy0 = min(p[1] for p in c["box"])
            cy1 = max(p[1] for p in c["box"])
            # 字框必须落在行框内（允许 2px 越界：字框由帧比例算出，边界可能压线）
            if cx0 < lx0 - 2 or cx1 > lx1 + 2 or cy0 < ly0 - 2 or cy1 > ly1 + 2:
                print(f"[FAIL] 第{li + 1}行 字 {c['ch']!r} 框越出行框")
                bad += 1
        print(f"  {li + 1}. {text!r}  {len(chars)} 字  帧数 {line['frames']}")

    if bad:
        print(f"[FAIL] {bad} 项断言失败")
        return 1
    print("[PASS] 逐字框数量与文本一致，且全部落在行框内")
    return 0


if __name__ == "__main__":
    sys.exit(main())
