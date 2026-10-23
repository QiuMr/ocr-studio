fn main() {
    // ── 版本号的唯一来源：`tauri.conf.json` 的 `version` ──────────────────
    //
    // 为什么不让 Cargo.toml 当源头：发版时要对齐的地方有 zip 文件名、
    // GitHub Release 的 tag、程序里显示的版本、更新检查时比对的版本。
    // 前后两组分别由 `pack_exe.ps1` / `gh release` 与这里注入的
    // `OCR_STUDIO_VERSION` 提供，于是「三处硬编码」收敛成一处：
    // **改版本只改 tauri.conf.json**。
    //
    // 不做 JSON 解析：tauri.conf.json 是标准 JSON（无注释），顶层 `"version"`
    // 是第一个出现的 version 键，切分足够，且不给 build 阶段引入依赖。
    println!("cargo:rerun-if-changed=tauri.conf.json");
    let raw = std::fs::read_to_string("tauri.conf.json")
        .expect("build.rs 读不到 tauri.conf.json（build.rs 的 CWD 是 src-tauri/）");
    let ver = raw
        .split("\"version\"")
        .nth(1)
        .and_then(|s| s.split('"').nth(1))
        .map(|s| s.trim().to_string())
        .expect("tauri.conf.json 里找不到顶层 \"version\"");
    // 收紧一下：版本号必须像 1.2.3。宁可编译失败，也不要把脏值埋进程序里
    // ——发布出去之后「版本号不对」是没法远程修的。
    let ok = !ver.is_empty()
        && ver.split('.').count() >= 2
        && ver
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+');
    assert!(ok, "tauri.conf.json 的 version 不像版本号: {ver:?}");
    println!("cargo:rustc-env=OCR_STUDIO_VERSION={ver}");

    tauri_build::build()
}
