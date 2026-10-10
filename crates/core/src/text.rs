//! 文本度量：统一的粗略宽度估算，App（布局）与 Render（绘制）共用同一口径，
//! 避免两处估算不一致导致居中/对齐偏移或文字截断。

/// 估算文本像素宽度：CJK 等宽字符按 `font_size` 宽，ASCII 按 `0.62 × font_size` 宽。
///
/// 这是布局层与绘制层共用的一套近似口径（不用 DWrite 精确测量，
/// 保证布局预留空间与绘制实际占宽一致）。
pub fn estimate_width(text: &str, font_size: f32) -> f32 {
    let units: f32 = text
        .chars()
        .map(|c| if c.is_ascii() { 0.62 } else { 1.0 })
        .sum();
    units * font_size
}

/// **容器上界**所需的单字宽度（em 倍数）：用于「容器必须装得下文本」的场合
/// （重命名框宽、折行预算、悬停工具提示宽）。
///
/// 与 [`estimate_width`] 的分工：后者是**平均**口径，用于布局居中/等宽对齐/是否截断的粗判；
/// 前者是**上界**口径——按真实字体的 advance 取值（Microsoft YaHei UI / Segoe UI 实测，
/// em）：`W` 1.02、`@` 1.03、`M` 0.98、`m` 0.94、`%` 0.89、`&` 0.87、
/// `O/Q/N/G/D/H/U/B/R/P/C/K/V/X/Y/Z/w` ≈ 0.76~0.83，其余拉丁字母/数字/空格 ≈ 0.5~0.62。
///
/// **为什么必须有这一档**：全大写或含大量 `W/M/@/m/w` 的文件名（如 `WWWW….txt`、
/// `MMMMMMMM.doc`）平均字宽可达 0.9 em 以上，用 0.62 em 的平均口径算出来的框宽会让
/// DirectWrite 画出的文本溢出框右缘而被静默裁掉——「不截断」在这类名字上失效。
/// 上界档只影响容器尺寸，不参与居中/截断判定，故不会让普通名字的排版变样。
/// 非 ASCII 一律按 1.0（CJK 恰好 1 em），emoji 区段按 1.5（Segoe UI Emoji 实测可超 1 em）。
pub fn char_upper_unit(c: char) -> f32 {
    if !c.is_ascii() {
        let cp = c as u32;
        // 常见 emoji / 符号区段：字形宽可超过一个 em
        return if (0x2190..=0x2BFF).contains(&cp) || (0x1F000..=0x1FAFF).contains(&cp) {
            1.5
        } else {
            1.0
        };
    }
    match c {
        'W' | '@' => 1.05,
        'M' => 1.0,
        'm' => 0.95,
        '%' => 0.9,
        '&' => 0.88,
        'O' | 'Q' | 'N' | 'G' | 'D' | 'H' | 'U' | 'B' | 'R' | 'P' | 'C' | 'K' | 'V' | 'X' | 'Y'
        | 'Z' | 'w' => 0.83,
        _ => 0.62,
    }
}

/// 文本在**上界口径**下的宽度：真实排版宽度不会超过它（容器尺寸用，见 [`char_upper_unit`]）。
pub fn upper_width(text: &str, font_size: f32) -> f32 {
    text.chars().map(char_upper_unit).sum::<f32>() * font_size
}

/// 一个视觉行在**字符**坐标系里的区间（左闭右开）。
///
/// 用字符下标而不是字节下标：调用方（光标列、点击定位）本来就按字符计数，
/// 换算一次就够，避免两套下标互相污染。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualLine {
    /// 起始字符下标（含）。
    pub start: usize,
    /// 结束字符下标（不含）。
    pub end: usize,
}

/// 把一条逻辑行按**上界宽度**折成若干**视觉行**（贪心、绝不切开多字节字符）。
///
/// 这是「就地重命名框宽度有界 + 内容换行显示」的唯一折行口径：App 层（光标定位、
/// 点击命中）与 Render 层（逐行绘制）必须调它，否则光标会落在与所见不同的字符上。
///
/// 逐字宽度用 [`char_upper_unit`]（上界）而不是平均口径：折行的目的是「折出来的行
/// **实际画出来**也不会超出容器」，用平均口径折行会让宽字形（全大写 / `W M @`）仍然溢出。
///
/// 语义：
/// - `max_w <= 0`（或文本为空）→ 恒返回一行（`0..len`），调用方无需特判；
/// - 单个字符本身就超过 `max_w` 时独占一行（保证每次推进，绝不空转死循环）；
/// - 换行点从字符边界切，**不做**单词/标点级避让（与绘制层的两行标签同一策略）。
pub fn wrap_visual_lines(text: &str, max_w: f32, font_size: f32) -> Vec<VisualLine> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![VisualLine { start: 0, end: 0 }];
    }
    if max_w <= 0.0 || font_size <= 0.0 {
        return vec![VisualLine {
            start: 0,
            end: chars.len(),
        }];
    }
    // 容差：逐字累加的浮点误差（长名字可达 1e-4 量级）不得让「恰好放得下」的文本多折一行；
    // 取字号的 1%，远小于一个字宽，不会真的放过放不下的字。
    let budget = max_w + font_size * 0.01;
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut used = 0.0f32;
    for (i, c) in chars.iter().enumerate() {
        let w = char_upper_unit(*c) * font_size;
        // 超预算且本行已有内容 → 在此字符前断行（该字符留给下一行）
        if used + w > budget && i > start {
            out.push(VisualLine { start, end: i });
            start = i;
            used = 0.0;
        }
        used += w;
    }
    out.push(VisualLine {
        start,
        end: chars.len(),
    });
    out
}

/// 折行后 `col`（字符列）落在第几个视觉行（返回视觉行下标；越界按末行处理）。
///
/// 边界口径：视觉行区间按**闭区间**判定（`start <= col <= end`），于是「行尾光标」停在
/// **本行末尾**而不是下一行开头——相邻两行共享这个位置，取靠前的一行，点击行尾时光标
/// 才不会跳到下一行。空行（`start == end`）也只匹配自身。
pub fn visual_line_of(lines: &[VisualLine], col: usize) -> usize {
    for (i, l) in lines.iter().enumerate() {
        if col >= l.start && col <= l.end {
            return i;
        }
    }
    lines.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::{estimate_width, upper_width, visual_line_of, wrap_visual_lines, VisualLine};

    #[test]
    fn ascii_is_062_per_char() {
        assert_eq!(estimate_width("abc", 10.0), 18.6); // 3 × 0.62 × 10
    }

    #[test]
    fn cjk_is_full_width() {
        assert_eq!(estimate_width("栅栏", 10.0), 20.0); // 2 × 1.0 × 10
    }

    #[test]
    fn mixed_text_sums_units() {
        assert_eq!(estimate_width("a栅b", 10.0), 22.4); // 0.62 + 1.0 + 0.62
    }

    #[test]
    fn empty_text_is_zero() {
        assert_eq!(estimate_width("", 10.0), 0.0);
    }

    #[test]
    fn scales_with_font_size() {
        let half = estimate_width("栅栏abc", 10.0);
        assert_eq!(estimate_width("栅栏abc", 20.0), half * 2.0);
    }

    /// 上界口径必须 ≥ 平均口径（容器尺寸用它才不会裁字）。
    #[test]
    fn upper_width_is_never_below_estimate() {
        for t in [
            "abc.txt",
            "WWWW",
            "@@@",
            "中文名.docx",
            "MmWw%&",
            "PROJEKT.DOCX",
            "",
        ] {
            assert!(
                upper_width(t, 12.0) >= estimate_width(t, 12.0) - 1e-3,
                "上界不得低于平均: {t}"
            );
        }
        // 宽字形被抬高；CJK 与平均口径一致（实测恰好 1 em）
        assert!(upper_width("W", 12.0) > estimate_width("W", 12.0));
        assert!(upper_width("m", 12.0) > estimate_width("m", 12.0));
        assert_eq!(upper_width("中", 12.0), estimate_width("中", 12.0));
        // 窄字形保持平均口径（否则普通名字的容器会无谓变宽）
        assert_eq!(upper_width("i", 12.0), estimate_width("i", 12.0));
        // emoji / 符号区段按 1.5 em
        assert!(upper_width("😀", 10.0) > estimate_width("😀", 10.0));
        assert!(upper_width("😀", 10.0) >= 15.0);
    }

    /// 折行按**上界**宽度：平均口径「以为装得下」的宽字形文本也必须折行，
    /// 否则画出来仍会溢出容器（审查 F1 的回归点）。
    #[test]
    fn wrap_uses_upper_bound_units() {
        let text = "WWWWWWWW";
        let font = 12.0;
        let avg = estimate_width(text, font); // 8 × 0.62 × 12 = 59.5
        let lines = wrap_visual_lines(text, avg + 1.0, font);
        assert!(
            lines.len() > 1,
            "上界口径必须折行，否则宽字形会溢出容器: {lines:?}（平均宽 {avg}）"
        );
        let back: usize = lines.iter().map(|l| l.end - l.start).sum();
        assert_eq!(back, text.chars().count(), "折行不得丢字");
    }

    /// 折行覆盖全文、逐行不超预算、且行间首尾相接（无丢字/重字）。
    #[test]
    fn wrap_covers_text_without_loss_or_overlap() {
        let text = "一个很长的文件名示例文档2026-final.docx";
        let font = 12.0;
        for max_w in [20.0, 60.0, 100.0, 1000.0] {
            let lines = wrap_visual_lines(text, max_w, font);
            assert_eq!(lines[0].start, 0);
            assert_eq!(lines[lines.len() - 1].end, text.chars().count());
            for w in lines.windows(2) {
                assert_eq!(w[0].end, w[1].start, "行间必须首尾相接");
            }
            // 除「单字超预算」的极端外，每行宽度都应在预算内
            for l in &lines {
                let seg: String = text.chars().skip(l.start).take(l.end - l.start).collect();
                let w = estimate_width(&seg, font);
                assert!(
                    w <= max_w + font * 1.01,
                    "max_w={max_w} 时行宽 {w} 超出预算: {seg:?}"
                );
            }
        }
    }

    /// 预算为 0 / 空文本 / 单字超宽：都必须返回可用结果（不 panic、不死循环、不丢字）。
    #[test]
    fn wrap_degenerates_safely() {
        assert_eq!(
            wrap_visual_lines("abc", 0.0, 12.0),
            vec![VisualLine { start: 0, end: 3 }]
        );
        assert_eq!(
            wrap_visual_lines("", 50.0, 12.0),
            vec![VisualLine { start: 0, end: 0 }]
        );
        // 单字宽于预算：独占一行，仍逐字推进
        let lines = wrap_visual_lines("栅栏", 1.0, 12.0);
        assert_eq!(
            lines,
            vec![
                VisualLine { start: 0, end: 1 },
                VisualLine { start: 1, end: 2 }
            ]
        );
    }

    /// 行尾光标必须停在**本行**（相邻行共享边界，取靠前一行）。
    #[test]
    fn visual_line_of_prefers_earlier_line_at_boundary() {
        let lines = vec![
            VisualLine { start: 0, end: 3 },
            VisualLine { start: 3, end: 6 },
            VisualLine { start: 6, end: 8 },
        ];
        assert_eq!(visual_line_of(&lines, 0), 0);
        assert_eq!(visual_line_of(&lines, 2), 0);
        assert_eq!(visual_line_of(&lines, 3), 0, "行尾光标停在本行");
        assert_eq!(visual_line_of(&lines, 4), 1);
        assert_eq!(visual_line_of(&lines, 6), 1);
        assert_eq!(visual_line_of(&lines, 8), 2);
        assert_eq!(visual_line_of(&lines, 99), 2, "越界按末行处理");
    }
}
