"""极简 Minidump 解析：只取崩溃线程的异常码/地址，并映射到所属模块。

用途：WebView2 白屏时判断到底崩在哪个 DLL（显卡驱动 / chromium 自身 / 其它）。
零依赖，纯标准库。

用法：python scripts/minidump_info.py <dump路径> [dump路径...]
"""

import struct
import sys

# Stream types
MODULE_LIST = 4
EXCEPTION = 6
SYSTEM_INFO = 7
MISC_INFO = 15


def read_str(buf: bytes, rva: int) -> str:
    """MINIDUMP_STRING: u32 字节长度 + UTF-16LE"""
    if rva == 0 or rva + 4 > len(buf):
        return "?"
    (n,) = struct.unpack_from("<I", buf, rva)
    raw = buf[rva + 4 : rva + 4 + n]
    return raw.decode("utf-16-le", "replace")


def parse(buf: bytes) -> dict:
    if buf[:4] != b"MDMP":
        raise ValueError("不是 minidump 文件")
    (nstreams, dir_rva) = struct.unpack_from("<II", buf, 8)
    streams = {}
    for i in range(nstreams):
        off = dir_rva + i * 12
        stype, size, rva = struct.unpack_from("<III", buf, off)
        streams.setdefault(stype, []).append((size, rva))

    out = {}

    # ---- 模块表 ----
    modules = []
    if MODULE_LIST in streams:
        _, rva = streams[MODULE_LIST][0]
        (count,) = struct.unpack_from("<I", buf, rva)
        for i in range(count):
            base = rva + 4 + i * 108
            base_of_image, size_of_image = struct.unpack_from("<QI", buf, base)
            # MINIDUMP_MODULE: BaseOfImage(8) SizeOfImage(4) CheckSum(4)
            # TimeDateStamp(4) ModuleNameRva(4) -> 偏移 20
            name_rva = struct.unpack_from("<I", buf, base + 20)[0]
            modules.append(
                {
                    "base": base_of_image,
                    "size": size_of_image,
                    "name": read_str(buf, name_rva),
                }
            )
    out["modules"] = modules

    # ---- 异常 ----
    if EXCEPTION in streams:
        _, rva = streams[EXCEPTION][0]
        tid = struct.unpack_from("<I", buf, rva)[0]
        er = rva + 8
        code, flags = struct.unpack_from("<II", buf, er)
        addr = struct.unpack_from("<Q", buf, er + 16)[0]
        nparams = struct.unpack_from("<I", buf, er + 24)[0]
        params = struct.unpack_from("<15Q", buf, er + 32)
        out["exception"] = {
            "thread_id": tid,
            "code": code,
            "flags": flags,
            "address": addr,
            "nparams": nparams,
            "params": [hex(p) for p in params[:nparams]],
        }

    # ---- 系统信息 ----
    if SYSTEM_INFO in streams:
        _, rva = streams[SYSTEM_INFO][0]
        arch = struct.unpack_from("<H", buf, rva)[0]
        out["arch"] = {0: "x86", 9: "x64", 12: "arm64"}.get(arch, str(arch))

    return out


def locate(modules, addr: int) -> str:
    for m in modules:
        if m["base"] <= addr < m["base"] + max(m["size"], 1):
            return f"{m['name']} +0x{addr - m['base']:X}"
    return "(未落在任何模块内)"


COMMON_CODES = {
    0xC0000005: "ACCESS_VIOLATION 访问违例",
    0xC000001D: "ILLEGAL_INSTRUCTION 非法指令",
    0xC0000094: "INT_DIVIDE_BY_ZERO 整数除零",
    0xC00000FD: "STACK_OVERFLOW 栈溢出",
    0xC0000374: "HEAP_CORRUPTION 堆损坏",
    0xC0000409: "STACK_BUFFER_OVERRUN / fail fast",
    0x80000003: "BREAKPOINT 断点",
    0xE06D7363: "C++ 异常",
}


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    for path in sys.argv[1:]:
        print(f"=== {path}")
        try:
            with open(path, "rb") as f:
                buf = f.read()
            info = parse(buf)
        except Exception as e:
            print(f"    解析失败: {e}")
            continue

        ex = info.get("exception")
        mods = info["modules"]
        print(f"    架构={info.get('arch')} 模块数={len(mods)}")
        big = [m for m in mods if m["size"] > 1_000_000]
        print(f"    大模块: {', '.join(m['name'] for m in big[:8])}")
        if not ex:
            print("    无异常流（可能是主动退出/被终止）")
            continue
        code = ex["code"]
        print(f"    线程={ex['thread_id']} 异常码=0x{code:08X} ({COMMON_CODES.get(code, '未知')})")
        print(f"    崩溃地址=0x{ex['address']:X} -> {locate(mods, ex['address'])}")
        if ex["params"]:
            print(f"    参数={ex['params']}")
            # 访问违例：参数0=读0/写1/执行8，参数1=访问的地址
            if code == 0xC0000005 and ex["nparams"] >= 2:
                kind = {"0x0": "读", "0x1": "写", "0x8": "执行"}.get(ex["params"][0], "?")
                print(f"    违例类型={kind} 目标地址={ex['params'][1]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
