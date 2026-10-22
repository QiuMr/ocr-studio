"""核对 HF 的 CLIP tokenizer.json 与 ultralytics 用的 `clip.clip.tokenize` 是否逐 id 一致。

为什么要核：Rust 侧用 `mobileclip2-b-rs` 的分词器（吃 HF 格式 tokenizer.json）算
文本特征，而导出时参照的 `feat` 是 Python 侧 `clip.clip.tokenize` 出的 id 喂给
`mobileclip2_b.ts` 得到的。两处分词只要差一个 id，`pe` 就会变，而 pe 变了对
用户就是「同样的词找不到东西」——这种错不会报任何异常，必须在这里钉死。

用法：python scripts/_probe_tokenizer.py
"""
from __future__ import annotations

import sys
from pathlib import Path

TOKENIZER_JSON = Path(sys.argv[1] if len(sys.argv) > 1 else "models/yoloe/tokenizer.min.json")

WORDS = [
    "person",
    "bus",
    "traffic light",
    "a photo of a person, a type of object",
    "确认按钮",
    "button",
    "close",
    "the quick brown fox jumps over the lazy dog",
    "  leading and trailing  ",
    "UPPER lower 123 !@#$%^&*()",
]


def main() -> None:
    import clip as clip_pkg
    from tokenizers import Tokenizer

    if not TOKENIZER_JSON.is_file():
        sys.exit(f"找不到 {TOKENIZER_JSON}")

    tok = Tokenizer.from_file(str(TOKENIZER_JSON))

    # clip.clip.tokenize 会补 sot/eot 并 pad 到 77（用 0 填充）
    ref = clip_pkg.clip.tokenize(WORDS, truncate=True).tolist()
    if not isinstance(ref[0], list):
        ref = [ref]

    # 关键：**不能**把 id 0 当 padding 过滤掉——CLIP 的字节级词表里 id 0 就是字符
    # `!` 的合法 token（bytes_to_unicode 的第一项是 0x21）。过滤会把真实 token
    # 删掉，反而把「一致」判成「不一致」（这个探针第一版就栽在这）。
    # 正确做法：把 HF 的输出右补 0 到 77，与 `clip.tokenize` 的整条序列逐位比。
    pad_to = len(ref[0])
    bad = 0
    for i, w in enumerate(WORDS):
        ids = tok.encode(w, add_special_tokens=True).ids
        got = ids[:pad_to] + [0] * max(0, pad_to - len(ids))
        r = list(ref[i])
        same = got == r
        if not same:
            bad += 1
        print(f"{'OK ' if same else 'DIFF'} {w!r}  （{len(ids)} 个 token，补到 {pad_to}）")
        if not same:
            first = next(j for j in range(pad_to) if got[j] != r[j])
            print(f"     首个不同在 #{first}: hf={got[first]} ref={r[first]}")
            print(f"     hf  = {got[:20]}…")
            print(f"     ref = {r[:20]}…")

    print()
    print("全部一致 ✅" if bad == 0 else f"{bad} 条不一致 ❌")


if __name__ == "__main__":
    main()
