//! 程序版本号。
//!
//! **唯一来源是 `src-tauri/tauri.conf.json` 的 `version`** —— `build.rs` 把它
//! 读成 `OCR_STUDIO_VERSION` 环境变量注入进来。发版只改那一个地方即可：
//!
//! - `pack_exe.ps1` 从同一个文件读，拼 zip 文件名
//! - `scripts/release.ps1` 从同一个文件读，打 GitHub Release 的 tag
//! - 更新检查拿 [`VERSION`] 跟远端 `latest.json` 比
//!
//! 为什么不用 `env!("CARGO_PKG_VERSION")`：那个读的是 `Cargo.toml`，
//! 跟 Tauri 的配置是两份，天生会漂。

/// 当前编译进去的版本，如 `"0.1.0"`。
pub const VERSION: &str = env!("OCR_STUDIO_VERSION");

/// 语义化版本比较：`a` 比 `b` 新返回 `Ordering::Greater`。
///
/// 只比 `major.minor.patch` 三段数字，忽略 `-beta` 这类预发布后缀
/// （本项目不用预发布通道；真要用时这里得补 `-` 的排序规则，
/// 而现在补是过度设计）。段数不齐按 0 补。
///
/// **不要用字符串比较**：`"0.10.0" < "0.9.0"` 在字典序上是成立的，
/// 版本号这么比会漏掉更新——这是最典型的一类更新器 bug。
pub fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(s: &str) -> Vec<u64> {
        // `1.2.3-beta.4` → 只看 `1.2.3`
        let core = s.split(['-', '+']).next().unwrap_or("");
        core.split('.')
            .map(|p| p.trim().parse::<u64>().unwrap_or(0))
            .collect()
    }
    let (pa, pb) = (parts(a), parts(b));
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn version_is_not_empty() {
        assert!(!VERSION.is_empty(), "版本号不该为空");
        assert!(VERSION.contains('.'), "版本号至少要有两段: {VERSION}");
    }

    #[test]
    fn cmp_version_handles_double_digit() {
        // 字典序会在这里翻错，必须是数值比较
        assert_eq!(cmp_version("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(cmp_version("0.9.0", "0.10.0"), Ordering::Less);
        assert_eq!(cmp_version("10.0.0", "9.9.9"), Ordering::Greater);
    }

    #[test]
    fn cmp_version_equal_and_padding() {
        assert_eq!(cmp_version("1.2.3", "1.2.3"), Ordering::Equal);
        // 段数不齐按 0 补
        assert_eq!(cmp_version("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(cmp_version("1.2.1", "1.2"), Ordering::Greater);
    }

    #[test]
    fn cmp_version_ignores_prerelease_suffix() {
        // 当前策略：不区分预发布，`1.0.0-beta` 等同 `1.0.0`
        assert_eq!(cmp_version("1.0.0-beta", "1.0.0"), Ordering::Equal);
    }
}
