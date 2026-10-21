//! 真实屏幕截图 + 找图的自校验。
//!
//! 思路：截实屏 → 从截图里挑一块**纹理足够丰富**的区域裁成模板 → 在同一张
//! 内存图里把模板找回来，位置必须与裁切原点完全一致。
//!
//! 因为模板和搜索图来自同一次截屏（同一份内存数据），不存在「截完屏画面变了」
//! 的竞态，所以这个断言是确定性的 —— 它验证的是 GDI 取像素链路、
//! `GetDIBits` 的 BGRA→RGB 转换、以及模板匹配三者是否自洽。
//!
//! 之所以要挑「高方差区域」：纯色区域零均值 NCC 无定义（方差为 0），
//! 本实现会返回 0 分拒绝命中。桌面壁纸大片纯色时若随便裁一块，测试会假失败。
//!
//! 运行：cargo test --test capture_live -- --ignored --nocapture

use image::RgbImage;
use ocr_studio_lib::capture;
use ocr_studio_lib::matcher::{find_template, MatchOptions};

/// 在图中寻找标准差最大的 `tw × th` 区域，返回其左上角。
///
/// 网格步进 16 像素粗扫，够用且快 —— 目的只是避开纯色区，不是求精确极值。
fn find_textured_region(img: &RgbImage, tw: u32, th: u32) -> Option<(u32, u32)> {
    let (w, h) = img.dimensions();
    if w < tw || h < th {
        return None;
    }
    let mut best = (0u32, 0u32, 0f64);
    let step = 16u32;
    for y in (0..=(h - th)).step_by(step as usize) {
        for x in (0..=(w - tw)).step_by(step as usize) {
            let mut sum = 0f64;
            let mut sum_sq = 0f64;
            let mut n = 0f64;
            // 区域内部再按 4 像素取样，控制计算量
            for dy in (0..th).step_by(4) {
                for dx in (0..tw).step_by(4) {
                    let p = img.get_pixel(x + dx, y + dy);
                    let v = (77.0 * p[0] as f64 + 150.0 * p[1] as f64 + 29.0 * p[2] as f64) / 256.0;
                    sum += v;
                    sum_sq += v * v;
                    n += 1.0;
                }
            }
            let mean = sum / n;
            let var = (sum_sq / n - mean * mean).max(0.0);
            if var > best.2 {
                best = (x, y, var);
            }
        }
    }
    // 方差低于 4（≈标准差 2 灰阶）认为没什么结构，不值得当模板
    if best.2 < 4.0 {
        None
    } else {
        Some((best.0, best.1))
    }
}

#[test]
#[ignore = "需要真实桌面会话（会截取当前屏幕）"]
fn screen_capture_has_expected_size() {
    capture::ensure_dpi_aware();
    let vs = capture::virtual_screen();
    println!("virtual_screen = {vs:?}");
    assert!(vs.width > 0 && vs.height > 0, "虚拟屏幕尺寸异常: {vs:?}");

    let img = capture::capture_region(None).expect("全屏截图失败");
    println!("captured = {}x{}", img.width(), img.height());
    assert_eq!(
        img.width() as i32, vs.width,
        "截图像素宽应与 GetSystemMetrics 一致（不一致说明 DPI 感知有问题）"
    );
    assert_eq!(img.height() as i32, vs.height);
}

#[test]
#[ignore = "需要真实桌面会话（会截取当前屏幕）"]
fn full_screen_self_match_finds_crop_origin() {
    capture::ensure_dpi_aware();
    let full = capture::capture_region(None).expect("全屏截图失败");
    println!("full = {}x{}", full.width(), full.height());

    const TW: u32 = 64;
    const TH: u32 = 64;
    let (sx, sy) = match find_textured_region(&full, TW, TH) {
        Some(v) => v,
        None => {
            println!("屏幕过于单调（找不到方差足够的区域），跳过自匹配");
            return;
        }
    };
    println!("template origin = ({sx}, {sy})");

    let tpl = image::imageops::crop_imm(&full, sx, sy, TW, TH).to_image();
    let hits = find_template(
        &full,
        &tpl,
        &MatchOptions {
            min_score: 0.95,
            max_results: 1,
        },
    )
    .expect("匹配失败");

    assert!(!hits.is_empty(), "同一次截屏内自匹配必须命中");
    println!("hit = {:?}", hits[0]);
    assert_eq!(
        (hits[0].x, hits[0].y),
        (sx, sy),
        "裁切原点必须能被原样找回"
    );
    assert!(hits[0].score > 0.999, "同一像素数据的 NCC 应≈1");
}

#[test]
#[ignore = "需要真实桌面会话（会截取当前屏幕）"]
fn sub_region_capture_keeps_coordinates_local() {
    capture::ensure_dpi_aware();
    let vs = capture::virtual_screen();
    // 取屏幕中间一块 400x300（屏幕太小时退化为整屏）
    let rw = 400.min(vs.width);
    let rh = 300.min(vs.height);
    if rw < 128 || rh < 128 {
        println!("屏幕太小({}x{})，跳过", vs.width, vs.height);
        return;
    }
    let rx = vs.x + (vs.width - rw) / 2;
    let ry = vs.y + (vs.height - rh) / 2;
    let region = capture::ScreenRegion {
        x: rx,
        y: ry,
        width: rw,
        height: rh,
    };
    let sub = capture::capture_region(Some(region)).expect("区域截图失败");
    println!("sub = {}x{} @ ({rx}, {ry})", sub.width(), sub.height());
    assert_eq!(sub.width(), rw as u32, "区域宽度应精确");
    assert_eq!(sub.height(), rh as u32, "区域高度应精确");

    let (sx, sy) = match find_textured_region(&sub, 48, 48) {
        Some(v) => v,
        None => {
            println!("区域过于单调，跳过");
            return;
        }
    };
    let tpl = image::imageops::crop_imm(&sub, sx, sy, 48, 48).to_image();
    let hits = find_template(
        &sub,
        &tpl,
        &MatchOptions {
            min_score: 0.95,
            max_results: 1,
        },
    )
    .expect("匹配失败");
    assert!(!hits.is_empty());
    // 子图内坐标是相对子图左上角的 —— 屏幕绝对坐标由 HTTP 层加 offset
    assert_eq!((hits[0].x, hits[0].y), (sx, sy));
}

#[test]
#[ignore = "需要真实桌面会话"]
fn list_windows_returns_titles_and_rects() {
    let ws = capture::list_windows();
    println!("发现 {} 个可见窗口", ws.len());
    for w in ws.iter().take(10) {
        println!("  hwnd={:#x} {}x{} @({},{}) {:?}", w.hwnd, w.rect.width, w.rect.height, w.rect.x, w.rect.y, w.title);
    }
    assert!(!ws.is_empty(), "至少应枚举到几个可见窗口");
    for w in &ws {
        assert!(w.rect.width > 0 && w.rect.height > 0, "窗口尺寸异常: {w:?}");
        assert!(!w.title.trim().is_empty());
    }
}

#[test]
#[ignore = "需要真实桌面会话"]
fn window_capture_produces_non_blank_image() {
    // 用任务栏之外的第一个窗口试；找不到就跳过
    let Some(target) = capture::list_windows()
        .into_iter()
        .find(|w| w.rect.width > 200 && w.rect.height > 150)
    else {
        println!("没有合适窗口，跳过");
        return;
    };
    println!(
        "尝试截取 hwnd={:#x} {:?} {}x{}",
        target.hwnd, target.title, target.rect.width, target.rect.height
    );
    match capture::capture_window(target.hwnd) {
        Ok(img) => {
            println!("got {}x{}", img.width(), img.height());
            assert_eq!(img.width(), target.rect.width as u32);
            assert_eq!(img.height(), target.rect.height as u32);
            // 至少得有点非黑像素，否则说明 PrintWindow 假成功且回退也失败
            let non_black = img.pixels().filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count();
            println!("non-black pixels = {non_black}");
            assert!(non_black > 100, "窗口截图几乎全黑，GDI 链路可能有问题");
        }
        Err(e) => {
            // 受保护窗口 / 最小化窗口失败是正常现象，不算测试失败
            println!("截窗失败（可能是受保护或已最小化窗口）: {e}");
        }
    }
}
