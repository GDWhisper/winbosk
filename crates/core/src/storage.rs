//! 栅栏「文件位置」（存储）的纯展示模型。
//!
//! 把 [`crate::model::Fence::storage_path`] 归约为控制中心可直接渲染的三元组
//! （模式 / 真实落地路径 / 是否可回退），并提供路径中段省略。
//!
//! 本模块**零 OS 依赖**、纯函数，可内存单元测试。

use crate::text::estimate_width;

/// 栅栏文件落地模式。
///
/// 两种模式是**机制差异**而非程度差异：前者栅栏索引内部库里的副本，
/// 后者栅栏与磁盘目录互为镜像。控制中心必须让用户能分辨。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    /// `storage_path == None`：应用数据目录下的共享 `library`（栅栏索引其副本）。
    AppLibrary,
    /// `storage_path == Some(dir)`：与磁盘目录双向镜像（栅栏即文件夹）。
    ExternalFolder,
}

impl StorageKind {
    /// 控制中心状态芯片文案。
    pub fn badge(self) -> &'static str {
        match self {
            StorageKind::AppLibrary => "应用内部",
            StorageKind::ExternalFolder => "外部文件夹",
        }
    }
}

/// 控制中心「文件位置」行的渲染数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageInfo {
    pub kind: StorageKind,
    /// 真实落地目录（绝对路径）。内部模式 = 共享库目录。
    pub path: String,
    /// 是否允许「恢复默认」（解除外部链接，回到内部库模式）。
    pub can_reset: bool,
}

/// 由 `storage_path` 与上下文目录推导展示信息。
///
/// - `library_dir`：应用内部共享库目录（`<exe_parent>/data/library`）。
/// - `desktop_dir`：真实桌面目录。指向它的栅栏是「桌面镜像」——解除链接会让栅栏清空，
///   而真实桌面图标此刻正被壳层接管隐藏，用户会看到「桌面全空」，故禁止回退。
pub fn describe(
    storage_path: Option<&str>,
    library_dir: &str,
    desktop_dir: Option<&str>,
) -> StorageInfo {
    match storage_path {
        None => StorageInfo {
            kind: StorageKind::AppLibrary,
            path: library_dir.to_string(),
            can_reset: false,
        },
        Some(dir) => {
            let is_desktop = desktop_dir.map(|d| same_dir(d, dir)).unwrap_or(false);
            StorageInfo {
                kind: StorageKind::ExternalFolder,
                path: dir.to_string(),
                can_reset: !is_desktop,
            }
        }
    }
}

/// 两个目录字符串是否指向同一处：忽略尾部 `\` / `/` 与 ASCII 大小写。
fn same_dir(a: &str, b: &str) -> bool {
    fn trim(s: &str) -> &str {
        s.trim_end_matches(['\\', '/'])
    }
    let (ta, tb) = (trim(a), trim(b));
    !ta.is_empty() && ta.eq_ignore_ascii_case(tb)
}

/// 中段省略：把过长的路径压成 `C:\…\debug\data\library` 形式。
///
/// 取舍优先级：**叶子名完整** > **更多层级** > **盘符前缀**。
/// 先保证最后一节（文件名/文件夹名）完整显示，再由后向前贪心追加层级；
/// 层级被截断且仍有预算时，尽量以 `C:\…\尾部` 补回盘符（最粗粒度的决策信息）。
/// 叶子本身都放不下时退化为纯尾部截断（`…brary`），预算极窄时返回空串。
///
/// 宽度口径与 [`crate::text::estimate_width`] 一致（CJK 1.0、ASCII 0.62 × 字号），
/// 与绘制层 `draw_text` 的截断口径同源。只按 `char` 边界切分，绝不切断多字节字符。
/// 能整段放下则原样返回；`budget <= 0` 返回空串。
pub fn elide_middle(path: &str, budget: f32, font_size: f32) -> String {
    if budget <= 0.0 || font_size <= 0.0 {
        return String::new();
    }
    if estimate_width(path, font_size) <= budget {
        return path.to_string();
    }
    let sep = if path.contains('\\') { '\\' } else { '/' };
    let sep_str = sep.to_string();
    let sep_w = estimate_width(&sep_str, font_size);
    let ell_w = estimate_width("…", font_size);
    let comps: Vec<&str> = path.split(['\\', '/']).filter(|c| !c.is_empty()).collect();
    let Some(&leaf) = comps.last() else {
        return String::new();
    };

    // 叶子都放不下：退化为纯尾部截断（保住文件名的后半段）。
    let prefix_w = ell_w + sep_w;
    if estimate_width(leaf, font_size) + prefix_w > budget {
        let tail = take_tail(path, (budget - ell_w).max(0.0), font_size);
        return if tail.is_empty() {
            String::new()
        } else {
            format!("…{tail}")
        };
    }

    // 由后向前贪心追加层级；叶子始终保留。
    let mut shown: Vec<&str> = vec![leaf];
    let mut width = estimate_width(leaf, font_size) + prefix_w;
    for c in comps[..comps.len() - 1].iter().rev() {
        let add = estimate_width(c, font_size) + sep_w;
        if width + add > budget {
            break;
        }
        shown.insert(0, c);
        width += add;
    }

    if shown.len() == comps.len() {
        return path.to_string();
    }
    let joined = shown.join(&sep_str);
    // 盘符被截掉时，试一次更短的 `C:\…\尾部` 形式。
    if let Some(&first) = comps.first() {
        if is_drive(first) && !shown.contains(&first) {
            let cand = format!("{first}{sep}…{sep}{joined}");
            if estimate_width(&cand, font_size) <= budget {
                return cand;
            }
        }
    }
    format!("…{sep}{joined}")
}

/// 是否 Windows 盘符组件（`C:`）。
fn is_drive(c: &str) -> bool {
    let b = c.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// 取末尾可放入 `budget` 的最长后缀（按 `char` 累加，不切开多字节字符）。
fn take_tail(s: &str, budget: f32, font_size: f32) -> String {
    let mut out = String::new();
    for ch in s.chars().rev() {
        let mut cand = String::with_capacity(out.len() + 4);
        cand.push(ch);
        cand.push_str(&out);
        if estimate_width(&cand, font_size) > budget {
            break;
        }
        out = cand;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FONT: f32 = 10.0;
    const LIB: &str = "C:\\app\\data\\library";

    #[test]
    fn describe_none_is_app_library() {
        let info = describe(None, LIB, None);
        assert_eq!(info.kind, StorageKind::AppLibrary);
        assert_eq!(info.path, LIB);
        assert!(!info.can_reset);
    }

    #[test]
    fn describe_some_is_external_and_resettable() {
        let info = describe(Some("D:\\归档"), LIB, Some("C:\\Users\\me\\Desktop"));
        assert_eq!(info.kind, StorageKind::ExternalFolder);
        assert_eq!(info.path, "D:\\归档");
        assert!(info.can_reset);
    }

    #[test]
    fn describe_desktop_source_is_not_resettable() {
        let desktop = "C:\\Users\\me\\Desktop";
        let info = describe(Some(desktop), LIB, Some(desktop));
        assert_eq!(info.kind, StorageKind::ExternalFolder);
        assert!(
            !info.can_reset,
            "桌面镜像栅栏解除链接会让栅栏清空，必须禁止"
        );
    }

    #[test]
    fn describe_dir_compare_ignores_case_and_trailing_separator() {
        let info = describe(
            Some("c:\\users\\ME\\desktop\\"),
            LIB,
            Some("C:\\Users\\me\\Desktop"),
        );
        assert!(!info.can_reset);
    }

    #[test]
    fn elide_fits_returns_unchanged() {
        assert_eq!(elide_middle("D:\\归档", 500.0, FONT), "D:\\归档");
    }

    #[test]
    fn elide_keeps_leaf_whole() {
        let p = "C:\\Users\\vacio\\.workbuddy-ai\\target\\debug\\data\\library";
        let out = elide_middle(p, 60.0, FONT);
        assert!(out.contains('…'), "{out}");
        assert!(out.ends_with("library"), "叶子名必须完整: {out}");
        assert!(estimate_width(&out, FONT) <= 60.0, "{out}");
    }

    #[test]
    fn elide_shows_drive_when_room_allows() {
        let p = "C:\\Users\\vacio\\.workbuddy-ai\\target\\debug\\data\\library";
        let out = elide_middle(p, 200.0, FONT);
        assert!(out.starts_with("C:\\"), "有余量时补回盘符: {out}");
        assert!(out.contains('…'), "{out}");
        assert!(out.ends_with("library"), "{out}");
        assert!(estimate_width(&out, FONT) <= 200.0, "{out}");
    }

    #[test]
    fn elide_narrow_budget_degrades_to_tail() {
        let p = "C:\\Users\\vacio\\.workbuddy-ai\\target\\debug\\data\\library";
        let out = elide_middle(p, 30.0, FONT);
        assert!(out.starts_with('…'), "{out}");
        assert!(estimate_width(&out, FONT) <= 30.0, "{out}");
    }

    #[test]
    fn elide_never_splits_multibyte() {
        let p = "D:\\非常长的中文目录名称\\另一个很长的中文子目录\\最后的文件夹";
        let out = elide_middle(p, 40.0, FONT);
        // 能构造出 String 即说明未切断多字节；再断言宽度与长度约束。
        assert!(out.starts_with('…'), "{out}");
        assert!(out.ends_with("文件夹"), "{out}");
        assert!(estimate_width(&out, FONT) <= 40.0, "{out}");
        assert!(out.chars().count() <= p.chars().count(), "{out}");
    }

    #[test]
    fn elide_zero_budget_is_empty() {
        assert_eq!(elide_middle("D:\\归档", 0.0, FONT), "");
        assert_eq!(elide_middle("D:\\归档", -1.0, FONT), "");
        assert_eq!(elide_middle("D:\\归档", 100.0, 0.0), "");
    }

    #[test]
    fn is_drive_only_matches_windows_drive() {
        assert!(is_drive("C:"));
        assert!(!is_drive("\\\\server"));
        assert!(!is_drive("home"));
        assert!(!is_drive(""));
    }
}
