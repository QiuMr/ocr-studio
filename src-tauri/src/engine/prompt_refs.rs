//! 中文视觉参考库：把「一个中文名字」绑到「一块参考图区域」。
//!
//! 为什么需要它
//! ------------
//! 语言找图的文本塔是英文语义空间，中文只能靠 `zh_words` 词表映射 ——
//! 词表覆盖不到就找不了（比如自家 App 里那个叫「提交按钮」的东西，
//! 全世界没有第二个长得一样的，CLIP 词表里也不可能有）。
//!
//! 参考库绕开了文本塔：用户**框一次**目标、起个中文名，以后按中文名调用时走
//! **图找图那条通道**（VPE 视觉提示编码器）。好处有三：
//!
//! 1. 任意中文名都能用，不受词表限制；
//! 2. 不依赖那 242MB 文本塔 —— 只有图找图三件套的机器也能用中文；
//! 3. 对「自家界面里的特定元素」比文本提示准得多（它看的是长得像不像，
//!    不是语义像不像）。
//!
//! 存哪儿
//! ------
//! `<应用数据目录>/prompt_refs/`：
//! - `refs.json` 索引（名字 / 图片文件名 / 框 / 尺寸）
//! - `ref_<毫秒>.png` 参考图
//!
//! 存**整张参考图**而不是裁剪后的小块：VPE 的输入是整图 letterbox 到 640 再
//! 叠框掩码，裁掉上下文会让提示变差。

use std::path::{Path, PathBuf};

use image::DynamicImage;
use serde::{Deserialize, Serialize};

use super::VisionError;

/// 一条参考。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefEntry {
    /// 中文名（用户起的，调用时就用它）。
    pub name: String,
    /// 参考图文件名（相对本目录）。
    pub image: String,
    /// 参考框，**参考图原图坐标** `[x1,y1,x2,y2]`。
    pub rect: [f32; 4],
    /// 参考图尺寸，给列表展示用。
    pub width: u32,
    pub height: u32,
    /// 创建时间（本地时间字符串，给人看）。
    pub created: String,
}

/// 参考库。整个服务一份，读写都在 `AppState` 里串行化。
#[derive(Debug, Default)]
pub struct RefStore {
    dir: PathBuf,
    entries: Vec<RefEntry>,
}

impl RefStore {
    /// 从目录加载。目录不存在就当作空库（不报错 —— 没教过就是没有）。
    pub fn load(dir: &Path) -> Self {
        let index = dir.join("refs.json");
        let entries = match std::fs::read_to_string(&index) {
            Ok(s) => serde_json::from_str::<Vec<RefEntry>>(&s).unwrap_or_else(|e| {
                tracing::warn!("参考库索引 {} 解析失败，按空库处理：{e}", index.display());
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Self {
            dir: dir.to_path_buf(),
            entries,
        }
    }

    /// 目录（新建参考时用）。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn list(&self) -> &[RefEntry] {
        &self.entries
    }

    /// 按名字查。名字是唯一的（新增时同名覆盖）。
    pub fn get(&self, name: &str) -> Option<&RefEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// 参考图的绝对路径。
    pub fn image_path(&self, e: &RefEntry) -> PathBuf {
        self.dir.join(&e.image)
    }

    /// 读出参考图。
    pub fn load_image(&self, e: &RefEntry) -> Result<DynamicImage, VisionError> {
        let p = self.image_path(e);
        image::open(&p).map_err(|err| {
            VisionError::BadRequest(format!(
                "参考「{}」的图片读不出来（{}）：{err}",
                e.name,
                p.display()
            ))
        })
    }

    /// 新增（同名覆盖）。`img` 是**整张参考图**，`rect` 是它上面的框。
    pub fn add(
        &mut self,
        name: &str,
        img: &DynamicImage,
        rect: [f32; 4],
    ) -> Result<RefEntry, VisionError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(VisionError::BadRequest("参考名不能为空".into()));
        }
        let (w, h) = (img.width(), img.height());
        if w == 0 || h == 0 {
            return Err(VisionError::BadRequest("参考图是空的".into()));
        }
        let rect = clamp_rect(rect, w, h)?;

        std::fs::create_dir_all(&self.dir).map_err(|e| {
            VisionError::BadRequest(format!(
                "建不了参考库目录 {}：{e}",
                self.dir.display()
            ))
        })?;

        // 同名覆盖：先删掉老条目（老图片文件留着，下次清理 —— 直接删有风险，
        // 万一有别的条目引用了同一张图）。
        let old = self.entries.iter().find(|e| e.name == name).cloned();
        if let Some(old) = &old {
            self.entries.retain(|e| e.name != name);
            let _ = std::fs::remove_file(self.dir.join(&old.image));
        }

        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let file = format!("ref_{ms}.png");
        let path = self.dir.join(&file);
        img.save(&path).map_err(|e| {
            VisionError::BadRequest(format!("参考图存不下来（{}）：{e}", path.display()))
        })?;

        let entry = RefEntry {
            name: name.to_string(),
            image: file,
            rect,
            width: w,
            height: h,
            created: now_string(),
        };
        self.entries.push(entry.clone());
        self.save()?;
        Ok(entry)
    }

    /// 按名字删一条（连图片一起）。不存在返回 false。
    pub fn remove(&mut self, name: &str) -> Result<bool, VisionError> {
        let name = name.trim();
        let Some(pos) = self.entries.iter().position(|e| e.name == name) else {
            return Ok(false);
        };
        let e = self.entries.remove(pos);
        let _ = std::fs::remove_file(self.dir.join(&e.image));
        self.save()?;
        Ok(true)
    }

    fn save(&self) -> Result<(), VisionError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| {
            VisionError::BadRequest(format!("建不了参考库目录 {}：{e}", self.dir.display()))
        })?;
        let index = self.dir.join("refs.json");
        // 原子写：先写临时文件再改名，写一半崩了也不会把整个库弄坏
        let tmp = self.dir.join("refs.json.tmp");
        let s = serde_json::to_string_pretty(&self.entries)
            .map_err(|e| VisionError::BadRequest(format!("参考库序列化失败: {e}")))?;
        std::fs::write(&tmp, s).map_err(|e| {
            VisionError::BadRequest(format!("参考库写不下来（{}）：{e}", tmp.display()))
        })?;
        std::fs::rename(&tmp, &index).map_err(|e| {
            VisionError::BadRequest(format!("参考库索引改名失败：{e}"))
        })?;
        Ok(())
    }
}

/// 框夹进图片范围，并检查非空。
fn clamp_rect(r: [f32; 4], w: u32, h: u32) -> Result<[f32; 4], VisionError> {
    let (w, h) = (w as f32, h as f32);
    let mut x1 = r[0].clamp(0.0, w);
    let mut y1 = r[1].clamp(0.0, h);
    let mut x2 = r[2].clamp(0.0, w);
    let mut y2 = r[3].clamp(0.0, h);
    if x2 < x1 {
        std::mem::swap(&mut x1, &mut x2);
    }
    if y2 < y1 {
        std::mem::swap(&mut y1, &mut y2);
    }
    if (x2 - x1) < 2.0 || (y2 - y1) < 2.0 {
        return Err(VisionError::BadRequest(format!(
            "参考框太小了（{x1},{y1},{x2},{y2}），至少要有 2x2 像素"
        )));
    }
    Ok([x1, y1, x2, y2])
}

fn now_string() -> String {
    // 不引 chrono：只要一个能给人看的时间串
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ocr_refs_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn img(w: u32, h: u32) -> DynamicImage {
        DynamicImage::new_rgb8(w, h)
    }

    #[test]
    fn add_then_get_then_remove() {
        let dir = tmp("basic");
        let mut s = RefStore::load(&dir);
        assert!(s.list().is_empty());

        let e = s.add("提交按钮", &img(80, 40), [10.0, 10.0, 50.0, 30.0]).unwrap();
        assert_eq!(e.name, "提交按钮");
        assert_eq!(s.list().len(), 1);
        assert!(dir.join(&e.image).is_file(), "参考图要真的落盘");

        // 重新加载能读回来
        let s2 = RefStore::load(&dir);
        assert_eq!(s2.list().len(), 1);
        assert_eq!(s2.get("提交按钮").unwrap().rect, [10.0, 10.0, 50.0, 30.0]);

        assert!(s.remove("提交按钮").unwrap());
        assert!(!s.remove("提交按钮").unwrap());
        assert!(s.list().is_empty());
    }

    #[test]
    fn same_name_overwrites() {
        let dir = tmp("overwrite");
        let mut s = RefStore::load(&dir);
        s.add("按钮", &img(40, 40), [1.0, 1.0, 20.0, 20.0]).unwrap();
        s.add("按钮", &img(40, 40), [5.0, 5.0, 30.0, 30.0]).unwrap();
        assert_eq!(s.list().len(), 1, "同名不能变成两条");
        assert_eq!(s.get("按钮").unwrap().rect, [5.0, 5.0, 30.0, 30.0]);
    }

    #[test]
    fn rect_is_clamped_and_validated() {
        let dir = tmp("clamp");
        let mut s = RefStore::load(&dir);
        let e = s
            .add("超框", &img(50, 50), [-20.0, 0.0, 999.0, 40.0])
            .unwrap();
        assert_eq!(e.rect, [0.0, 0.0, 50.0, 40.0]);
        assert!(s.add("太小", &img(50, 50), [1.0, 1.0, 1.5, 9.0]).is_err());
        // 坐标反过来也能用
        let e = s.add("反的", &img(50, 50), [30.0, 30.0, 10.0, 10.0]).unwrap();
        assert_eq!(e.rect, [10.0, 10.0, 30.0, 30.0]);
    }

    #[test]
    fn empty_dir_is_empty_store() {
        let dir = tmp("missing");
        let s = RefStore::load(&dir);
        assert!(s.list().is_empty(), "目录不存在不该报错");
    }
}
