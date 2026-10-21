//! 引擎目录发现：扫 `models/` 判断哪些引擎真的可用。
//!
//! 目的只有一个：让前端和 `/engine/list` 能拿到「本机到底能开哪些引擎」，
//! 而不是先调一遍再吃错误。**这里只读文件系统，不加载任何模型**——
//! 加载一次 det+rec 要几百毫秒到几秒、还要吃显存，绝不能用来做探测。
//!
//! 目录约定见 `docs/ENGINES.md` 第 3 节，新旧路径都认：
//!
//! ```text
//! models/
//!   ocr/ppocr/<level>/{det,rec,cls}.onnx + keys.txt   ← 新
//!   ocr/ddddocr/<variant>/{det,rec}.onnx + charset.txt ← 新
//!   detect/yolo/<version>/{model.onnx,labels.txt}      ← 新
//!   <tiny|small|medium>/...                            ← 旧路径，继续有效
//! ```

use std::path::{Path, PathBuf};

use super::{Capabilities, EngineSpec, EngineStatus, YoloVersion};
use crate::config::ModelLevel;

/// PP-OCR 档位模型目录。新路径优先，找不到 `det.onnx` 就回退旧路径。
///
/// 回退而不是报错，是为了让老用户的 `models/small/` 不用搬家——
/// 模型文件动辄 30MB，让人手动挪目录是没必要的摩擦。
pub fn ppocr_dir(root: &Path, level: ModelLevel) -> PathBuf {
    let new = root.join("ocr").join("ppocr").join(level.dir_name());
    if new.join("det.onnx").exists() {
        new
    } else {
        root.join(level.dir_name())
    }
}

pub fn ddddocr_dir(root: &Path, variant: &str) -> PathBuf {
    root.join("ocr").join("ddddocr").join(variant)
}

pub fn yolo_dir(root: &Path, version: YoloVersion) -> PathBuf {
    root.join("detect").join("yolo").join(version.as_str())
}

/// 读 `labels.txt`：每行一个类名，行号即 `class_id`。
///
/// 缺失或为空时返回空 vec，由调用方降级成 `class_{id}` 并打 warn——
/// 没有 labels 仍然可以检测，只是类别名不可读，不该因此判定引擎不可用。
pub fn load_labels(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    content
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// 扫描全部候选引擎。
pub fn discover(root: &Path) -> Vec<EngineStatus> {
    let mut out = Vec::new();

    // ---- PP-OCR：三档都列出来，缺文件的标 available=false 并说明缺什么 ----
    for level in [ModelLevel::Tiny, ModelLevel::Small, ModelLevel::Medium] {
        let dir = ppocr_dir(root, level);
        let has_det = dir.join("det.onnx").exists();
        let has_rec = dir.join("rec.onnx").exists();
        let has_keys = dir.join("keys.txt").exists();
        let has_cls = dir.join("cls.onnx").exists();

        let available = has_det && has_rec && has_keys;
        let note = if available {
            if has_cls {
                None
            } else {
                Some("缺少 cls.onnx，方向分类不可用".to_string())
            }
        } else {
            let mut miss = Vec::new();
            if !has_det {
                miss.push("det.onnx");
            }
            if !has_rec {
                miss.push("rec.onnx");
            }
            if !has_keys {
                miss.push("keys.txt");
            }
            Some(format!("{} 缺少 {}", dir.display(), miss.join(" / ")))
        };

        out.push(EngineStatus {
            spec: EngineSpec::ppocr(level),
            capabilities: Capabilities {
                line_boxes: available,
                char_boxes: available,
                rotation_cls: available && has_cls,
                detect: false,
                classes: Vec::new(),
                batch: false,
            },
            available,
            actual_device: None,
            note,
        });
    }

    // ---- DDDDOCR：common / old 两版 ----
    //
    // 只要求 rec.onnx + 字典：ddddocr 的 common/old 是**单行分类器**，自带的行检测
    // （common_det.onnx）是 YOLOX 目标检测，训的是验证码字符块，切文档截图很糟，
    // 所以本引擎的行检测复用 PP-OCR 的 det。det 缺失时仍算「可用」，
    // 但会真跑不起来——这一点由 note 和加载错误一起说清楚，见 ENGINE_MODELS_NOTE。
    for variant in ["common", "old"] {
        let dir = ddddocr_dir(root, variant);
        let has_rec = dir.join("rec.onnx").exists();
        let has_charset = ["charset.txt", "keys.txt", "dict.txt"]
            .iter()
            .any(|n| dir.join(n).exists());
        let has_shared_det = ppocr_dir(root, ModelLevel::default())
            .join("det.onnx")
            .exists();
        let available = has_rec && has_charset && has_shared_det;

        let note = if available {
            Some("单行识别引擎，行检测共用 PP-OCR det；中文/英文都支持，验证码更快".to_string())
        } else if !has_rec {
            Some(format!("{} 缺少 rec.onnx", dir.display()))
        } else if !has_charset {
            Some(format!("{} 缺少字典（charset.txt / keys.txt / dict.txt）", dir.display()))
        } else {
            Some(
                "缺少 PP-OCR 的 det.onnx：ddddocr 只做单行识别，需要它来找文本行".to_string(),
            )
        };

        out.push(EngineStatus {
            spec: EngineSpec::ddddocr(variant),
            capabilities: Capabilities {
                line_boxes: available,
                // 与 PP-OCR 一样来自 CTC 帧区间，不是等分估计
                char_boxes: available,
                rotation_cls: false,
                detect: false,
                classes: Vec::new(),
                batch: false,
            },
            available,
            actual_device: None,
            note,
        });
    }

    // ---- YOLO：v5 / v7 / v8 / v11 ----
    for ver in [
        YoloVersion::V5,
        YoloVersion::V7,
        YoloVersion::V8,
        YoloVersion::V11,
    ] {
        let dir = yolo_dir(root, ver);
        let model = dir.join("model.onnx");
        let available = model.exists();
        let classes = if available {
            let labels = load_labels(&dir.join("labels.txt"));
            if labels.is_empty() {
                tracing::warn!(
                    "{} 缺少 labels.txt，类别名将显示为 class_{{id}}",
                    dir.display()
                );
            }
            labels
        } else {
            Vec::new()
        };

        out.push(EngineStatus {
            spec: EngineSpec::yolo(ver),
            capabilities: Capabilities {
                line_boxes: false,
                char_boxes: false,
                rotation_cls: false,
                detect: available,
                classes,
                batch: false,
            },
            available,
            actual_device: None,
            note: if available {
                None
            } else {
                Some(format!("{} 缺少 model.onnx", dir.display()))
            },
        });
    }

    out
}

/// 只返回可用的引擎。
pub fn discover_available(root: &Path) -> Vec<EngineStatus> {
    discover(root).into_iter().filter(|e| e.available).collect()
}

/// 按 spec 找模型目录（供 M3.1+ 的加载逻辑用）。
pub fn model_dir(root: &Path, spec: &EngineSpec) -> PathBuf {
    let variant = spec.variant.as_deref();
    match &spec.kind {
        // 档位名解析失败时退回默认档（small），与 `/ocr/*` 缺省行为一致
        super::EngineKind::PpOcr => ppocr_dir(
            root,
            variant.and_then(ModelLevel::from_str).unwrap_or_default(),
        ),
        super::EngineKind::DdddOcr => ddddocr_dir(root, variant.unwrap_or("common")),
        super::EngineKind::Yolo(ver) => yolo_dir(root, *ver),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 在临时目录里搭一套假模型文件，验证发现逻辑。
    fn touch(p: &Path) {
        if let Some(d) = p.parent() {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(p, b"stub").unwrap();
    }

    #[test]
    fn discover_reports_legacy_ppocr_path() {
        let dir = std::env::temp_dir().join(format!("ocr_eng_legacy_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        touch(&dir.join("small").join("det.onnx"));
        touch(&dir.join("small").join("rec.onnx"));
        touch(&dir.join("small").join("keys.txt"));

        let list = discover(&dir);
        let small = list
            .iter()
            .find(|e| e.spec.key() == "ppocr:small")
            .expect("应列出 ppocr:small");
        assert!(small.available);
        // 没有 cls.onnx → 方向分类不可用，且 note 要说清楚
        assert!(!small.capabilities.rotation_cls);
        assert!(small.note.as_deref().unwrap_or("").contains("cls.onnx"));

        // 空目录的档位必须标成不可用
        let tiny = list.iter().find(|e| e.spec.key() == "ppocr:tiny").unwrap();
        assert!(!tiny.available);
        assert!(!tiny.capabilities.line_boxes);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_path_wins_over_legacy() {
        let dir = std::env::temp_dir().join(format!("ocr_eng_new_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // 旧路径有 det，新路径也有 det —— 应优先新路径
        touch(&dir.join("small").join("det.onnx"));
        touch(&dir.join("ocr").join("ppocr").join("small").join("det.onnx"));

        let got = ppocr_dir(&dir, ModelLevel::Small);
        assert_eq!(
            got,
            dir.join("ocr").join("ppocr").join("small"),
            "新路径存在时必须优先"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yolo_labels_become_capabilities() {
        let dir = std::env::temp_dir().join(format!("ocr_eng_yolo_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let v8 = dir.join("detect").join("yolo").join("v8");
        touch(&v8.join("model.onnx"));
        fs::write(v8.join("labels.txt"), "person\r\ncar\r\n\r\nbike\r\n").unwrap();

        let list = discover(&dir);
        let y = list
            .iter()
            .find(|e| e.spec.key() == "yolo:v8")
            .expect("应列出 yolo:v8");
        assert!(y.available);
        assert!(y.capabilities.detect);
        // 空行被过滤，CRLF 被去掉
        assert_eq!(y.capabilities.classes, vec!["person", "car", "bike"]);
        // 其他版本不可用
        let v5 = list.iter().find(|e| e.spec.key() == "yolo:v5").unwrap();
        assert!(!v5.available);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fallback_to_class_id_when_labels_missing() {
        let dir = std::env::temp_dir().join(format!("ocr_eng_nolabel_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let v8 = dir.join("detect").join("yolo").join("v8");
        touch(&v8.join("model.onnx"));

        let list = discover(&dir);
        let y = list.iter().find(|e| e.spec.key() == "yolo:v8").unwrap();
        // 没有 labels 只是类别名不可读，不该让引擎不可用
        assert!(y.available);
        assert!(y.capabilities.detect);
        assert!(y.capabilities.classes.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }
}
