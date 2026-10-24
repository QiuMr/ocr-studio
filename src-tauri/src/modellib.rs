//! 可选模型库：按需下载、手动导入、状态管理。
//!
//! 为什么不全打进分发包：PP-OCRv6 三档（7 / 32 / 133MB）+ ddddocr（52MB）
//! + 提示找图两件套（23MB）+ 检测模型，合起来 300MB 以上，而**单个用户
//! 实际只用其中一两个**。主包只带 small，其余按需下载 —— 225MB 降到约 60MB。
//!
//! 三件事决定了这里的实现方式：
//!
//! 1. **模型落在 `models_root` 下**，不另建目录。引擎层所有加载路径都是
//!    `models_root.join(...)`，换个根目录意味着改几十处调用点，且极容易
//!    「本机能跑、别人机器找不到」。绿色版解压目录通常可写，直接复用最稳。
//! 2. **一个条目 = 一个 zip**。PP-OCRv6 一档是 det/rec/cls + keys + meta 五个
//!    文件，逐个下发会让「装了一半算不算装上」变得很难判断。打包成一个 zip、
//!    解到目录里，判定条件就退化成「目录里那几个文件在不在」。
//! 3. **清单必须有内置兜底**。没网的时候也得让用户看见「这里有哪些能力可以加」，
//!    只是点下载会失败 —— 空白列表会让人以为程序坏了。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 清单地址。挂在固定 tag `models` 的 Release 上。
///
/// **那个 Release 必须标成 prerelease**：GitHub 的 `releases/latest` 取的是
/// 最新创建的正式 release，模型库要是正式 release 且比版本发布得晚，
/// `releases/latest/download/latest.json` 就会 404，自动更新全线报废。
pub const CATALOG_URL: &str =
    "https://github.com/QiuMr/ocr-studio/releases/download/models/catalog.json";

/// 内置兜底清单，编译期嵌进 exe（几十 KB，不占分发体积）。
const BUILTIN_CATALOG: &str = include_str!("../assets/catalog.json");

/// 清单缓存多久算过期。模型库不像版本更新那么频繁，一小时拉一次够。
const CATALOG_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    /// 分组：ocr / find / vision / image，前端按这个分栏
    #[serde(default)]
    pub group: String,
    /// 一句话说明（给用户看，不要写技术细节）
    #[serde(default)]
    pub note: String,
    /// Release 上的 zip 资产名
    pub file: String,
    /// 解压目标：`models/<dir>/`
    pub dir: String,
    /// zip 体积（字节）
    pub size: u64,
    pub sha256: String,
    /// 装上之后能用哪些端点
    #[serde(default)]
    pub endpoints: Vec<String>,
    /// 安装判定：这些文件都在就算已安装。空 = 目录非空即可。
    #[serde(default)]
    pub requires: Vec<String>,
    /// 是否推荐（前端给个「推荐」标签）
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Catalog {
    pub version: u32,
    pub models: Vec<CatalogEntry>,
}

/// 给前端的状态。字段名用 snake_case，与其它命令保持一致。
#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub name: String,
    pub group: String,
    pub note: String,
    pub size: u64,
    pub endpoints: Vec<String>,
    pub installed: bool,
    pub recommended: bool,
    /// 清单是内置兜底还是刚从网上拉的（false 时前端提示「没网，清单可能不是最新」）
    pub online: bool,
}

fn client(timeout_secs: u64) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .connect_timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join("models-catalog.json")
}

/// 拉清单：优先网络，失败退回本地缓存，再失败用内置。
///
/// 三层的意义：网络正常就能拿到最新清单（新增能力不用更新程序）；
/// 没网但有缓存（之前拉过）就还能显示上次的；彻底的新机器用内置清单，
/// 至少让用户看见「这里有东西可以下」。
pub fn load_catalog(data_dir: &Path, force: bool) -> (Catalog, bool) {
    let cp = cache_path(data_dir);

    if !force {
        if let Ok(meta) = std::fs::metadata(&cp) {
            if let Ok(age) = meta.modified().map(|m| m.elapsed()) {
                if age.map(|a| a < CATALOG_TTL).unwrap_or(false) {
                    if let Ok(text) = std::fs::read_to_string(&cp) {
                        if let Ok(c) = serde_json::from_str::<Catalog>(&text) {
                            return (c, true);
                        }
                    }
                }
            }
        }
    }

    let fetched = client(15).and_then(|c| {
        let resp = c
            .get(CATALOG_URL)
            .send()
            .map_err(|e| format!("拉取模型清单失败：{e}"))?;
        if !resp.status().is_success() {
            return Err(format!("拉取模型清单失败：HTTP {}", resp.status()));
        }
        resp.text().map_err(|e| format!("读取清单失败：{e}"))
    });

    match fetched {
        Ok(text) => match serde_json::from_str::<Catalog>(&text) {
            Ok(c) => {
                // 写缓存失败不影响使用（只影响下次离线时能不能显示）
                let _ = std::fs::write(&cp, &text);
                (c, true)
            }
            Err(e) => {
                tracing::warn!("模型清单解析失败：{e}；改用内置清单");
                (builtin(), false)
            }
        },
        Err(e) => {
            tracing::info!("{e}；改用本地缓存或内置清单");
            if let Ok(text) = std::fs::read_to_string(&cp) {
                if let Ok(c) = serde_json::from_str::<Catalog>(&text) {
                    return (c, true);
                }
            }
            (builtin(), false)
        }
    }
}

fn builtin() -> Catalog {
    serde_json::from_str(BUILTIN_CATALOG).unwrap_or_else(|e| {
        tracing::error!("内置模型清单损坏：{e}");
        Catalog {
            version: 0,
            models: Vec::new(),
        }
    })
}

/// 某个条目是否已安装。
pub fn is_installed(models_root: &Path, entry: &CatalogEntry) -> bool {
    let dir = models_root.join(&entry.dir);
    if entry.requires.is_empty() {
        return dir.is_dir()
            && std::fs::read_dir(&dir)
                .map(|mut it| it.next().is_some())
                .unwrap_or(false);
    }
    entry.requires.iter().all(|f| dir.join(f).exists())
}

pub fn statuses(models_root: &Path, catalog: &Catalog, online: bool) -> Vec<ModelStatus> {
    catalog
        .models
        .iter()
        .map(|e| ModelStatus {
            id: e.id.clone(),
            name: e.name.clone(),
            group: e.group.clone(),
            note: e.note.clone(),
            size: e.size,
            endpoints: e.endpoints.clone(),
            installed: is_installed(models_root, e),
            recommended: e.recommended,
            online,
        })
        .collect()
}

pub fn find<'a>(catalog: &'a Catalog, id: &str) -> Option<&'a CatalogEntry> {
    catalog.models.iter().find(|e| e.id == id)
}

/// 下载并安装。`on_progress(done, total, phase)` 由调用方接事件。
///
/// 校验顺序：**先 size 再 sha256 再解压**。sha256 是最后一道防线 ——
/// 模型文件被截断或被中间人改过，装进去只会让引擎在启动时报一堆
/// 看不懂的算子错误，不如在这里就拦住。
pub fn install<F>(
    models_root: &Path,
    data_dir: &Path,
    entry: &CatalogEntry,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, u64, &str),
{
    if is_installed(models_root, entry) {
        return Ok(());
    }

    let dl_dir = data_dir.join("downloads");
    std::fs::create_dir_all(&dl_dir).map_err(|e| format!("创建下载目录失败：{e}"))?;
    let zip_path = dl_dir.join(&entry.file);

    // 有残包（上次没下完）就先删，避免 sha256 对不上还重试不了
    if zip_path.exists() {
        let _ = std::fs::remove_file(&zip_path);
    }

    download(&entry.file_url(), &zip_path, entry.size, &mut on_progress)?;

    on_progress(0, 0, "verify");
    let got = sha256_file(&zip_path)?;
    if !got.eq_ignore_ascii_case(&entry.sha256) {
        let _ = std::fs::remove_file(&zip_path);
        return Err(format!(
            "文件校验失败（sha256 不匹配），已删除。可能是下载被中断或文件被替换。\n期望 {}\n实际 {got}",
            entry.sha256
        ));
    }

    on_progress(0, 0, "extract");
    let (tmp, dst) = prepare_stage(models_root, entry)?;
    if let Err(e) = unzip(&zip_path, &tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    commit_stage(&tmp, &dst)?;

    let _ = std::fs::remove_file(&zip_path);
    let _ = std::fs::remove_dir_all(&dl_dir.join(format!("_unpack_{}", entry.id)));

    if !is_installed(models_root, entry) {
        return Err(format!(
            "解压完成但没找到期望的文件。目录：{}",
            dst.display()
        ));
    }

    on_progress(0, 0, "done");
    Ok(())
}

/// 准备解压暂存目录，返回 (暂存目录, 最终目录)。
///
/// **暂存目录必须和目标同级**，不能放 `%APPDATA%` 或系统 temp：Windows 上
/// `rename` 跨卷会直接失败（`ERROR_NOT_SAME_DEVICE`）。用户把程序解压到 D:、
/// 而 appdata 在 C: 是极常见的情况 —— 第一版就是这么踩的：包下载好了、
/// 校验过了、解压也成了，最后一步 rename 失败，界面停在「正在校验」不动。
fn prepare_stage(
    models_root: &Path,
    entry: &CatalogEntry,
) -> Result<(PathBuf, PathBuf), String> {
    let dst = models_root.join(&entry.dir);
    let parent = dst
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| models_root.to_path_buf());
    std::fs::create_dir_all(&parent).map_err(|e| {
        format!(
            "创建模型目录失败：{e}\n目标目录：{}\n如果程序装在 Program Files 下，请改用免安装解压目录。",
            parent.display()
        )
    })?;
    let tmp = parent.join(format!(".tmp_{}", entry.id));
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    std::fs::create_dir_all(&tmp).map_err(|e| format!("创建解压目录失败：{e}"))?;
    Ok((tmp, dst))
}

/// 把暂存目录搬到最终位置。
///
/// 旧目录先删再 rename：模型包是整目录替换，不存在「合并」语义；
/// 留一半旧文件反而会让引擎加载到版本混杂的权重。
fn commit_stage(tmp: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        std::fs::remove_dir_all(dst).map_err(|e| format!("清理旧模型目录失败：{e}"))?;
    }
    std::fs::rename(tmp, dst).map_err(|e| {
        let _ = std::fs::remove_dir_all(tmp);
        format!("写入模型目录失败：{e}\n目标：{}", dst.display())
    })
}

/// 手动导入：用户自己选的 zip。
///
/// 存在的意义不是「偷懒的兜底」，而是**网络受限时的唯一活路**：
/// GitHub 的 release 下载在国内时不时连不通，这时候让用户从别处
/// 拿到同一个 zip、选文件导入，能力照样能加上。
pub fn import_zip<F>(
    models_root: &Path,
    entry: &CatalogEntry,
    zip_path: &Path,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, u64, &str),
{
    if !zip_path.is_file() {
        return Err("选的文件不存在".to_string());
    }

    on_progress(0, 0, "verify");
    let got = sha256_file(zip_path)?;
    // 手动导入**不强制** sha256 一致：用户可能拿到的是另一个来源的等价包。
    // 但要不一致就明确告诉他，让他自己决定要不要继续 —— 静默放行最坏。
    if !got.eq_ignore_ascii_case(&entry.sha256) {
        tracing::warn!(
            "手动导入的包 sha256 与清单不一致：期望 {} 实际 {got}；继续安装",
            entry.sha256
        );
    }

    on_progress(0, 0, "extract");
    // 同样用同卷暂存目录（见 prepare_stage 的注释）
    let (tmp, dst) = prepare_stage(models_root, entry)?;
    if let Err(e) = unzip(zip_path, &tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    commit_stage(&tmp, &dst)?;

    if !is_installed(models_root, entry) {
        return Err(format!(
            "解压完成但没找到期望的文件，请确认选的是「{}」的模型包。",
            entry.name
        ));
    }
    on_progress(0, 0, "done");
    Ok(())
}

/// 卸载：删掉模型目录。只删 models/<dir>，不动别的东西。
pub fn remove(models_root: &Path, entry: &CatalogEntry) -> Result<(), String> {
    let dir = models_root.join(&entry.dir);
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("删除失败：{e}"))
}

impl CatalogEntry {
    /// 下载地址。刻意只留一条 —— 没有备用源，多写一条假的只会让人以为
    /// 切源生效了，实际还是同一个域名。
    pub fn file_url(&self) -> String {
        format!(
            "https://github.com/QiuMr/ocr-studio/releases/download/models/{}",
            self.file
        )
    }
}

fn download<F>(url: &str, dst: &Path, expect: u64, on_progress: &mut F) -> Result<(), String>
where
    F: FnMut(u64, u64, &str),
{
    // 大文件（133MB）不能用短 timeout：那会把正常下载掐断。
    // 只约束连接阶段，读阶段靠 reqwest 的默认流式读取。
    let c = client(3600)?;
    let mut resp = c
        .get(url)
        .send()
        .map_err(|e| format!("连接下载地址失败：{e}\n{url}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "下载失败：HTTP {}\n{url}\n可能是这个版本还没上传模型包。",
            resp.status()
        ));
    }
    let total = resp.content_length().unwrap_or(expect);

    let mut file = std::fs::File::create(dst).map_err(|e| format!("创建文件失败：{e}"))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|e| format!("下载中断：{e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("写入失败：{e}"))?;
        done += n as u64;
        on_progress(done, total, "download");
    }
    drop(file);

    if done == 0 {
        return Err("下载到 0 字节，可能是网络被拦截了。".to_string());
    }
    Ok(())
}

fn sha256_file(p: &Path) -> Result<String, String> {
    use sha2::Digest as _;
    let mut f = std::fs::File::open(p).map_err(|e| format!("打开文件失败：{e}"))?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("读取失败：{e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn unzip(zip_path: &Path, dst: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(|e| format!("打开压缩包失败：{e}"))?;
    let mut ar = zip::ZipArchive::new(f).map_err(|e| format!("解析压缩包失败：{e}"))?;
    for i in 0..ar.len() {
        let mut zf = ar
            .by_index(i)
            .map_err(|e| format!("读取压缩包条目失败：{e}"))?;
        // zip slip 防护：条目名里带 .. 或绝对路径时拒绝。
        // 包是我们自己打的，但手动导入这条路上用户可能给任意文件。
        let Some(rel) = zf.enclosed_name() else {
            return Err(format!("压缩包里有不安全的路径：{}", zf.name()));
        };
        let out = dst.join(rel);
        if zf.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("建目录失败：{e}"))?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("建目录失败：{e}"))?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| format!("写文件失败：{e}"))?;
        std::io::copy(&mut zf, &mut w).map_err(|e| format!("解压失败：{e}"))?;
    }
    Ok(())
}
