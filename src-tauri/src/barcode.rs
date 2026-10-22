//! 二维码 / 条码识别（基于 `rxing`，ZXing 的纯 Rust 移植）。
//!
//! 为什么自己转灰度而不是把图交给 rxing 的 `detect_multiple_in_image`：
//! 那个入口需要开 rxing 的 `image` + `image_formats` feature，会把 image crate
//! 的 avif/tiff/webp/exr 全套编解码（rav1e 等）拖进二进制。我们本来就有一张
//! `RgbImage`，转 luma8 只是一次线性扫描，用零拷贝入口 `*_in_luma_with_hints`
//! 即可，省下几十 MB 体积。
//!
//! 覆盖的码制：QR（含 Micro QR）、DataMatrix、Aztec、PDF417、MaxiCode，以及
//! Code 39/93/128、EAN-8/13、UPC-A/E、ITF、Codabar、RSS 等一维码。
//!
//! 已知限制（诚实记录）：
//! - **不做畸变校正**。透视严重变形的码 rxing 自己会尝试定位，但过于极端的
//!   角度仍会失败。需要时先把图裁小、矫正再传。
//! - **不返回纠错级别、版本号**这类元数据，只给文本 + 定位点。
//! - 一维码在**长边被压缩**（比如整屏截图缩放到很窄）时容易失败，
//!   因为条空宽度比被破坏了。要稳就传原分辨率的图。

use std::collections::HashSet;

use image::RgbImage;
use rxing::{BarcodeFormat, DecodeHintValue, DecodeHints, Exceptions};

/// 一个识别到的码。
#[derive(Debug, Clone)]
pub struct BarcodeHit {
    /// 码里编的内容（原文，不做任何解析）
    pub text: String,
    /// 码制名，用 ZXing 的规范写法（`QR_CODE` / `CODE_128` / `EAN_13` …）
    pub format: String,
    /// 定位点/角点，**原图坐标**。QR 一般给 3 个定位图案 + 1 个校正点。
    pub points: Vec<(f32, f32)>,
    /// 上面那些点的轴对齐包围盒 `[x, y, w, h]`（已取整，可能为空点 → 全 0）
    pub bounds: [i32; 4],
}

/// 一次识别的参数。
#[derive(Debug, Clone, Default)]
pub struct BarcodeOptions {
    /// 只找这些码制。空 = 全部。元素形如 `qr` / `code128` / `ean13` / `datamatrix`。
    pub formats: Vec<String>,
    /// 是否让 rxing 多花点时间找（默认开）。关掉更快，但一维码容易漏。
    pub try_harder: bool,
}

/// 在 `img` 里找出所有二维码/条码。
///
/// 找不到返回空 `Vec`（不是错误——「这张图里没有码」是正常业务结果）。
pub fn decode_barcodes(
    img: &RgbImage,
    opts: &BarcodeOptions,
) -> Result<Vec<BarcodeHit>, String> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err("图片尺寸为 0".to_string());
    }
    // rxing 对过小的图基本没戏，提前给出可读的错误而不是让它内部崩
    if w < 16 || h < 16 {
        return Err(format!("图片太小（{w}x{h}），条码至少需要十几像素宽"));
    }

    let luma = to_luma8(img);

    let mut hints = DecodeHints::default();
    if opts.try_harder {
        hints = hints.with(DecodeHintValue::TryHarder(true));
    }
    let wanted = parse_formats(&opts.formats)?;
    if !wanted.is_empty() {
        hints = hints.with(DecodeHintValue::PossibleFormats(wanted));
    }

    let results = match rxing::helpers::detect_multiple_in_luma_with_hints(luma, w, h, &mut hints) {
        Ok(v) => v,
        // ZXing 的约定：一个都没找到时**抛 NotFoundException**，而不是返回空集合。
        // 对我们来说「这张图里没有码」是正常业务结果，必须转成空 Vec；
        // 若把它当错误往上抛，调用方会把「没码」误判成「接口坏了」。
        Err(Exceptions::NotFoundException(_)) => return Ok(Vec::new()),
        Err(e) => return Err(format!("条码识别失败: {e}")),
    };

    Ok(results
        .into_iter()
        .map(|r| {
            let points: Vec<(f32, f32)> = r.getPoints().iter().map(|p| (p.x, p.y)).collect();
            BarcodeHit {
                text: r.getText().to_string(),
                format: format_name(r.getBarcodeFormat()).to_string(),
                bounds: bounds_of(&points),
                points,
            }
        })
        .collect())
}

/// 灰度化：与模板匹配用同一套定点 luma，保证两处对「同一个像素」的判断一致。
fn to_luma8(img: &RgbImage) -> Vec<u8> {
    let raw = img.as_raw();
    let mut out = Vec::with_capacity(raw.len() / 3);
    for p in raw.chunks_exact(3) {
        // 0.299R + 0.587G + 0.114B ≈ (77R + 150G + 29B) >> 8
        let y = (77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8;
        out.push(y as u8);
    }
    out
}

/// 定位点的轴对齐包围盒。没有点（一维码偶尔如此）时返回全 0。
fn bounds_of(points: &[(f32, f32)]) -> [i32; 4] {
    if points.is_empty() {
        return [0, 0, 0, 0];
    }
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let x0 = min_x.floor().max(0.0) as i32;
    let y0 = min_y.floor().max(0.0) as i32;
    let x1 = max_x.ceil().max(0.0) as i32;
    let y1 = max_y.ceil().max(0.0) as i32;
    [x0, y0, (x1 - x0).max(0), (y1 - y0).max(0)]
}

/// 码制的对外名字。
///
/// **不用 `BarcodeFormat` 的 `Display`**：rxing 的 Display 给的是小写带空格的
/// 展示名（`qrcode` / `code 128` / `ean 13`），拿来当接口字段既难解析也不稳定
/// （空格在 URL/表单里还要转义）。这里统一成 ZXing 的规范大写名，与 Java 版
/// ZXing、多数扫码工具的约定一致，脚本可以直接字符串比较。
fn format_name(f: &BarcodeFormat) -> &'static str {
    match f {
        BarcodeFormat::AZTEC => "AZTEC",
        BarcodeFormat::CODABAR => "CODABAR",
        BarcodeFormat::CODE_39 => "CODE_39",
        BarcodeFormat::CODE_93 => "CODE_93",
        BarcodeFormat::CODE_128 => "CODE_128",
        BarcodeFormat::DATA_MATRIX => "DATA_MATRIX",
        BarcodeFormat::EAN_8 => "EAN_8",
        BarcodeFormat::EAN_13 => "EAN_13",
        BarcodeFormat::ITF => "ITF",
        BarcodeFormat::MAXICODE => "MAXICODE",
        BarcodeFormat::PDF_417 => "PDF_417",
        BarcodeFormat::QR_CODE => "QR_CODE",
        BarcodeFormat::MICRO_QR_CODE => "MICRO_QR_CODE",
        BarcodeFormat::RECTANGULAR_MICRO_QR_CODE => "RECTANGULAR_MICRO_QR_CODE",
        BarcodeFormat::RSS_14 => "RSS_14",
        BarcodeFormat::RSS_EXPANDED => "RSS_EXPANDED",
        BarcodeFormat::TELEPEN => "TELEPEN",
        BarcodeFormat::UPC_A => "UPC_A",
        BarcodeFormat::UPC_E => "UPC_E",
        BarcodeFormat::UPC_EAN_EXTENSION => "UPC_EAN_EXTENSION",
        BarcodeFormat::DXFilmEdge => "DX_FILM_EDGE",
        BarcodeFormat::UNSUPORTED_FORMAT => "UNSUPORTED_FORMAT",
    }
}

/// 把用户写的码制名解析成 rxing 的枚举。
///
/// 接受常见写法（大小写不敏感、`-`/空格当 `_`），并支持两个**分组**快捷写法：
/// - `qr` / `二维码` → QR_CODE
/// - `1d` / `条形码` / `barcode` → 全部一维码
/// - `2d` / `矩阵码` → 全部二维码（QR + DataMatrix + Aztec + PDF417 + MaxiCode）
///
/// 空列表返回空集合 = 不过滤（全找）。认不出的名字直接报错而不是静默忽略：
/// 拼错一个码制名却「什么也没找到」，比报错更难排查。
fn parse_formats(list: &[String]) -> Result<HashSet<BarcodeFormat>, String> {
    let mut out = HashSet::new();
    for raw in list {
        let key = raw.trim().to_ascii_uppercase().replace(['-', ' '], "_");
        if key.is_empty() {
            continue;
        }
        match key.as_str() {
            "QR" | "QRCODE" | "QR_CODE" | "二维码" => {
                out.insert(BarcodeFormat::QR_CODE);
            }
            "1D" | "BARCODE" | "条形码" | "一维码" => {
                out.extend(ONE_D);
            }
            "2D" | "矩阵码" | "二维" => {
                out.extend(TWO_D);
            }
            _ => match single_format(&key) {
                Some(f) => {
                    out.insert(f);
                }
                None => {
                    return Err(format!(
                        "不认识的码制 {raw:?}。可用：qr / datamatrix / aztec / pdf417 / maxicode / \
                         code39 / code93 / code128 / ean8 / ean13 / upca / upce / itf / codabar，\
                         或分组写法 1d（全部一维码）/ 2d（全部二维码）"
                    ));
                }
            },
        }
    }
    Ok(out)
}

const ONE_D: [BarcodeFormat; 10] = [
    BarcodeFormat::CODE_39,
    BarcodeFormat::CODE_93,
    BarcodeFormat::CODE_128,
    BarcodeFormat::EAN_8,
    BarcodeFormat::EAN_13,
    BarcodeFormat::UPC_A,
    BarcodeFormat::UPC_E,
    BarcodeFormat::ITF,
    BarcodeFormat::CODABAR,
    BarcodeFormat::RSS_14,
];

const TWO_D: [BarcodeFormat; 5] = [
    BarcodeFormat::QR_CODE,
    BarcodeFormat::DATA_MATRIX,
    BarcodeFormat::AZTEC,
    BarcodeFormat::PDF_417,
    BarcodeFormat::MAXICODE,
];

fn single_format(key: &str) -> Option<BarcodeFormat> {
    let by_us = match key {
        "QR" | "QRCODE" | "QR_CODE" => BarcodeFormat::QR_CODE,
        "MICRO_QR" | "MICRO_QR_CODE" => BarcodeFormat::MICRO_QR_CODE,
        "DATA_MATRIX" | "DATAMATRIX" => BarcodeFormat::DATA_MATRIX,
        "AZTEC" => BarcodeFormat::AZTEC,
        "PDF417" | "PDF_417" => BarcodeFormat::PDF_417,
        "MAXICODE" | "MAXI_CODE" => BarcodeFormat::MAXICODE,
        "CODE39" | "CODE_39" => BarcodeFormat::CODE_39,
        "CODE93" | "CODE_93" => BarcodeFormat::CODE_93,
        "CODE128" | "CODE_128" => BarcodeFormat::CODE_128,
        "EAN8" | "EAN_8" => BarcodeFormat::EAN_8,
        "EAN13" | "EAN_13" => BarcodeFormat::EAN_13,
        "UPCA" | "UPC_A" => BarcodeFormat::UPC_A,
        "UPCE" | "UPC_E" => BarcodeFormat::UPC_E,
        "ITF" => BarcodeFormat::ITF,
        "CODABAR" => BarcodeFormat::CODABAR,
        "RSS14" | "RSS_14" => BarcodeFormat::RSS_14,
        "RSS_EXPANDED" => BarcodeFormat::RSS_EXPANDED,
        "TELEPEN" => BarcodeFormat::TELEPEN,
        "DX_FILM_EDGE" => BarcodeFormat::DXFilmEdge,
        // 我们自己的表没命中就交给 rxing 的别名表（它还认 "alpha39"、
        // "gs1 databar"、"iso 15438" 这类历史写法），认不出会得到 UNSUPORTED_FORMAT。
        _ => {
            let f = BarcodeFormat::from(key);
            if f == BarcodeFormat::UNSUPORTED_FORMAT {
                return None;
            }
            f
        }
    };
    Some(by_us)
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;
    use rxing::{MultiFormatWriter, Writer};

    /// 用 rxing 自己生成一张码，再解码回来。生成→识别闭环能同时验证
    /// 「灰度化没把图搞坏」和「hints 没把格式过滤搞坏」。
    fn render_barcode(text: &str, format: BarcodeFormat, w: u32, h: u32) -> RgbImage {
        let mat = MultiFormatWriter
            .encode(text, &format, w as i32, h as i32)
            .expect("编码失败");
        let (mw, mh) = (mat.getWidth(), mat.getHeight());
        let mut img = RgbImage::new(mw, mh);
        for y in 0..mh {
            for x in 0..mw {
                let on = mat.get(x, y);
                let v = if on { 0u8 } else { 255u8 };
                img.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }
        img
    }

    #[test]
    fn decodes_generated_qr() {
        let img = render_barcode("https://example.com/hello", BarcodeFormat::QR_CODE, 240, 240);
        let hits = decode_barcodes(&img, &BarcodeOptions { formats: vec![], try_harder: true })
            .expect("识别应成功");
        assert_eq!(hits.len(), 1, "应识别出 1 个码，实际 {}", hits.len());
        assert_eq!(hits[0].text, "https://example.com/hello");
        assert_eq!(hits[0].format, "QR_CODE");
        assert!(!hits[0].points.is_empty(), "QR 应给出定位点");
        assert!(hits[0].bounds[2] > 0 && hits[0].bounds[3] > 0, "包围盒应有面积");
    }

    #[test]
    fn decodes_generated_code128() {
        let img = render_barcode("ABC-123456", BarcodeFormat::CODE_128, 320, 100);
        let hits = decode_barcodes(&img, &BarcodeOptions { formats: vec![], try_harder: true })
            .expect("识别应成功");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "ABC-123456");
        assert_eq!(hits[0].format, "CODE_128");
    }

    #[test]
    fn format_filter_excludes_other_kinds() {
        let img = render_barcode("FILTER-ME", BarcodeFormat::QR_CODE, 240, 240);
        // 只找一维码 → 这张 QR 不该被认出来
        let hits = decode_barcodes(
            &img,
            &BarcodeOptions {
                formats: vec!["1d".to_string()],
                try_harder: true,
            },
        )
        .expect("识别应成功");
        assert!(hits.is_empty(), "限定一维码时不应识别出 QR，实际 {} 个", hits.len());

        // 只找二维码 → 命中
        let hits = decode_barcodes(
            &img,
            &BarcodeOptions {
                formats: vec!["二维码".to_string()],
                try_harder: true,
            },
        )
        .expect("识别应成功");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn unknown_format_name_is_error() {
        let img = render_barcode("X", BarcodeFormat::QR_CODE, 200, 200);
        let err = decode_barcodes(
            &img,
            &BarcodeOptions {
                formats: vec!["qrcod3".to_string()],
                try_harder: true,
            },
        )
        .expect_err("拼错的码制名应报错");
        assert!(err.contains("不认识的码制"), "实际：{err}");
    }

    #[test]
    fn blank_image_yields_no_hits() {
        let img = RgbImage::from_pixel(200, 200, image::Rgb([200u8, 200, 200]));
        let hits = decode_barcodes(&img, &BarcodeOptions::default()).expect("识别应成功");
        assert!(hits.is_empty(), "空白图不该命中");
    }

    #[test]
    fn tiny_image_is_rejected() {
        let img = RgbImage::new(4, 4);
        assert!(decode_barcodes(&img, &BarcodeOptions::default()).is_err());
    }
}
