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
//!   detect/yolo/<任意名>.onnx                          ← 检测模型，**出厂为空**
//!   detect/yolo/<子目录>/<任意名>.onnx                  ← 兼容旧的 v8/ 分层
//!   <tiny|small|medium>/...                            ← 旧路径，继续有效
//! ```
//!
//! 目标检测的模型是**用户自己的**：出厂不预置任何检测模型（这是「模型工作台」，
//! 不是「送你一个模型」）。用户既可以往 `detect/yolo/` 丢多个 onnx，也可以在
//! 设置里「添加模型」直接指向任意路径（存在 `config.json` 的 `detect_models` 里，
//! 不复制文件）。两条来源在 [`list_yolo_models`] 里合并；**列表第一个即默认模型**。

use std::path::{Path, PathBuf};

use super::{Capabilities, EngineSpec, EngineStatus};
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

/// 目标检测模型的**约定目录**（不一定真的有文件，见 [`yolo_model_dir`]）。
pub fn yolo_dir(root: &Path) -> PathBuf {
    root.join("detect").join("yolo")
}

/// 一个可用的目标检测模型。
///
/// `id` 就是引擎标识 `yolo:<id>` 的后半段，也是下拉里显示的名字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YoloModel {
    pub id: String,
    /// onnx 的绝对路径（外部模型可能在任意目录）
    pub path: PathBuf,
    /// true = 用户在设置里加的外部路径；false = `detect/yolo/` 下扫到的
    pub external: bool,
}

impl YoloModel {
    /// 类名的兜底来源：与模型同目录的 `labels.txt`。
    ///
    /// 首选来源是 ONNX 自带的 metadata（ultralytics 导出会写入 `names`），
    /// 只有拿不到时才回落这里，见 `yolo::ModelMeta`。
    pub fn labels_path(&self) -> PathBuf {
        self.path.with_file_name("labels.txt")
    }

    /// 另一种常见命名：`mymodel.onnx` 配 `mymodel.txt`。
    pub fn stem_labels_path(&self) -> PathBuf {
        self.path.with_extension("txt")
    }
}

fn is_onnx(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("onnx"))
}

/// 扫描 `detect/yolo/` 下所有 onnx，再并上配置里的外部路径。
///
/// 顺序即优先级：先同级 onnx（按文件名排序，顺序稳定），再一级子目录里的 onnx
/// （兼容旧的 `v8/` 分层），最后是外部路径。**列表第一个就是「默认模型」**
/// （`engine` 参数不写 id 时用它）——出厂时这个目录是空的，所以默认模型完全由
/// 用户决定。同一个 id 撞车时依次尝试「加父目录名」「加序号」，保证选项不重复。
pub fn list_yolo_models(root: &Path, external: &[String]) -> Vec<YoloModel> {
    let dir = yolo_dir(root);
    let mut paths: Vec<PathBuf> = Vec::new();

    // 同级所有 onnx + 一级子目录里的 onnx（旧的 detect/yolo/v8/model.onnx）。
    // 这里**不预置**任何模型：没有文件就是没有检测引擎，由前端给空状态引导。
    let mut same_level: Vec<PathBuf> = Vec::new();
    let mut nested: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && is_onnx(&p) {
                same_level.push(p);
            } else if p.is_dir() {
                if let Ok(inner) = std::fs::read_dir(&p) {
                    nested.extend(
                        inner
                            .flatten()
                            .map(|e| e.path())
                            .filter(|q| q.is_file() && is_onnx(q)),
                    );
                }
            }
        }
    }
    same_level.sort();
    nested.sort();
    paths.extend(same_level);
    paths.extend(nested);

    let mut out: Vec<YoloModel> = Vec::with_capacity(paths.len() + external.len());
    let mut used: Vec<String> = Vec::new();

    for path in paths {
        let id = unique_id(&path, &used, false);
        used.push(id.clone());
        out.push(YoloModel {
            id,
            path,
            external: false,
        });
    }
    for raw in external {
        let p = PathBuf::from(raw.trim());
        if !p.is_file() {
            // 外部路径可能被用户挪走/删掉。只提示，不影响其它引擎可用。
            tracing::warn!("自定义检测模型不存在，已跳过: {}", p.display());
            continue;
        }
        if out.iter().any(|m| m.path == p) {
            continue; // 同一份文件既在目录里又被手动加过，只留一条
        }
        let id = unique_id(&p, &used, true);
        used.push(id.clone());
        out.push(YoloModel {
            id,
            path: p,
            external: true,
        });
    }
    out
}

/// 生成不冲突的模型 id。
///
/// 优先用文件名（`yolov8n.onnx` → `yolov8n`）；撞车时退到「父目录名/文件名」，
/// 再撞就加序号。外部模型还要避开「看起来像版本号」的 id——那种 id 会被
/// `EngineSpec::parse` 当成旧的 `yolo:v8` 写法吃掉。
fn unique_id(path: &Path, used: &[String], external: bool) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let mut candidates: Vec<String> = Vec::new();
    // 子目录里的 `model.onnx` 用父目录名当 id（`v8/model.onnx` → `v8`），
    // 比 `v8/model` 好读，也是旧结构自然该有的名字
    if stem == "model" && !parent.is_empty() && parent != "yolo" {
        candidates.push(parent.to_string());
    }
    candidates.push(stem.clone());
    if !parent.is_empty() && parent != "yolo" {
        candidates.push(format!("{parent}/{stem}"));
    }
    for c in &candidates {
        if !used.contains(c) && !(external && super::looks_like_yolo_version(c)) {
            return c.clone();
        }
    }
    let base = candidates.last().cloned().unwrap_or(stem);
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !used.contains(c))
        .unwrap()
}

/// 默认模型：列表里的第一个。`None` 表示一个都没有。
pub fn default_yolo_model(root: &Path, external: &[String]) -> Option<YoloModel> {
    list_yolo_models(root, external).into_iter().next()
}

/// 按 id 找模型；`id = None` 表示要默认那个。
///
/// 找不到时返回 `None`，由调用方组织报错（这样才能把可选 id 列给用户）。
pub fn find_yolo_model(root: &Path, external: &[String], id: Option<&str>) -> Option<YoloModel> {
    let list = list_yolo_models(root, external);
    match id {
        None => list.into_iter().next(),
        Some(want) => list.into_iter().find(|m| m.id == want),
    }
}

/// 默认检测模型所在的目录（兼容旧调用点；没有模型时返回约定目录）。
pub fn yolo_model_dir(root: &Path) -> PathBuf {
    default_yolo_model(root, &[])
        .and_then(|m| m.path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| yolo_dir(root))
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
///
/// `yolo_external` 是配置里「添加模型」记下的外部 onnx 路径——检测模型可由用户
/// 指向任意位置，所以发现阶段就把它并进来，`/engine/list` 才能列全。
pub fn discover(root: &Path, yolo_external: &[String]) -> Vec<EngineStatus> {
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

    // ---- 目标检测：一个模型一条，不按版本分 ----
    //
    // 版本差异（输出布局 / objectness / 输入边长）在加载时按 ONNX 形状探测，
    // 所以这里的发现逻辑只关心「有哪些 onnx」。类名同样在加载时从模型 metadata
    // 读（没加载就不读，避免为了列个清单去开一遍会话），这里只用 labels.txt 兜底。
    let convention = yolo_dir(root);
    let models = list_yolo_models(root, yolo_external);
    if models.is_empty() {
        out.push(EngineStatus {
            spec: EngineSpec::yolo(),
            capabilities: Capabilities::default(),
            available: false,
            actual_device: None,
            note: Some(format!(
                "{} 还没有模型。放一个 ultralytics 导出的单输出 ONNX（或点「添加模型」\
                 指向任意位置的 onnx），类名会自动从模型里读，不用写 labels.txt",
                convention.display()
            )),
        });
        return out;
    }
    for (i, m) in models.into_iter().enumerate() {
        let classes = load_labels(&m.labels_path());
        let classes = if classes.is_empty() {
            load_labels(&m.stem_labels_path())
        } else {
            classes
        };
        let mut note = if classes.is_empty() {
            format!("{}（类名在加载时从模型元数据读取）", m.path.display())
        } else {
            // 不写死「labels.txt」：也可能是同名的 `<模型名>.txt`（stem 兜底）。
            // 说清来源文件反而更啰嗦，用户关心的是「有几个类」。
            format!(
                "{}（类名来自标签文件，共 {} 个）",
                m.path.display(),
                classes.len()
            )
        };
        if m.external {
            note.push_str(" · 外部模型");
        }
        if i == 0 {
            note.push_str(" · 默认");
        }
        out.push(EngineStatus {
            // 第一个就是默认模型，用不带 id 的 `yolo`——前端下拉的缺省值、
            // `/vision/pipeline` 的 `detect_engine` 缺省值都是它
            spec: if i == 0 {
                EngineSpec::yolo()
            } else {
                EngineSpec::yolo_model(&m.id)
            },
            capabilities: Capabilities {
                line_boxes: false,
                char_boxes: false,
                rotation_cls: false,
                detect: true,
                classes,
                batch: false,
            },
            available: true,
            actual_device: None,
            note: Some(note),
        });
    }

    out
}

/// 只返回可用的引擎。
pub fn discover_available(root: &Path, yolo_external: &[String]) -> Vec<EngineStatus> {
    discover(root, yolo_external)
        .into_iter()
        .filter(|e| e.available)
        .collect()
}

/// 按 spec 找模型目录（供 M3.1+ 的加载逻辑用）。
pub fn model_dir(root: &Path, spec: &EngineSpec, yolo_external: &[String]) -> PathBuf {
    let variant = spec.variant.as_deref();
    match &spec.kind {
        // 档位名解析失败时退回默认档（small），与 `/ocr/*` 缺省行为一致
        super::EngineKind::PpOcr => ppocr_dir(
            root,
            variant.and_then(ModelLevel::from_str).unwrap_or_default(),
        ),
        super::EngineKind::DdddOcr => ddddocr_dir(root, variant.unwrap_or("common")),
        super::EngineKind::Yolo => find_yolo_model(root, yolo_external, variant)
            .and_then(|m| m.path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| yolo_dir(root)),
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

        let list = discover(&dir, &[]);
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
    fn yolo_lists_every_model_not_one_per_version() {
        let dir = std::env::temp_dir().join(format!("ocr_eng_yolo_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let y = dir.join("detect").join("yolo");
        // 同级两个模型 + 旧的分版本子目录
        touch(&y.join("model.onnx"));
        touch(&y.join("custom.onnx"));
        touch(&y.join("v8").join("model.onnx"));
        fs::write(y.join("labels.txt"), "person\r\ncar\r\n\r\nbike\r\n").unwrap();

        let list = discover(&dir, &[]);
        let dets: Vec<&EngineStatus> = list
            .iter()
            .filter(|e| e.spec.kind.family() == "yolo")
            .collect();
        // 一个文件一条，但**不许出现按版本分裂的 v5/v8/v11**——名字来自文件名/目录名
        assert_eq!(dets.len(), 3, "三个 onnx 应各列一条");
        // 顺序即优先级：同级按文件名排序，第一个就是默认模型（出厂目录是空的，
        // 所以这里没有任何「约定的默认文件」）
        assert_eq!(dets[0].spec.key(), "yolo"); // custom.onnx（字典序在前）
        assert_eq!(dets[1].spec.key(), "yolo:model");
        assert_eq!(dets[2].spec.key(), "yolo:v8");
        assert!(dets.iter().all(|d| d.available && d.capabilities.detect));
        // 空行被过滤，CRLF 被去掉（labels.txt 是兜底来源）
        assert_eq!(dets[0].capabilities.classes, vec!["person", "car", "bike"]);
        // v8 目录下没有自己的 labels.txt
        assert!(dets[2].capabilities.classes.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yolo_empty_dir_means_no_engine() {
        // 出厂状态：detect/yolo/ 里一个 onnx 都没有 → 不该凭空造出一个检测引擎，
        // 更不该有「默认模型」。前端据此给空状态引导。
        let dir = std::env::temp_dir().join(format!("ocr_eng_yoloempty_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        touch(&dir.join("detect").join("yolo").join("README.txt"));

        assert!(list_yolo_models(&dir, &[]).is_empty());
        assert!(default_yolo_model(&dir, &[]).is_none());
        let y = discover(&dir, &[])
            .into_iter()
            .find(|e| e.spec.kind.family() == "yolo")
            .unwrap();
        assert!(!y.available, "没有模型时必须标成不可用");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yolo_model_ids_never_collide() {
        let dir = std::env::temp_dir().join(format!("ocr_eng_yoloid_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let y = dir.join("detect").join("yolo");
        // 两个同名 a.onnx 在两个不同子目录里
        touch(&y.join("x").join("a.onnx"));
        touch(&y.join("y").join("a.onnx"));

        let ids: Vec<String> = list_yolo_models(&dir, &[])
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "id 撞车会被缓存键覆盖");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yolo_external_model_paths_are_listed() {
        let dir = std::env::temp_dir().join(format!("ocr_eng_yoloext_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ext = std::env::temp_dir().join(format!("my_plate_model_{}.onnx", std::process::id()));
        fs::write(&ext, b"stub").unwrap();

        let models = list_yolo_models(&dir, &[ext.display().to_string()]);
        let m = models.iter().find(|m| m.external).expect("外部模型应被列出");
        assert_eq!(m.id, format!("my_plate_model_{}", std::process::id()));
        assert_eq!(m.path, ext);

        // 文件被删掉后只跳过，不 panic
        let missing = list_yolo_models(&dir, &["D:/definitely/not/here.onnx".to_string()]);
        assert!(missing.is_empty());

        let _ = fs::remove_file(&ext);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yolo_external_id_avoids_version_lookalike() {
        // 外部文件叫 v8.onnx 时，id 不能是 "v8"——那会被 parse 当成旧写法 yolo:v8
        let dir = std::env::temp_dir().join(format!("ocr_eng_yolover_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ext_dir = std::env::temp_dir().join(format!("yolo_ver_{}", std::process::id()));
        fs::create_dir_all(&ext_dir).unwrap();
        let ext = ext_dir.join("v8.onnx");
        fs::write(&ext, b"stub").unwrap();

        let models = list_yolo_models(&dir, &[ext.display().to_string()]);
        assert_eq!(models.len(), 1);
        assert_ne!(models[0].id, "v8");

        let _ = fs::remove_file(&ext);
        let _ = fs::remove_dir_all(&ext_dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yolo_missing_model_explains_where_to_put_it() {
        let dir = std::env::temp_dir().join(format!("ocr_eng_noyolo_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let y = discover(&dir, &[])
            .into_iter()
            .find(|e| e.spec.kind.family() == "yolo")
            .unwrap();
        assert!(!y.available);
        let note = y.note.unwrap_or_default();
        // 提示要说清「放哪」和「不用写 labels.txt」
        assert!(note.contains("detect"), "{note}");
        assert!(note.contains("labels.txt"), "{note}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_yolo_model_resolves_default_and_by_id() {
        let dir = std::env::temp_dir().join(format!("ocr_eng_yolofind_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let y = dir.join("detect").join("yolo");
        touch(&y.join("model.onnx"));
        touch(&y.join("plate.onnx"));

        assert_eq!(find_yolo_model(&dir, &[], None).unwrap().id, "model");
        assert_eq!(find_yolo_model(&dir, &[], Some("plate")).unwrap().id, "plate");
        // 不存在的 id 返回 None，让上层能列出可选项
        assert!(find_yolo_model(&dir, &[], Some("nope")).is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
