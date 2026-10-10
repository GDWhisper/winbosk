//! 就地文本编辑：D2D 内联编辑、光标/IME、图标与栅栏重命名。

use crate::*;

/// 内联编辑目标：栅栏内图标重命名 / 栅栏标题重命名 / 规则配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditTarget {
    Item { fence: usize, icon: usize },
    FenceTitle { fence: usize },
    RuleExtension { fence: usize },
    RuleExcludeExtension { fence: usize },
    RulePattern { fence: usize },
}

/// D2D 内联文本编辑：文本、光标与 IME 合成状态全在 App 层，绘制与输入同表面，
/// 彻底摆脱 HWND 弹出框的层级/焦点/对齐问题。支持单行（重命名/规则输入）与多行（便签）。
#[derive(Debug)]
pub(crate) struct InlineEdit {
    pub(crate) target: EditTarget,
    /// 编辑区矩形（物理像素）。
    pub(crate) rect: RectF,
    /// 文本行（单行编辑时恒为 1 行）。
    pub(crate) lines: Vec<String>,
    /// 光标位置（行、列）。
    pub(crate) line: usize,
    pub(crate) col: usize,
    /// 空文本时的占位提示。
    pub(crate) placeholder: String,
    pub(crate) single_line: bool,
    /// **视觉折行**的估算宽度预算：`Some` = 宽度有界、超宽内容换行显示（就地重命名框，
    /// 见 [`rename_edit_rect`]）；`None` = 不折行（规则输入框 / 便签按逻辑行绘制）。
    /// 与绘制层 `SceneEdit::wrap_w` 同值，两边必须用同一个 `wrap_visual_lines` 折行。
    pub(crate) wrap_w: Option<f32>,
    /// 是否聚焦（绘制光标/聚焦描边）。
    pub(crate) focused: bool,
    /// IME 合成状态。
    pub(crate) composing: bool,
    pub(crate) comp: String,
    /// IME 结果上屏中（避免合成结果触发递归提交）。
    pub(crate) committing: bool,
}

impl InlineEdit {
    fn current_line(&self) -> &str {
        self.lines.get(self.line).map(|s| s.as_str()).unwrap_or("")
    }

    /// 当前行**实际绘制**的文本：光标处插入 IME 合成串。拼接顺序与
    /// `draw_inline_edit` 逐字一致——编辑框宽度与折行都必须按「真正画出去的那串」算，
    /// 否则合成中的候选串会被框裁掉或与光标错位。
    fn display_text(&self) -> String {
        let line = self.current_line();
        let before: String = line.chars().take(self.col).collect();
        let after: String = line.chars().skip(self.col).collect();
        format!("{before}{}{after}", self.comp)
    }

    /// 光标在**显示串**（`display_text`）里的字符下标：合成串插在光标处，光标跟在它后面。
    fn caret_index(&self) -> usize {
        self.col + self.comp.chars().count()
    }

    /// 当前行的视觉行划分（折行口径与绘制层共用 `winbosk_core::text`）。
    fn visual_lines(&self, font_size: f32) -> Vec<winbosk_core::text::VisualLine> {
        let text = self.display_text();
        match self.wrap_w {
            Some(budget) => winbosk_core::text::wrap_visual_lines(&text, budget, font_size),
            None => vec![winbosk_core::text::VisualLine {
                start: 0,
                end: text.chars().count(),
            }],
        }
    }

    /// 光标所在的**全局视觉行序号**：光标之前所有逻辑行的视觉行数之和 + 本行内的视觉行下标。
    ///
    /// 不能简写成 `line + vi`：折行（`wrap_w = Some`）与多逻辑行（便签）同时成立时，
    /// 前面的逻辑行也可能各自折成多行。必须与 `draw.rs` 逐视觉行排布**同一个行序号口径**，
    /// 否则 IME 候选窗会落在别的行上。
    fn caret_row(&self, font_size: f32) -> usize {
        let mut row = 0usize;
        for (i, line) in self.lines.iter().enumerate() {
            if i == self.line {
                return row
                    + winbosk_core::text::visual_line_of(
                        &self.visual_lines(font_size),
                        self.caret_index(),
                    );
            }
            row += match self.wrap_w {
                // 非光标行不含合成串，直接按原文折行
                Some(budget) => {
                    winbosk_core::text::wrap_visual_lines(line, budget, font_size).len()
                }
                None => 1,
            };
        }
        row
    }

    /// 折行框内按**视觉行**上下移动光标（保持行内字符偏移，落点越界则贴行尾）。
    ///
    /// 仅用于「单逻辑行 + 折行」的重命名框（多行便签走 `cursor_up_down`）。
    /// 合成期间不介入：合成串的定位归 IME，抢方向键会让候选选择与光标互相打架。
    fn move_visual_line(&mut self, font_size: f32, down: bool) {
        if self.composing {
            return;
        }
        let lines = self.visual_lines(font_size);
        if lines.len() <= 1 {
            return;
        }
        let cur = winbosk_core::text::visual_line_of(&lines, self.caret_index());
        let target = if down { cur + 1 } else { cur.saturating_sub(1) };
        if target == cur || target >= lines.len() {
            return;
        }
        let offset = self.caret_index() - lines[cur].start;
        let want = (lines[target].start + offset).min(lines[target].end);
        // 显示串下标 → 逻辑列（与 `edit_click` 同一折算：合成串区间内一律回落到合成起点）
        let (logical, comp_len) = (self.col, self.comp.chars().count());
        self.col = if want <= logical {
            want
        } else if want <= logical + comp_len {
            logical
        } else {
            want - comp_len
        };
    }

    /// 在光标处插入一个字符。
    fn insert_char(&mut self, ch: char) {
        let col = self.col.min(self.current_line().chars().count());
        let b = self.byte_col();
        self.lines[self.line].insert(b, ch);
        self.col = col + 1;
    }

    /// 光标前的字节下标（`col` 是字符数，需换算为字节）。
    fn byte_col(&self) -> usize {
        self.current_line()
            .chars()
            .take(self.col)
            .map(char::len_utf8)
            .sum()
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            let b = self.byte_col();
            let prev = self.current_line()[..b]
                .chars()
                .next_back()
                .map(char::len_utf8)
                .unwrap_or(0);
            self.lines[self.line].replace_range(b - prev..b, "");
            self.col -= 1;
        } else if !self.single_line && self.line > 0 {
            let prev_len = self.lines[self.line - 1].chars().count();
            let cur = self.lines.remove(self.line);
            self.lines[self.line - 1].push_str(&cur);
            self.line -= 1;
            self.col = prev_len;
        }
    }

    fn delete_at(&mut self) {
        let b = self.byte_col();
        let line = self.current_line().to_string();
        if b < line.len() {
            let ch_len = line[b..].chars().next().map(char::len_utf8).unwrap_or(0);
            self.lines[self.line].replace_range(b..b + ch_len, "");
        } else if !self.single_line && self.line + 1 < self.lines.len() {
            let next = self.lines.remove(self.line + 1);
            self.lines[self.line].push_str(&next);
        }
    }

    fn cursor_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if !self.single_line && self.line > 0 {
            self.line -= 1;
            self.col = self.current_line().chars().count();
        }
    }

    fn cursor_right(&mut self) {
        let len = self.current_line().chars().count();
        if self.col < len {
            self.col += 1;
        } else if !self.single_line && self.line + 1 < self.lines.len() {
            self.line += 1;
            self.col = 0;
        }
    }

    fn cursor_up_down(&mut self, down: bool) {
        if self.single_line {
            return;
        }
        let target = if down {
            if self.line + 1 < self.lines.len() {
                self.line + 1
            } else {
                return;
            }
        } else if self.line > 0 {
            self.line - 1
        } else {
            return;
        };
        let len = self.lines[target].chars().count();
        self.line = target;
        self.col = self.col.min(len);
    }

    fn home_end(&mut self, end: bool) {
        self.col = if end {
            self.current_line().chars().count()
        } else {
            0
        };
    }

    /// Enter：单行 = 提交；多行 = 换行。
    fn enter(&mut self) -> bool {
        if self.single_line {
            true
        } else {
            let b = self.byte_col();
            let rest = self.lines[self.line][b..].to_string();
            self.lines[self.line].truncate(b);
            self.line += 1;
            self.lines.insert(self.line, rest);
            self.col = 0;
            false
        }
    }

    /// IME 合成结束：把结果串插入光标处。
    pub(crate) fn commit_ime(&mut self, text: &str) {
        let col = self.col.min(self.current_line().chars().count());
        let b = self.byte_col();
        self.lines[self.line].insert_str(b, text);
        self.col = col + text.chars().count();
        self.composing = false;
        self.comp.clear();
    }
}
pub(crate) fn dismiss_edit(rt: &mut Runtime) {
    // 解除 IME 关联（幂等）先行：覆盖「编辑已结束但关联残留」的异常路径，也覆盖
    // 下方提交逻辑的所有分支（状态全集校验，见 docs/plans/11 §2.2）。
    release_overlay_ime(rt);
    let Some(edit) = rt.edit.take() else {
        return;
    };
    match edit.target {
        EditTarget::FenceTitle { fence } => {
            let text = edit.lines.join("").trim().to_string();
            if apply_rename(rt, EditTarget::FenceTitle { fence }, &text) {
                inject_rebuild(rt);
            }
        }
        EditTarget::Item { fence, icon } => {
            let text = edit.lines.join("").trim().to_string();
            if apply_rename(rt, EditTarget::Item { fence, icon }, &text) {
                inject_rebuild(rt);
            }
        }
        target @ (EditTarget::RuleExtension { .. }
        | EditTarget::RuleExcludeExtension { .. }
        | EditTarget::RulePattern { .. }) => {
            let text = edit.lines.join("");
            apply_rule_input(rt, target, &text);
        }
    }
}

/// 提交当前编辑（单行 Enter）：重命名/规则提交并关闭。
pub(crate) fn commit_edit(rt: &mut Runtime) {
    // 与 `dismiss_edit` 同一出口约定：提交即结束编辑会话，解除 IME 关联（幂等）。
    release_overlay_ime(rt);
    let Some((target, text)) = rt.edit.as_ref().map(|e| (e.target, e.lines.join(""))) else {
        return;
    };
    rt.edit = None;
    match target {
        EditTarget::FenceTitle { fence } => {
            let text = text.trim().to_string();
            if apply_rename(rt, EditTarget::FenceTitle { fence }, &text) {
                inject_rebuild(rt);
            }
        }
        EditTarget::Item { fence, icon } => {
            let text = text.trim().to_string();
            if apply_rename(rt, EditTarget::Item { fence, icon }, &text) {
                inject_rebuild(rt);
            }
        }
        target @ (EditTarget::RuleExtension { .. }
        | EditTarget::RuleExcludeExtension { .. }
        | EditTarget::RulePattern { .. }) => {
            apply_rule_input(rt, target, &text);
        }
    }
}

/// 解析扩展名输入（支持英文逗号、中文逗号、空格、分号分隔，自动去点并转小写）
pub(crate) fn parse_extension_tokens(text: &str) -> Vec<String> {
    text.split([',', '，', ' ', ';'])
        .map(|s| s.trim().trim_start_matches('.').to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 解析通配符模式输入（支持英文逗号、中文逗号、分号分隔，保留大小写与空格）
pub(crate) fn parse_pattern_tokens(text: &str) -> Vec<String> {
    text.split([',', '，', ';'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 应用规则输入（回车或失焦提交）：解析并写入对应栅栏的规则字段。
pub(crate) fn apply_rule_input(rt: &mut Runtime, target: EditTarget, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    match target {
        EditTarget::RuleExtension { fence } => {
            let tokens = parse_extension_tokens(text);
            if !tokens.is_empty() {
                if let Some(f) = rt.desk.fences.get_mut(fence) {
                    let mut rule = f.rule.clone().unwrap_or_default();
                    rule.enabled = true;
                    rule.is_custom = true;
                    for t in tokens {
                        if !rule.custom_extensions.contains(&t) {
                            rule.custom_extensions.push(t);
                        }
                    }
                    f.rule = Some(rule);
                    let _ = rt.store.save(&rt.desk);
                    inject_rebuild(rt);
                }
            }
        }
        EditTarget::RuleExcludeExtension { fence } => {
            let tokens = parse_extension_tokens(text);
            if !tokens.is_empty() {
                if let Some(f) = rt.desk.fences.get_mut(fence) {
                    let mut rule = f.rule.clone().unwrap_or_default();
                    rule.enabled = true;
                    rule.is_custom = true;
                    for t in tokens {
                        if !rule.exclude_extensions.contains(&t) {
                            rule.exclude_extensions.push(t);
                        }
                    }
                    f.rule = Some(rule);
                    let _ = rt.store.save(&rt.desk);
                    inject_rebuild(rt);
                }
            }
        }
        EditTarget::RulePattern { fence } => {
            let tokens = parse_pattern_tokens(text);
            if !tokens.is_empty() {
                if let Some(f) = rt.desk.fences.get_mut(fence) {
                    let mut rule = f.rule.clone().unwrap_or_default();
                    rule.enabled = true;
                    rule.is_custom = true;
                    for t in tokens {
                        if !rule.name_patterns.contains(&t) {
                            rule.name_patterns.push(t);
                        }
                    }
                    f.rule = Some(rule);
                    let _ = rt.store.save(&rt.desk);
                    inject_rebuild(rt);
                }
            }
        }
        _ => {}
    }
}

/// 打开规则输入框（弹出 D2D 内联编辑）。
pub(crate) fn open_rule_input(rt: &mut Runtime, target: EditTarget, placeholder: &str) {
    rt.edit = Some(InlineEdit {
        target,
        rect: RectF {
            x: 0.0,
            y: 0.0,
            w: 140.0 * rt.theme.scale,
            h: rt.theme.controls.chip_h,
        },
        lines: vec![String::new()],
        line: 0,
        col: 0,
        placeholder: placeholder.to_string(),
        single_line: true,
        // 规则输入框宽度由控制台布局逐帧回写，不折行（见 `rename_edit_rect` 的 Non-Goal）
        wrap_w: None,
        focused: true,
        composing: false,
        comp: String::new(),
        committing: false,
    });
    focus_overlay(rt);
    tracing::info!(target = ?target, "打开规则输入框（D2D 内联）");
}

/// 键盘事件 → 内联编辑（光标/退格/删除/方向/Home/End/回车/Esc/Ctrl+V 粘贴）。
pub(crate) fn edit_key(rt: &mut Runtime, vk: u32, ctrl: bool) {
    if rt.edit.is_none() {
        return;
    }
    let mut caret_moved = false;
    match vk {
        v if v == VK_RETURN.0 as u32 => {
            let single = rt.edit.as_ref().map(|e| e.single_line).unwrap_or(true);
            if single {
                commit_edit(rt);
            } else if let Some(e) = rt.edit.as_mut() {
                e.enter();
                caret_moved = true;
            }
        }
        v if v == VK_ESCAPE.0 as u32 => {
            rt.edit = None;
            release_overlay_ime(rt);
        }
        v if v == VK_BACK.0 as u32 => {
            if let Some(e) = rt.edit.as_mut() {
                e.backspace();
            }
            caret_moved = true;
        }
        v if v == VK_DELETE.0 as u32 => {
            if let Some(e) = rt.edit.as_mut() {
                e.delete_at();
            }
            caret_moved = true;
        }
        v if v == VK_LEFT.0 as u32 => {
            if let Some(e) = rt.edit.as_mut() {
                if ctrl {
                    e.home_end(false);
                } else {
                    e.cursor_left();
                }
            }
            caret_moved = true;
        }
        v if v == VK_RIGHT.0 as u32 => {
            if let Some(e) = rt.edit.as_mut() {
                if ctrl {
                    e.home_end(true);
                } else {
                    e.cursor_right();
                }
            }
            caret_moved = true;
        }
        v if v == VK_UP.0 as u32 => {
            let font = rt.theme.label.size;
            if let Some(e) = rt.edit.as_mut() {
                if e.single_line && e.wrap_w.is_some() {
                    // 折行框：上下 = 视觉行；多行便签与规则输入走逻辑行
                    e.move_visual_line(font, false);
                } else {
                    e.cursor_up_down(false);
                }
            }
            caret_moved = true;
        }
        v if v == VK_DOWN.0 as u32 => {
            let font = rt.theme.label.size;
            if let Some(e) = rt.edit.as_mut() {
                if e.single_line && e.wrap_w.is_some() {
                    e.move_visual_line(font, true);
                } else {
                    e.cursor_up_down(true);
                }
            }
            caret_moved = true;
        }
        v if v == VK_HOME.0 as u32 => {
            if let Some(e) = rt.edit.as_mut() {
                e.home_end(false);
            }
            caret_moved = true;
        }
        v if v == VK_END.0 as u32 => {
            if let Some(e) = rt.edit.as_mut() {
                e.home_end(true);
            }
            caret_moved = true;
        }
        v if v == 0x56 && ctrl => {
            // Ctrl+V 粘贴
            edit_paste(rt);
            caret_moved = true;
        }
        _ => {}
    }
    if caret_moved {
        position_ime_window(rt);
    }
}

/// 普通字符（WM_CHAR，非 IME 路径）插入。
pub(crate) fn edit_char(rt: &mut Runtime, ch: u16) {
    let Some(edit) = rt.edit.as_mut() else {
        return;
    };
    if edit.committing {
        return;
    }
    // 代理对缓冲（emoji 由两个 WM_CHAR 到达）
    match ch {
        0xD800..=0xDBFF => {
            rt.edit_high = Some(ch);
            return;
        }
        0xDC00..=0xDFFF => {
            if let Some(hi) = rt.edit_high.take() {
                let c =
                    char::from_u32(0x10000 + ((hi as u32 - 0xD800) << 10) + (ch as u32 - 0xDC00))
                        .unwrap_or('\u{FFFD}');
                edit.insert_char(c);
            }
            position_ime_window(rt);
            return;
        }
        _ => {}
    }
    rt.edit_high = None;
    if ch < 32 {
        return; // 控制字符忽略（回车/退格等已由 KeyDown 处理）
    }
    if let Some(c) = char::from_u32(ch as u32) {
        edit.insert_char(c);
    }
    position_ime_window(rt);
}

/// 从剪贴板读 Unicode 文本。
pub(crate) fn clipboard_text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        // CF_UNICODETEXT = 13（标准剪贴板格式，避免引入 Ole feature）
        let h = GetClipboardData(13).ok()?;
        if h.is_invalid() {
            let _ = CloseClipboard();
            return None;
        }
        let hg = HGLOBAL(h.0);
        let p = GlobalLock(hg);
        let size = GlobalSize(hg);
        let out = if !p.is_null() && size > 0 {
            let n = (size / 2) as usize;
            let slice = std::slice::from_raw_parts(p as *const u16, n);
            let end = slice.iter().position(|&c| c == 0).unwrap_or(n);
            Some(String::from_utf16_lossy(&slice[..end]))
        } else {
            None
        };
        let _ = GlobalUnlock(hg);
        let _ = CloseClipboard();
        out
    }
}

/// 把剪贴板文本插入内联编辑（单行换行转空格，多行按行插入）。
pub(crate) fn edit_paste(rt: &mut Runtime) {
    let Some(text) = clipboard_text() else {
        return;
    };
    let Some(edit) = rt.edit.as_mut() else {
        return;
    };
    if edit.single_line {
        let text = text.replace(['\r', '\n'], " ");
        let col = edit.col.min(edit.current_line().chars().count());
        let b = edit.byte_col();
        edit.lines[0].insert_str(b, &text);
        edit.col = col + text.chars().count();
    } else {
        let normalized = text.replace("\r\n", "\n");
        let parts: Vec<&str> = normalized.split('\n').collect();
        let b = edit.byte_col();
        let line = edit.line;
        let tail = edit.lines[line][b..].to_string();
        edit.lines[line].truncate(b);
        edit.lines[line].push_str(parts[0]);
        let mut rest: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
        if let Some(last) = rest.last_mut() {
            last.push_str(&tail);
        }
        let insert_at = line + 1;
        edit.lines.splice(insert_at..insert_at, rest);
        edit.line = line + parts.len() - 1;
        edit.col = edit.current_line().chars().count();
    }
}

/// 把 IME 合成窗口定位到光标（候选列表跟随光标）。
pub(crate) fn position_ime_window(rt: &Runtime) {
    let (x, y) = edit_caret_point(rt);
    unsafe {
        let ctx = ImmGetContext(rt.hwnd);
        if !ctx.0.is_null() {
            let form = COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: POINT { x, y },
                rcArea: RECT::default(),
            };
            let _ = ImmSetCompositionWindow(ctx, &form);
            let _ = ImmReleaseContext(rt.hwnd, ctx);
        }
    }
}

/// 内联编辑光标屏幕坐标（IME 窗口定位用；物理像素）。
///
/// 三个分支对应三种排布，必须与 `draw.rs::draw_inline_edit` 逐像素同源：
/// - 多行便签：按逻辑行（不折行）；
/// - 单视觉行的重命名/规则输入：整框垂直居中；
/// - 折成多行的重命名：光标所在视觉行自上而下排布（`EDIT_LINE_H_MULT` 行距）。
pub(crate) fn edit_caret_point(rt: &Runtime) -> (i32, i32) {
    let Some(edit) = &rt.edit else {
        return (0, 0);
    };
    let font = rt.theme.label.size;
    let pad_y = rt.theme.controls.input_pad_y;
    let line_h = font * winbosk_render::EDIT_LINE_H_MULT;
    let text = edit.display_text();
    let caret = edit.caret_index();
    let lines = edit.visual_lines(font);
    let vi = winbosk_core::text::visual_line_of(&lines, caret);
    let before: String = text.chars().take(caret).skip(lines[vi].start).collect();
    let x = edit.rect.x + rt.theme.controls.input_pad_x + label_width(&before, font);
    let y = if !edit.single_line {
        // 多行便签：按**全局视觉行序号**（前面各逻辑行的视觉行数之和 + 本行内下标）
        edit.rect.y + pad_y + edit.caret_row(font) as f32 * line_h + font * 0.8
    } else if lines.len() <= 1 {
        edit.rect.y + edit.rect.h / 2.0
    } else {
        edit.rect.y + pad_y + vi as f32 * line_h + font * 0.8
    };
    (x as i32, y as i32)
}

/// 鼠标点在编辑框内（x 为虚拟屏幕物理坐标）：把光标定位到对应字符。
/// 与 `edit_caret_point` 同口径（**文本左缘 = rect.x + input_pad_x**，逐字累计宽度），
/// 半字宽以上的点击进下一格，与常见编辑器行为一致。
pub(crate) fn edit_click(rt: &mut Runtime, x: f32, y: f32) {
    let font = rt.theme.label.size;
    let pad_x = rt.theme.controls.input_pad_x;
    let pad_y = rt.theme.controls.input_pad_y;
    let line_h = font * winbosk_render::EDIT_LINE_H_MULT;
    let Some(edit) = rt.edit.as_mut() else {
        return;
    };
    if !edit.single_line {
        return; // 多行便签仍走键盘定位（本轮不涉及）
    }
    // 折行框必须按 y 先定行：只给 x 无法判断点在第几行（`OverlayEvent::EditCaret` 两个坐标都带）
    let (rect, text, col) = (edit.rect, edit.display_text(), edit.col);
    let visual = edit.visual_lines(font);
    let comp_len = edit.comp.chars().count();
    edit.col = click_column_wrapped(
        x, y, rect, &text, &visual, col, comp_len, font, pad_x, pad_y, line_h,
    );
    position_ime_window(rt);
}

/// 折行框内的点击定位（纯内核，`edit_click` 的可测部分）：`(x, y)` → 逻辑光标列。
///
/// - `y` 决定视觉行（单视觉行恒第 0 行；落在行间/框外按最近行处理，绝不越界）；
/// - `x` 在该视觉行内按 [`click_column`] 定位（文本左缘仍是 `rect.x + pad_x`）；
/// - 显示串下标 → 逻辑列：落在 IME 合成串区间内一律折算回合成起点。
#[allow(clippy::too_many_arguments)]
fn click_column_wrapped(
    x: f32,
    y: f32,
    rect: RectF,
    text: &str,
    visual: &[winbosk_core::text::VisualLine],
    col: usize,
    comp_len: usize,
    font: f32,
    pad_x: f32,
    pad_y: f32,
    line_h: f32,
) -> usize {
    if visual.is_empty() {
        return col;
    }
    let vi = if visual.len() > 1 {
        let rel_y = (y - (rect.y + pad_y)).max(0.0);
        ((rel_y / line_h) as usize).min(visual.len() - 1)
    } else {
        0
    };
    let vl = visual[vi];
    let seg: String = text
        .chars()
        .skip(vl.start)
        .take(vl.end - vl.start)
        .collect();
    let display_idx = vl.start + click_column(x, rect.x + pad_x, &seg, font);
    if display_idx <= col {
        display_idx
    } else if display_idx <= col + comp_len {
        col
    } else {
        display_idx - comp_len
    }
}

/// 点击位置 → 光标列（纯函数，`edit_click` 的可测内核）。
///
/// **对齐契约**：编辑文本恒左对齐于 `text_left = rect.x + input_pad_x`，绘制
/// （`draw.rs::draw_inline_edit` 的 `lr.left`）、光标（同处 `caret_x`）、IME 窗口
/// （`edit_caret_point`）与这里**共用同一个左缘**。`rect` 只描述「框」，任何生长
/// 方式都不得再引入第二份文本原点——否则「看到第 N 个字、点到第 M 个字」。
pub(crate) fn click_column(x: f32, text_left: f32, line: &str, font_size: f32) -> usize {
    let rel = (x - text_left).max(0.0);
    let mut col = 0;
    let mut acc = 0.0;
    for (idx, c) in line.char_indices() {
        let w = label_width(&line[..idx + c.len_utf8()], font_size)
            - label_width(&line[..idx], font_size);
        if acc + w / 2.0 >= rel {
            break;
        }
        acc += w;
        col += 1;
    }
    col
}

/// 双击/右键打开：按显式成员列表反查并启动。
pub(crate) fn start_inplace_rename(rt: &mut Runtime, target: EditTarget) {
    // 初始文本
    let current = match target {
        EditTarget::Item { fence, icon } => match item_name(rt, fence, icon) {
            Some(name) => name,
            None => {
                tracing::warn!(fence, icon, "无法定位图标标签，跳过就地改名");
                return;
            }
        },
        EditTarget::FenceTitle { fence } => match rt.desk.fences.get(fence) {
            Some(f) => f
                .title
                .clone()
                .unwrap_or_else(|| format!("栅栏 {}", fence + 1)),
            None => return,
        },
        _ => return,
    };
    // 定位矩形（物理像素）：与逐帧刷新同一真源 `rename_edit_rect`——打开的那一刻就按
    // 当前文本算好宽/高与折行预算，整个名字（含扩展名）完整落在输入框内，与 Windows 一致。
    let Some(bbox) = rename_edit_rect(rt, target, &current) else {
        tracing::warn!(target = ?target, "无法定位编辑框，跳过就地改名");
        return;
    };
    rt.edit = Some(InlineEdit {
        target,
        rect: bbox.rect,
        lines: vec![current],
        line: 0,
        col: 0,
        placeholder: String::new(),
        single_line: true,
        wrap_w: Some(bbox.wrap_w),
        focused: true,
        composing: false,
        comp: String::new(),
        committing: false,
    });
    focus_overlay(rt);
    position_ime_window(rt);
    tracing::info!(target = ?target, "开始就地重命名（D2D 内联）");
}

/// 应用重命名结果：返回内容是否真的变化（变化才触发重绘）。
pub(crate) fn apply_rename(rt: &mut Runtime, target: EditTarget, new_name: &str) -> bool {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return false;
    }
    match target {
        EditTarget::FenceTitle { fence } => {
            let Some(f) = rt.desk.fences.get_mut(fence) else {
                return false;
            };
            let old = f
                .title
                .clone()
                .unwrap_or_else(|| format!("栅栏 {}", fence + 1));
            if old == new_name {
                return false;
            }
            f.title = Some(new_name.to_string());
            let _ = rt.store.save(&rt.desk);
            tracing::info!(fence, name = new_name, "栅栏改名");
            true
        }
        EditTarget::Item { fence, icon } => commit_icon_rename(rt, fence, icon, new_name),
        _ => false,
    }
}

/// 重命名栅栏内图标（编辑程序名字）：改的是磁盘上的真实文件名（快捷方式保持
/// `.lnk`/`.url`/`.appref-ms` 扩展名），改名后重建元数据、位图与引用。
/// 返回内容是否真的变化。
pub(crate) fn commit_icon_rename(
    rt: &mut Runtime,
    fence: usize,
    icon: usize,
    new_name: &str,
) -> bool {
    let (id, path, current) = match rt.desk.fences.get(fence).and_then(|f| f.icon_ids.get(icon)) {
        Some(id) => match rt.desk.icons.get(id) {
            Some(ic) => (id.clone(), ic.path.clone(), ic.display_name.clone()),
            None => return false,
        },
        None => return false,
    };
    if new_name == current {
        return false;
    }
    let Some(old_path) = path else {
        tracing::warn!(id = %id, "虚拟项（无路径）无法改名");
        return false;
    };
    let Some(new_path) = new_path_for_rename(&old_path, new_name) else {
        tracing::warn!(old_path, "无法计算新路径");
        return false;
    };
    let new_path_str = new_path.to_string_lossy().into_owned();
    if new_path_str == old_path {
        return false;
    }
    // 磁盘改名：与原文件夹名一致——Windows 资源管理器也是这样直接改文件
    if let Err(e) = std::fs::rename(&old_path, &new_path) {
        tracing::warn!(old_path, new_path = %new_path_str, "文件改名失败: {e}");
        return false;
    }
    // 重建 DesktopItem（新路径 → 新 id/显示名/类别），替换 items 池中的项
    let new_item = match winbosk_shell::items::item_from_path(&new_path_str) {
        Ok(it) => it,
        Err(e) => {
            tracing::warn!(new_path = %new_path_str, "重建图标项失败（文件已改名，重启后重新识别）: {e}");
            return false;
        }
    };
    let new_display = new_item.display_name.clone();
    let new_id = new_item.id.clone();
    if let Some(idx) = rt.item_index.remove(&id) {
        rt.items[idx] = new_item;
        rt.item_index.insert(new_id.clone(), idx);
    }
    // 更新元数据：旧 id 换新 id，path/显示名/详情按新路径补齐
    if let Some(mut ic) = rt.desk.icons.remove(&id) {
        ic.id = new_id.clone();
        ic.display_name = new_display;
        ic.path = Some(new_path_str.clone());
        winbosk_core::details::enrich(&mut ic, &new_path_str);
        rt.desk.icons.insert(new_id.clone(), ic);
    }
    // 替换栅栏成员与自由区引用
    for f in &mut rt.desk.fences {
        if let Some(pos) = f.icon_ids.iter().position(|x| x == &id) {
            f.icon_ids[pos] = new_id.clone();
        }
    }
    if let Some(pos) = rt.desk.free_icons.iter().position(|x| x == &id) {
        rt.desk.free_icons[pos] = new_id.clone();
    }
    // 新 id → 新位图槽（不复用旧槽，避免与既有槽冲突），重新提取图标
    rt.bitmap_ids.remove(&id);
    let slot = rt.bitmap_ids.values().copied().max().unwrap_or(0) + 1;
    if let Some(idx) = rt.item_index.get(&new_id).copied() {
        match winbosk_shell::icons::extract_icon(&rt.items[idx], ICON_EXTRACT_SIZE) {
            Ok(data) => {
                rt.bitmap_ids.insert(new_id.clone(), slot);
                rt.pending_uploads.push((slot, data));
            }
            Err(e) => tracing::warn!(new_path = %new_path_str, "改名后图标提取失败: {e}"),
        }
    }
    let _ = rt.store.save(&rt.desk);
    tracing::info!(old_path, new_path = %new_path_str, "图标改名");
    true
}

/// 注入一次「仅重绘」事件：让 `handle_event` 尾部重建场景与命中模型。
pub(crate) fn inject_rebuild(rt: &mut Runtime) {
    let ev = Box::new(OverlayEvent::EditCommitted);
    unsafe {
        let _ = PostMessageW(
            Some(rt.hwnd),
            WM_WINBOSK_INJECT,
            WPARAM(0),
            LPARAM(Box::into_raw(ev) as isize),
        );
    }
}

/// 图标标签文本（当前显示名）。
pub(crate) fn item_name(rt: &Runtime, fence: usize, icon: usize) -> Option<String> {
    rt.desk
        .fences
        .get(fence)
        .and_then(|f| f.icon_ids.get(icon))
        .and_then(|id| rt.desk.icons.get(id))
        .map(|ic| ic.display_name.clone())
}

/// 图标标签的绘制矩形（物理像素，虚拟屏幕坐标）：就地编辑框的定位基准。
/// 与 `layout_fence` / `grid_icons` / `list_icons` 的几何保持一致。
pub(crate) fn item_label_rect(rt: &Runtime, fence: usize, icon: usize) -> Option<RectF> {
    let f = rt.desk.fences.get(fence)?;
    f.icon_ids.get(icon)?;
    let s = rt.theme.scale;
    let pad = f.appearance.padding * s;
    let title_block_h = rt.theme.title.size * 1.6 + rt.theme.title_padding_bottom;
    let content_top = f.bounds.y + pad + title_block_h + pad;
    let content_left = f.bounds.x + pad;
    let inner_w = (f.bounds.w - 2.0 * pad).max(1.0);
    match f.appearance.layout {
        FenceLayout::Grid => {
            let icon_size = f.appearance.icon_size * s;
            // 与 scene.rs 同口径：格宽保底 + 两行标签行高（否则编辑框/后续行错位）。
            let cell_w = grid_cell_w(icon_size, f.appearance.gap * s);
            let row_h = grid_row_h(&rt.theme, icon_size);
            let cols = ((inner_w / cell_w).floor() as usize).max(1);
            // 与 `scene.rs::grid_icons` 同口径：图标在格内居中（`ix` = 图标左缘）。
            let ix = content_left + (icon % cols) as f32 * cell_w + (cell_w - icon_size) / 2.0;
            let iy = content_top + (icon / cols) as f32 * row_h - f.scroll;
            // 编辑框覆盖两行标签区（含上下剪裁余量 input_pad_y * 2.0，见 draw_inline_edit 的内缩）。
            // 横向与 draw.rs 的网格标签框**逐像素同源**（整格、以图标中心对称）：标签已居中，
            // 编辑框若仍贴格左缘，重命名时文字会在光标底下横跳。
            let edit_h =
                rt.theme.label.size * GRID_CAPTION_H_MULT + rt.theme.controls.input_pad_y * 2.0;
            Some(RectF {
                x: ix - (cell_w - icon_size) / 2.0,
                y: iy + icon_size + rt.theme.icon_caption_gap,
                w: cell_w,
                h: edit_h,
            })
        }
        FenceLayout::List => {
            let label_h = rt.theme.label.size * 1.6;
            let list_icon = LIST_ICON_SIZE * s;
            let row_h = list_icon.max(label_h) + rt.theme.list_row_gap;
            let header_h = label_h + 8.0 * s;
            let type_w = LIST_TYPE_W * s;
            let mod_w = LIST_MOD_W * s;
            let size_w = LIST_SIZE_W * s;
            let col_gap = LIST_COL_GAP * s;
            let name_w = (inner_w - col_gap * 3.0 - type_w - mod_w - size_w).max(60.0 * s);
            let iy = content_top + header_h + icon as f32 * row_h - f.scroll;
            // 编辑框高度 = 文本行高 + 上下剪裁余量（同 Grid：见 Grid 分支注释）。
            let edit_h = label_h + rt.theme.controls.input_pad_y * 2.0;
            Some(RectF {
                x: content_left + list_icon + rt.theme.list_label_gap,
                y: iy + (list_icon - label_h) / 2.0,
                w: name_w,
                h: edit_h,
            })
        }
        FenceLayout::Sidebar => {
            // 侧边栏无内联标签：编辑框放在图标旁侧，复用工具提示的定位口径
            // （纵向 dock 放图标右侧/左侧、垂直居中；横向 dock 放图标下方、水平居中）。
            let pos = f.appearance.sidebar_pos;
            let icon_size = f.appearance.icon_size * s;
            let eff_gap = f.appearance.gap * s + icon_size * 0.5;
            let (icon_x, icon_y) = if pos == SidebarPosition::Top {
                // 横向 dock：厚度 = 紧贴放大图标，图标垂直居中，排布自左往右
                let dock_h = icon_size * 1.5 + 6.0 * s * 2.0;
                let start_x = f.bounds.x + pad;
                let start_y = f.bounds.y + (dock_h - icon_size) / 2.0;
                (
                    start_x + icon as f32 * (icon_size + eff_gap) - f.scroll,
                    start_y,
                )
            } else {
                // 纵向 dock：图标在 dock 内水平居中，排布自上往下
                let dock_w = f.bounds.w.max(icon_size);
                let start_x = f.bounds.x + (dock_w - icon_size) / 2.0;
                let start_y = f.bounds.y + pad;
                (
                    start_x,
                    start_y + icon as f32 * (icon_size + eff_gap) - f.scroll,
                )
            };
            let label_h = rt.theme.label.size * 1.6;
            // 编辑框高度 = 文本行高 + 上下剪裁余量（同 Grid：见 Grid 分支注释）。
            let edit_h = label_h + rt.theme.controls.input_pad_y * 2.0;
            let text = item_name(rt, fence, icon).unwrap_or_default();
            let w = (crate::scene::estimate_text_width(&text, rt.theme.label.size)
                * crate::scene::TEXT_WIDTH_SLACK
                + rt.theme.controls.input_pad_x * 2.0)
                .max(label_h);
            let gap_to_icon = 10.0 * s;
            let (mut bx, mut by) = match pos {
                SidebarPosition::Left => (
                    icon_x + icon_size + gap_to_icon,
                    icon_y + (icon_size - label_h) / 2.0,
                ),
                SidebarPosition::Right => (
                    icon_x - gap_to_icon - w,
                    icon_y + (icon_size - label_h) / 2.0,
                ),
                SidebarPosition::Top => (
                    icon_x + icon_size / 2.0 - w / 2.0,
                    icon_y + icon_size + gap_to_icon,
                ),
            };
            // 钳制到虚拟屏幕内：贴边停靠时编辑框不外溢到屏幕外（宽度超出贴边即可）
            bx = bx.max(4.0).min((rt.vw - w - 4.0).max(4.0));
            by = by.max(4.0).min((rt.vh - edit_h - 4.0).max(4.0));
            Some(RectF {
                x: bx,
                y: by,
                w,
                h: edit_h,
            })
        }
    }
}

/// 重命名框与虚拟屏幕边缘的最小留白（物理像素）。
const RENAME_SCREEN_MARGIN: f32 = 4.0;

/// 折行预算相对框内可用宽的余量：折行逐字宽度已取**上界**（[`winbosk_core::text::char_upper_unit`]），
/// 这里只留 5% 吸收浮点累计与抗锯齿，不再乘 [`crate::scene::TEXT_WIDTH_SLACK`]（那是平均口径
/// 的补偿，两档叠乘会让折行过早、框无谓变高）。
const WRAP_WIDTH_SLACK: f32 = 1.05;

/// 就地重命名框的一次求值结果：矩形 + 折行预算。
///
/// 「框」与「折行」必须同时求出并一起回写（[`refresh_rename_rect`]）：绘制层要用同一个
/// `wrap_w` 把文本折成同样的行，App 层要用它算光标/点击/IME 位置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RenameBox {
    pub(crate) rect: RectF,
    /// 视觉折行的估算宽度预算（与 [`rename_width_for`] 同口径的估算宽度）。
    pub(crate) wrap_w: f32,
    /// 折出的视觉行数（≥1；测试与日志用，绘制层自行按同一函数重算）。
    pub(crate) lines: usize,
}

/// 折行/定宽所需的度量（物理像素，与绘制层同口径）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct EditMetrics {
    pub(crate) font: f32,
    pub(crate) pad_x: f32,
    pub(crate) pad_y: f32,
}

/// 就地重命名编辑框几何：锚点（图标标签矩形 / 栅栏标题矩形）不变，
/// **框宽以卡片为上限生长，装不下的部分改走折行**。
///
/// 两轮用户实测把口径逼成了现在这条（缺一不可）：
/// 1. 框宽必须随文本生长 —— 原来的框宽就是一个格宽，长文件名被静默裁掉；
/// 2. 生长必须有上限 —— 无界生长会让框越过栅栏卡片（截图实测：第 0 列的居中生长
///    把左半边推到卡片外，右侧同样会盖住邻栏）；
/// 3. 超上限的内容 **换行显示**，而不是裁掉、也不是缩字号/横向滚动 ——
///    框高随视觉行数增长（`font × EDIT_LINE_H_MULT` 每行）。
///
/// 宽度上限 = 卡片横向内缘（侧边栏挂在 dock 之外，上限为虚拟屏幕）。**折行预算** =
/// 框内可用宽 ÷ [`TEXT_WIDTH_SLACK`]：估算口径偏乐观，按可用宽直接折行仍可能画出框外。
///
/// 生长/定位方向：网格以图标中心对称（标签本就居中于图标下方），侧边栏按停靠边反向
/// （右侧停靠向左长），其余（列表 / 栅栏标题 / 左停靠侧边栏）自左缘向右长。
///
/// 非重命名目标（规则输入框）返回 `None`：其矩形由控制台布局逐帧回写，不参与生长。
pub(crate) fn rename_edit_rect(rt: &Runtime, target: EditTarget, text: &str) -> Option<RenameBox> {
    let metrics = EditMetrics {
        font: rt.theme.label.size,
        pad_x: pad_x(rt),
        pad_y: rt.theme.controls.input_pad_y,
    };
    match target {
        EditTarget::Item { fence, icon } => {
            let f = rt.desk.fences.get(fence)?;
            f.icon_ids.get(icon)?;
            let base = item_label_rect(rt, fence, icon)?;
            let dir = grow_dir_for(f.appearance.layout, f.appearance.sidebar_pos);
            // 侧边栏的框按设计挂在 dock **之外**（与工具提示同口径，见 `item_label_rect`），
            // 既不受卡片横向约束、也不受纵向约束（它不在卡片里）。
            let sidebar = f.appearance.layout == FenceLayout::Sidebar;
            let pad = f.appearance.padding * rt.theme.scale;
            let hspan = (!sidebar)
                .then(|| fence_inner_span(rt, fence, pad))
                .flatten();
            let vspan = (!sidebar)
                .then(|| fence_inner_vspan(rt, fence, pad))
                .flatten();
            Some(rename_box_geometry(
                base,
                dir,
                text,
                metrics,
                hspan,
                vspan,
                (rt.vw, rt.vh),
            ))
        }
        EditTarget::FenceTitle { fence } => {
            // `fence_title_rect` 直接下标取栅栏：越界必须先在这里挡住（返回 None 而非 panic）
            rt.desk.fences.get(fence)?;
            let base = fence_title_rect(rt, fence);
            // 内缩与 `fence_title_rect` 同源（theme.fence_padding）：装得下时原样不动
            let hspan = fence_inner_span(rt, fence, rt.theme.fence_padding)?;
            let vspan = fence_inner_vspan(rt, fence, rt.theme.fence_padding);
            Some(rename_box_geometry(
                base,
                RenameGrow::Right,
                text,
                metrics,
                Some(hspan),
                vspan,
                (rt.vw, rt.vh),
            ))
        }
        _ => None,
    }
}

/// 纯几何流水线：锚点 → 定宽（卡片上限内）→ 折行 → 框高随行数增长 → 钳制。
///
/// 抽成纯函数是为了让「用户实测的两条路径」能脱离 `Runtime` 单测：
/// 长名字在第 0 列不再越出卡片（横向钳制），且**整段内容仍完整可见**（折行而非裁字）。
#[allow(clippy::too_many_arguments)]
fn rename_box_geometry(
    base: RectF,
    dir: RenameGrow,
    text: &str,
    metrics: EditMetrics,
    fence_span: Option<(f32, f32)>,
    fence_vspan: Option<(f32, f32)>,
    view: (f32, f32),
) -> RenameBox {
    let (vw, vh) = view;
    let need = rename_width_for(text, metrics.font, metrics.pad_x);
    // 宽度上限：卡片横向内缘 ∩ 屏幕可用宽（无卡片约束时 = 屏幕可用宽）。
    // **必须先与屏幕取交**：钳制顺序里屏幕那道在最前，若 cap 超过屏幕可用宽，
    // 后面的 `rect.w.min(..)` 会在**折行预算算完之后**把框改窄，
    // 于是「按旧宽折的行」放不进「变窄后的框」→ 又出现裁字。
    let screen_cap = (vw - RENAME_SCREEN_MARGIN * 2.0).max(1.0);
    let cap = match fence_span {
        Some((l, r)) => (r - l).max(1.0).min(screen_cap),
        None => screen_cap,
    };
    let w = rename_box_width(need, base.w, cap);
    let mut rect = sized_rect(base, dir, w);
    // 折行预算：可用宽 ÷ [`WRAP_WIDTH_SLACK`]；下限半个字宽，保证任何情况下每行至少
    // 能放一个字（否则折行会退化成逐字一行）。折行逐字宽度本身已用上界口径
    // （`char_upper_unit`），此处只留一点点余量吸收浮点与抗锯齿。
    let wrap_w = ((rect.w - metrics.pad_x * 2.0).max(metrics.font * 0.5)) / WRAP_WIDTH_SLACK;
    let lines = winbosk_core::text::wrap_visual_lines(text, wrap_w, metrics.font).len();
    // 框高随视觉行数增长（与绘制层 `line_h` 同一倍数），向下长。
    // 仅 ≥2 行时才需要：单行沿用锚点高度（绘制层单行走「整框垂直居中」，与旧行为一致），
    // 否则短名字的框会被这里撑高一截。
    if lines > 1 {
        let need_h =
            lines as f32 * metrics.font * winbosk_render::EDIT_LINE_H_MULT + metrics.pad_y * 2.0;
        rect.h = rect.h.max(need_h);
    }
    // 横向：先钳屏幕（硬边界），再收进卡片（只平移；宽已在上面收到上限内，宽度不变）
    rect.w = rect.w.min(screen_cap);
    rect.x = rect.x.clamp(
        RENAME_SCREEN_MARGIN,
        (vw - rect.w - RENAME_SCREEN_MARGIN).max(RENAME_SCREEN_MARGIN),
    );
    if let Some(span) = fence_span {
        rect = clamp_into_span(rect, span);
    }
    // 纵向：优先留在卡片内（卡片够高时整框上移贴住内下缘），最后钳屏幕
    if let Some((top, bottom)) = fence_vspan {
        if rect.y + rect.h > bottom {
            rect.y = (bottom - rect.h).max(top);
        }
    }
    rect.y = rect.y.clamp(
        RENAME_SCREEN_MARGIN,
        (vh - rect.h - RENAME_SCREEN_MARGIN).max(RENAME_SCREEN_MARGIN),
    );
    RenameBox {
        rect,
        wrap_w,
        lines,
    }
}

/// 框宽口径：不小于锚点宽（网格短名字的框仍是一个整格）、不超过卡片上限；
/// 超出上限的部分由折行承担。
fn rename_box_width(need: f32, base_w: f32, cap: f32) -> f32 {
    need.max(base_w).min(cap.max(1.0))
}

/// 栅栏卡片的横向内缘 `(left, right)`：左右各内缩 `pad`（由调用方给出与锚点矩形**同一个**
/// 内缩量，装得下时钳制恒等、不产生位置漂移）。
fn fence_inner_span(rt: &Runtime, fence: usize, pad: f32) -> Option<(f32, f32)> {
    let f = rt.desk.fences.get(fence)?;
    Some((f.bounds.x + pad, f.bounds.x + f.bounds.w - pad))
}

/// 栅栏卡片的纵向内缘 `(top, bottom)`：上下各内缩 `pad`。
///
/// 高度必须取**真实布局高度**：自动高度栅栏（`bounds.h <= 0`）在数据模型里没有高度，
/// 由 App 层旁路表 `last_layout_h` 提供（见 AGENTS.md 第 8 条），直接读 `bounds.h`
/// 会把卡片判成 0 高、纵向钳制立刻失效。
fn fence_inner_vspan(rt: &Runtime, fence: usize, pad: f32) -> Option<(f32, f32)> {
    let f = rt.desk.fences.get(fence)?;
    let h = if f.bounds.h > 0.0 {
        f.bounds.h
    } else {
        *rt.last_layout_h.get(fence)?
    };
    if h <= 0.0 {
        return None;
    }
    Some((f.bounds.y + pad, f.bounds.y + h - pad))
}

/// 把框横向收进 `[left, right]`：**只平移、不压缩**（压缩就是裁字）。
///
/// 框比区间还宽（区间被 `rename_box_width` 保证不会发生）或区间退化（`right < left`）时，
/// 退化为「左缘贴 `left`」——绝不改成裁字或缩字号。
fn clamp_into_span(r: RectF, (left, right): (f32, f32)) -> RectF {
    let max_x = (right - r.w).max(left);
    RectF {
        x: r.x.clamp(left, max_x),
        ..r
    }
}

/// 编辑框生长方向。
///
/// 判定依据是**框内文本的观感**（编辑文本恒左对齐于 `rect.x + input_pad_x`，见
/// [`click_column`] 的对齐契约），不是「标签/标题原来的绘制对齐方式」：
/// 网格与横向 dock 的标签本就**居中于图标**，框对称生长 + 文本左对齐 ⇒ 文本中心
/// `= 图标中心 − 0.1×文本宽`（余量的一半），整段文本始终「绕着图标中心两侧展开」，
/// 与 Windows 桌面上图标名的观感一致；列表名称列、栅栏标题、左停靠侧边栏的框与
/// 文本本就从栅栏/图标的左缘起算，钉住左缘向右长才不会让文字在打字时整体漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameGrow {
    /// 以中心为锚向两侧定宽（网格标签居中于图标下方；横向 dock 的编辑框居中于图标下方）。
    Centered,
    /// 保持左缘（列表名称列 / 栅栏标题 / 左停靠侧边栏）。
    Right,
    /// 保持右缘（右停靠侧边栏：编辑框挂在图标左侧，右缘贴图标，向右长会盖住 dock）。
    Left,
}

/// 生长方向由布局与停靠边唯一决定——**不是**每处自己 `match` 的局部约定。
fn grow_dir_for(layout: FenceLayout, pos: SidebarPosition) -> RenameGrow {
    match layout {
        FenceLayout::Grid => RenameGrow::Centered,
        FenceLayout::List => RenameGrow::Right,
        FenceLayout::Sidebar => match pos {
            SidebarPosition::Left => RenameGrow::Right,
            SidebarPosition::Top => RenameGrow::Centered,
            SidebarPosition::Right => RenameGrow::Left,
        },
    }
}

/// 按方向把框**定宽**到 `want_w`（只动宽度与相应的一侧锚点）。
///
/// 与「只增不减」的旧写法不同：宽度上限来自卡片，宽于上限的文本改走折行，
/// 所以这里允许比锚点窄（退化卡片下 `want_w` 可能小于锚点宽）。
fn sized_rect(mut base: RectF, dir: RenameGrow, want_w: f32) -> RectF {
    let want = want_w.max(1.0);
    match dir {
        RenameGrow::Centered => base.x += (base.w - want) / 2.0,
        RenameGrow::Right => {}
        RenameGrow::Left => base.x += base.w - want,
    }
    base.w = want;
    base
}

/// 编辑框左右内边距（物理像素，已随 DPI 缩放）。
fn pad_x(rt: &Runtime) -> f32 {
    rt.theme.controls.input_pad_x
}

/// 「完整显示 `text` 所需的最小框宽」= 文本**上界**宽 × [`TEXT_WIDTH_SLACK`] + 左右内边距。
///
/// 用 `upper_width`（逐字形上界）而不是平均口径 `label_width`：全大写或含大量
/// `W/M/@/m/w` 的文件名平均字宽可达 0.9 em 以上，按平均口径算出来的框会让 DirectWrite
/// 画出的字形溢出框右缘、被静默裁掉。抽成纯函数是为了把这条口径钉进单测。
fn rename_width_for(text: &str, font_size: f32, pad_x: f32) -> f32 {
    winbosk_core::text::upper_width(text, font_size) * crate::scene::TEXT_WIDTH_SLACK + pad_x * 2.0
}

/// 逐帧回写就地重命名框：文本一变（打字 / 退格 / 粘贴 / IME 上屏）框就跟着变宽、
/// 变到卡片上限后转为增加行数，输入框内始终完整显示全部内容，且不越出卡片。
///
/// 由 `build_scene` 在绘制前调用，因此**本帧**的绘制、窗口区域（`build_region`）与命中热区
/// （`HitModel::edit_rect`）共用同一个矩形与同一个折行预算。
/// 几何无变化时直接返回，不白做 IME 窗口重定位。
pub(crate) fn refresh_rename_rect(rt: &mut Runtime) {
    let Some((target, text)) = rt.edit.as_ref().map(|e| (e.target, e.display_text())) else {
        return;
    };
    let Some(bbox) = rename_edit_rect(rt, target, &text) else {
        return;
    };
    let same = rt.edit.as_ref().map(|e| (e.rect, e.wrap_w)) == Some((bbox.rect, Some(bbox.wrap_w)));
    if same {
        return;
    }
    if let Some(e) = rt.edit.as_mut() {
        e.rect = bbox.rect;
        e.wrap_w = Some(bbox.wrap_w);
    }
    position_ime_window(rt);
}

/// 栅栏标题文本矩形（就地编辑框的定位基准）。
pub(crate) fn fence_title_rect(rt: &Runtime, fence: usize) -> RectF {
    let f = &rt.desk.fences[fence];
    let pad = rt.theme.fence_padding;
    // 编辑框按实际绘制字号（`draw_inline_edit` 恒用 label 格式）定高：
    // 文本行高 + 上下剪裁余量（绘制时内缩），字形完整显示且垂直居中。
    let h = rt.theme.label.size * 1.6 + rt.theme.controls.input_pad_y * 2.0;
    RectF {
        x: f.bounds.x + pad,
        y: f.bounds.y + pad,
        w: (f.bounds.w - 2.0 * pad).max(1.0),
        h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_extension_tokens() {
        let text = " .pdf, JPG，  .PNG ; txt  ,, ";
        let tokens = parse_extension_tokens(text);
        assert_eq!(tokens, vec!["pdf", "jpg", "png", "txt"]);

        // 测试空输入
        assert!(parse_extension_tokens("   ").is_empty());
        assert!(parse_extension_tokens(",，;;").is_empty());
    }

    #[test]
    fn test_parse_pattern_tokens() {
        let text = " report* ,  *2024* ， draft?.docx ; test space ";
        let tokens = parse_pattern_tokens(text);
        assert_eq!(
            tokens,
            vec!["report*", "*2024*", "draft?.docx", "test space"]
        );

        // 测试空输入
        assert!(parse_pattern_tokens("   ").is_empty());
        assert!(parse_pattern_tokens(",，;;").is_empty());
    }

    fn rect(x: f32, w: f32) -> RectF {
        RectF {
            x,
            y: 100.0,
            w,
            h: 20.0,
        }
    }

    /// 定宽锚点：网格保持中心、列表/标题保持左缘、右停靠侧边栏保持右缘；
    /// 宽度等于锚点宽时几何必须逐字段不变（逐帧刷新的幂等底线）。
    #[test]
    fn sized_rect_anchors_and_identity() {
        let base = rect(50.0, 80.0);
        assert_eq!(sized_rect(base, RenameGrow::Centered, 80.0), base);
        assert_eq!(sized_rect(base, RenameGrow::Right, 80.0), base);
        assert_eq!(sized_rect(base, RenameGrow::Left, 80.0), base);

        let c = sized_rect(base, RenameGrow::Centered, 200.0);
        assert!((c.x + c.w / 2.0 - (base.x + base.w / 2.0)).abs() < 1e-3);
        assert_eq!(c.w, 200.0);
        let r = sized_rect(base, RenameGrow::Right, 200.0);
        assert_eq!(r.x, base.x);
        let l = sized_rect(base, RenameGrow::Left, 200.0);
        assert!((l.x + l.w - (base.x + base.w)).abs() < 1e-3);
        // 允许比锚点窄（卡片上限小于格宽时）：这正是「改走折行」的前提
        assert_eq!(sized_rect(base, RenameGrow::Right, 40.0).w, 40.0);
    }

    /// 框宽口径：不小于锚点宽、不超过卡片上限。
    #[test]
    fn rename_box_width_is_capped() {
        assert_eq!(rename_box_width(200.0, 72.0, 360.0), 200.0);
        assert_eq!(
            rename_box_width(40.0, 72.0, 360.0),
            72.0,
            "短名字不得小于格宽"
        );
        assert_eq!(
            rename_box_width(500.0, 72.0, 360.0),
            360.0,
            "超宽必须收到卡片上限"
        );
        assert_eq!(
            rename_box_width(500.0, 72.0, 0.0),
            1.0,
            "退化上限不得出现 0 宽"
        );
    }

    /// 内容宽度口径：必须容得下「文本 + 左右内边距」，且带安全余量（估算偏乐观时兜底）。
    #[test]
    fn rename_width_covers_text_and_padding() {
        let (font, pad) = (12.0, 8.0);
        let text = "一个很长的文件名示例.docx";
        let text_w = crate::scene::label_width(text, font);
        let w = rename_width_for(text, font, pad);
        assert!(w > text_w + pad * 2.0);
        assert!(
            (w - (text_w * crate::scene::TEXT_WIDTH_SLACK + pad * 2.0)).abs() < 1e-3,
            "重命名框必须复用全局余量，不许另写一个字面量"
        );
    }

    /// 空文本：只剩左右内边距，不得为 0（否则框会缩成一条线看不见）。
    #[test]
    fn rename_width_of_empty_text_is_padding_only() {
        assert_eq!(rename_width_for("", 12.0, 8.0), 16.0);
    }

    /// 回归：网格格宽装不下的长名字，所需框宽必须真的超过格宽——这正是「框不生长就必然
    /// 裁字」的量化证据；顺带锁住 `TEXT_WIDTH_SLACK` 不被压到 1.0。
    #[test]
    fn rename_width_exceeds_grid_cell_for_long_name() {
        let theme = Theme::default();
        let (font, pad) = (theme.label.size, theme.controls.input_pad_x);
        let cell_w = crate::scene::grid_cell_w(theme.icon_size, theme.icon_gap);
        let long = "项目文档归档2026年10月最终版.docx";
        assert!(
            crate::scene::label_width(long, font) > cell_w,
            "前提失效：这个名字应当装不进一个格"
        );
        assert!(rename_width_for(long, font, pad) > cell_w);
        assert!(rename_width_for(long, font, pad) > crate::scene::label_width(long, font));
    }

    /// 生长方向矩阵：网格居中、列表向右、侧边栏按停靠边反向。
    #[test]
    fn rename_grow_dir_matrix() {
        assert_eq!(
            grow_dir_for(FenceLayout::Grid, SidebarPosition::Left),
            RenameGrow::Centered
        );
        assert_eq!(
            grow_dir_for(FenceLayout::List, SidebarPosition::Right),
            RenameGrow::Right
        );
        assert_eq!(
            grow_dir_for(FenceLayout::Sidebar, SidebarPosition::Left),
            RenameGrow::Right
        );
        assert_eq!(
            grow_dir_for(FenceLayout::Sidebar, SidebarPosition::Top),
            RenameGrow::Centered
        );
        assert_eq!(
            grow_dir_for(FenceLayout::Sidebar, SidebarPosition::Right),
            RenameGrow::Left
        );
    }

    /// 对齐契约：`click_column` 是宽度累计函数的逆——给定文本左缘，点击第 N 个字符的
    /// 估算位置必须定位到第 N 列；且该左缘就是绘制/光标/IME 共用的 `rect.x + input_pad_x`。
    /// 这条断言把「rect 只描述框、不得另立文本原点」钉死（任何居中/向左定位都必须让
    /// 文本实际画在 `rect.x + pad_x` 上，否则「看到第 N 个字、点到第 M 个字」）。
    #[test]
    fn click_column_is_inverse_of_text_layout() {
        let font = 12.0;
        let pad_x = 8.0;
        let line = "一个很长的文件名示例.docx";
        // 居中定宽后的框：文本左缘仍严格等于 rect.x + pad_x
        let icon_center = 500.0;
        let base = rect(icon_center - 36.0, 72.0); // 格宽 72、图标中心 500
        let text_w = crate::scene::label_width(line, font);
        let grown = sized_rect(
            base,
            RenameGrow::Centered,
            text_w * crate::scene::TEXT_WIDTH_SLACK + pad_x * 2.0,
        );
        let text_left = grown.x + pad_x;
        // 逐字累计：点到第 N 个字的前半格 → 第 N 列；后半格 → 第 N+1 列
        let mut acc = 0.0;
        for (n, c) in line.chars().enumerate() {
            let w = label_width(&c.to_string(), font);
            let mid = text_left + acc + w * 0.25;
            assert_eq!(
                click_column(mid, text_left, line, font),
                n,
                "点在第 {n} 个字的前半格应停在第 {n} 列"
            );
            let late = text_left + acc + w * 0.75;
            assert_eq!(
                click_column(late, text_left, line, font),
                n + 1,
                "点在第 {n} 个字的四分之三处应进到第 {} 列",
                n + 1
            );
            acc += w;
        }
        // 点在文本左缘之前（框内左侧内边距）→ 恒为第 0 列
        assert_eq!(click_column(text_left - 5.0, text_left, line, font), 0);
    }

    /// 卡片内约束：框装得下时**只平移**、必须完整落在 `[left, right]` 内，宽度一个像素都不动。
    #[test]
    fn clamp_into_span_translates_only() {
        let span = (100.0, 400.0);
        // 居中定位把左缘推到卡片外 → 平移回来，宽度不变
        let out = clamp_into_span(rect(10.0, 252.0), span);
        assert_eq!(out.x, 100.0);
        assert_eq!(out.w, 252.0);
        assert!(out.x + out.w <= 400.0 + 1e-3);
        // 右缘越界 → 左移到贴右缘，宽度不变
        let out = clamp_into_span(rect(300.0, 252.0), span);
        assert!((out.x + out.w - 400.0).abs() < 1e-3);
        assert_eq!(out.w, 252.0);
        // 已经装得下 → 原样不动（幂等：逐帧刷新不会把框越推越偏）
        let inside = rect(120.0, 200.0);
        assert_eq!(clamp_into_span(inside, span), inside);
        // 防御性：区间退化（卡片窄于两倍内边距）时退化为「左缘贴 left」，不 panic、不出 NaN
        let degen = clamp_into_span(rect(-50.0, 40.0), (100.0, 60.0));
        assert_eq!(degen.x, 100.0);
        assert_eq!(degen.w, 40.0);
    }

    /// **用户实测回归点**：窄卡片 + 长名字（截图场景）——框宽收到卡片上限后，
    /// 内容**换行**完整显示：整框不出卡片，且每一视觉行的估算宽度都在框内可用宽之内
    /// （即「没有任何一个字被裁掉」）。
    #[test]
    fn long_name_wraps_inside_narrow_fence() {
        let metrics = EditMetrics {
            font: 12.0,
            pad_x: 8.0,
            pad_y: 4.0,
        };
        let fence_span = (40.0, 240.0); // 卡片内容内缘：宽 200
        let base = rect(40.0, 72.0); // 第 0 列格：左缘 = 卡片内缘
        let text = "launch-author.bat - 快捷方式";
        let out = rename_box_geometry(
            base,
            RenameGrow::Centered,
            text,
            metrics,
            Some(fence_span),
            None,
            (1920.0, 1080.0),
        );
        assert!(out.rect.x >= fence_span.0 - 1e-3, "左缘不得越过卡片");
        assert!(
            out.rect.x + out.rect.w <= fence_span.1 + 1e-3,
            "右缘不得越过卡片"
        );
        assert_eq!(out.rect.w, 200.0, "宽度必须收到卡片上限");
        assert!(
            out.lines >= 2,
            "装不下时必须折行，而不是裁字：{:?}",
            out.lines
        );
        // 每行内容都装得进框内可用宽（无裁字），且框高覆盖全部行
        let avail = out.rect.w - metrics.pad_x * 2.0;
        let wrapped = winbosk_core::text::wrap_visual_lines(text, out.wrap_w, metrics.font);
        let mut covered = 0usize;
        for l in &wrapped {
            let seg: String = text.chars().skip(l.start).take(l.end - l.start).collect();
            let w = crate::scene::label_width(&seg, metrics.font);
            assert!(
                w <= avail + 1e-3,
                "视觉行 {seg:?} 宽 {w} 超出框内可用宽 {avail}"
            );
            covered += l.end - l.start;
        }
        assert_eq!(covered, text.chars().count(), "折行不得丢字");
        let need_h = wrapped.len() as f32 * metrics.font * winbosk_render::EDIT_LINE_H_MULT
            + metrics.pad_y * 2.0;
        assert!(
            out.rect.h >= need_h - 1e-3,
            "框高 {:?} 必须覆盖 {} 行所需的 {need_h}",
            out.rect.h,
            wrapped.len()
        );
    }

    /// 宽卡片 + 长名字：一行装得下就不折行（框高保持锚点高度，避免短名字/中等名字的框变高）。
    #[test]
    fn name_that_fits_does_not_wrap() {
        let metrics = EditMetrics {
            font: 12.0,
            pad_x: 8.0,
            pad_y: 4.0,
        };
        let base = rect(40.0, 72.0);
        let text = "launch-author.bat - 快捷方式";
        let out = rename_box_geometry(
            base,
            RenameGrow::Centered,
            text,
            metrics,
            Some((40.0, 640.0)),
            None,
            (1920.0, 1080.0),
        );
        assert_eq!(out.lines, 1);
        assert_eq!(out.rect.h, base.h, "单行不得改变锚点高度");
    }

    /// 折行框贴到卡片底部时整框上移（不越出卡片下缘）；卡片本身就装不下时保持锚点并
    /// 交给屏幕钳制，绝不 panic / 出 NaN。
    #[test]
    fn wrapped_box_clamps_into_fence_vertically() {
        let metrics = EditMetrics {
            font: 12.0,
            pad_x: 8.0,
            pad_y: 4.0,
        };
        let base = RectF {
            x: 40.0,
            y: 300.0,
            w: 72.0,
            h: 40.0,
        };
        let text = "一个很长很长的文件名示例文档2026年最终版.docx";
        let out = rename_box_geometry(
            base,
            RenameGrow::Centered,
            text,
            metrics,
            Some((40.0, 240.0)),
            Some((20.0, 340.0)), // 卡片内缘：底 340
            (1920.0, 1080.0),
        );
        assert!(out.rect.y + out.rect.h <= 340.0 + 1e-3, "整框必须落进卡片");
        assert!(out.rect.y >= 20.0 - 1e-3, "上移不得越过卡片上缘");
        // 卡片太矮（高度不够）时：贴卡片上缘、允许向下溢出，仍留在屏幕内
        let out = rename_box_geometry(
            base,
            RenameGrow::Centered,
            text,
            metrics,
            Some((40.0, 240.0)),
            Some((20.0, 60.0)),
            (1920.0, 1080.0),
        );
        assert!(out.rect.y >= 20.0 - 1e-3);
        assert!(out.rect.y + out.rect.h <= 1080.0 - RENAME_SCREEN_MARGIN + 1e-3);
    }

    /// 屏幕钳制：卡片约束缺席（侧边栏挂在 dock 外）时，仍不得越出虚拟屏幕。
    #[test]
    fn sidebar_rename_box_clamps_to_screen_only() {
        let metrics = EditMetrics {
            font: 12.0,
            pad_x: 8.0,
            pad_y: 4.0,
        };
        let base = rect(4.0, 60.0);
        let out = rename_box_geometry(
            base,
            RenameGrow::Left,
            "一个很长很长的文件名示例文档2026年最终版.docx",
            metrics,
            None,
            None,
            (800.0, 600.0),
        );
        assert!(out.rect.x >= RENAME_SCREEN_MARGIN - 1e-3);
        assert!(out.rect.x + out.rect.w <= 800.0 - RENAME_SCREEN_MARGIN + 1e-3);
        assert!(out.rect.w <= 800.0 - RENAME_SCREEN_MARGIN * 2.0 + 1e-3);
    }

    /// **钳制顺序回归点**：卡片横向内缘比屏幕可用宽还宽时，屏幕那道必须先与上限取交——
    /// 否则折行按「旧宽」算完，横向钳制又把框改窄，折出来的行就放不进框里（又裁字）。
    #[test]
    fn cap_never_exceeds_screen_so_wrap_matches_final_width() {
        let metrics = EditMetrics {
            font: 12.0,
            pad_x: 8.0,
            pad_y: 4.0,
        };
        let vw = 200.0;
        let text = "一个比较长的文件名示例.docx";
        // 卡片横向内缘越出屏幕（贴边栅栏），宽度上限必须被屏幕收住
        let out = rename_box_geometry(
            rect(-40.0, 72.0),
            RenameGrow::Right,
            text,
            metrics,
            Some((-50.0, 250.0)),
            None,
            (vw, 600.0),
        );
        let avail = out.rect.w - metrics.pad_x * 2.0;
        let expected = avail.max(metrics.font * 0.5) / WRAP_WIDTH_SLACK;
        assert!(
            (out.wrap_w - expected).abs() < 1e-3,
            "折行预算必须按**最终**框宽算：wrap_w={} 期望={expected}",
            out.wrap_w
        );
        let wrapped = winbosk_core::text::wrap_visual_lines(text, out.wrap_w, metrics.font);
        for l in &wrapped {
            let seg: String = text.chars().skip(l.start).take(l.end - l.start).collect();
            let w = crate::scene::label_width(&seg, metrics.font);
            assert!(
                w <= avail + 1e-3,
                "行 {seg:?} 宽 {w} 超出最终框内可用宽 {avail}"
            );
        }
    }

    /// 折行预算：比框内可用宽保守（÷ `WRAP_WIDTH_SLACK`），并带半个字宽的下限。
    #[test]
    fn wrap_budget_is_conservative() {
        let metrics = EditMetrics {
            font: 12.0,
            pad_x: 8.0,
            pad_y: 4.0,
        };
        let out = rename_box_geometry(
            rect(40.0, 72.0),
            RenameGrow::Right,
            "中等长度的名字.txt",
            metrics,
            Some((40.0, 400.0)),
            None,
            (1920.0, 1080.0),
        );
        let avail = out.rect.w - metrics.pad_x * 2.0;
        assert!((out.wrap_w - avail / WRAP_WIDTH_SLACK).abs() < 1e-3);
        assert!(out.wrap_w < avail);
        // 极窄框：预算下限为半个字宽（否则折行退化成逐字一行）
        let tiny = rename_box_geometry(
            rect(40.0, 10.0),
            RenameGrow::Right,
            "宽",
            metrics,
            Some((40.0, 42.0)),
            None,
            (1920.0, 1080.0),
        );
        assert!(tiny.wrap_w >= metrics.font * 0.5 / WRAP_WIDTH_SLACK - 1e-3);
    }

    /// **回归（审查 F1）**：宽字形名字（全大写 / 大量 `W M @ m w`）必须真的装得下——
    /// 框宽与折行都按**上界**口径算，逐行上界宽 ≤ 框内可用宽（⇒ 真实排版宽度也 ≤ 可用宽）。
    /// 旧口径（平均 0.62 em）在这类名字上会「以为装得下」而完全不折行，末尾被静默裁掉。
    #[test]
    fn wide_glyph_names_fit_without_clipping() {
        let metrics = EditMetrics {
            font: 12.0,
            pad_x: 8.0,
            pad_y: 4.0,
        };
        for text in [
            "WWWWWWWWWWWWWWWWWWWW",
            "WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW.txt",
            "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@",
            "MMMMMMMMMM.doc",
            "mmmmmmmmmmmmmmmmmm.txt",
            "PROJEKT-MUSTERDATEI-2026.DOCX",
        ] {
            for cap in [800.0, 300.0, 200.0, 120.0] {
                let base = rect(40.0, 72.0);
                let out = rename_box_geometry(
                    base,
                    RenameGrow::Right,
                    text,
                    metrics,
                    Some((40.0, 40.0 + cap)),
                    None,
                    (1920.0, 1080.0),
                );
                let avail = out.rect.w - metrics.pad_x * 2.0;
                assert!(
                    out.rect.w <= cap + 1e-3,
                    "{text:?} cap={cap}: 框宽 {:?} 越出卡片",
                    out.rect.w
                );
                let wrapped = winbosk_core::text::wrap_visual_lines(text, out.wrap_w, metrics.font);
                assert_eq!(
                    wrapped.iter().map(|l| l.end - l.start).sum::<usize>(),
                    text.chars().count(),
                    "{text:?}: 折行不得丢字"
                );
                for l in &wrapped {
                    let seg: String = text.chars().skip(l.start).take(l.end - l.start).collect();
                    // 上界宽必须装得下 ⇒ 真实排版宽（≤ 上界）也装得下
                    let uw = winbosk_core::text::upper_width(&seg, metrics.font);
                    assert!(
                        uw <= avail + 1e-3,
                        "{text:?} cap={cap}: 行 {seg:?} 上界宽 {uw} 超出可用宽 {avail}"
                    );
                    // 旧的平均口径在这些名字上确实会低估（否则本测试没有意义）
                    let avg = crate::scene::label_width(&seg, metrics.font);
                    assert!(avg <= uw + 1e-3, "上界口径必须 ≥ 平均口径");
                }
            }
        }
        // 对照：旧口径（平均宽）在 `W×20` 上确实「以为一行装得下」
        let wide = "WWWWWWWWWWWWWWWWWWWW";
        let avg_need = crate::scene::label_width(wide, metrics.font)
            * crate::scene::TEXT_WIDTH_SLACK
            + metrics.pad_x * 2.0;
        assert!(
            winbosk_core::text::upper_width(wide, metrics.font) * crate::scene::TEXT_WIDTH_SLACK
                + metrics.pad_x * 2.0
                > avg_need,
            "上界口径必须比平均口径更宽，否则 F1 没修"
        );
    }

    /// 网格的居中定宽必须让整段文本「绕着图标中心两侧展开」：文本中心与图标中心的
    /// 偏差只来自余量的一半（0.1×文本宽），不随文本变长而漂向一侧。
    /// 若有人把网格改成单向右长，这条断言会立刻失败（长名字会整段右偏）。
    #[test]
    fn grid_growth_keeps_text_centered_on_icon() {
        let font = 12.0;
        let pad_x = 8.0;
        let icon_center = 500.0;
        let base = rect(icon_center - 36.0, 72.0);
        for name in ["短名.txt", "一个很长的文件名示例文档2026.docx"] {
            let text_w = crate::scene::label_width(name, font);
            let need = text_w * crate::scene::TEXT_WIDTH_SLACK + pad_x * 2.0;
            let grown = sized_rect(base, RenameGrow::Centered, need.max(base.w));
            let text_center = grown.x + pad_x + text_w / 2.0;
            let drift = (text_center - icon_center).abs();
            assert!(
                drift <= text_w * 0.1 + 1e-3,
                "{name}: 文本中心偏离图标中心 {drift}（上限 {}）",
                text_w * 0.1
            );
        }
    }

    /// 视觉折行与光标口径：合成串参与折行、光标落在合成串之后；行尾光标停在本行。
    #[test]
    fn visual_lines_and_caret_index_follow_wrap() {
        let mut e = InlineEdit {
            target: EditTarget::Item { fence: 0, icon: 0 },
            rect: rect(0.0, 120.0),
            lines: vec!["一二三四五六七八".into()], // 8 个全角字
            line: 0,
            col: 2,
            placeholder: String::new(),
            single_line: true,
            wrap_w: Some(48.0), // 每行 4 个全角字（12px 字号 × 4 = 48）
            focused: true,
            composing: true,
            comp: "甲".into(),
            committing: false,
        };
        assert_eq!(e.display_text(), "一二甲三四五六七八");
        assert_eq!(e.caret_index(), 3, "光标跟在合成串之后");
        let lines = e.visual_lines(12.0);
        assert_eq!(lines.len(), 3, "9 个字按每行 4 个折成 3 行: {lines:?}");
        assert_eq!(
            winbosk_core::text::visual_line_of(&lines, e.caret_index()),
            0,
            "光标仍在第 0 行"
        );
        // 光标移到末尾 → 末行（8 个字按每行 4 个折成 2 行，行尾光标停在末行）
        e.comp.clear();
        e.composing = false;
        e.col = 8;
        let lines = e.visual_lines(12.0);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            winbosk_core::text::visual_line_of(&lines, e.caret_index()),
            1
        );
        // 折行边界：第 4 列（第一行行尾）停在第 0 行，不跳到下一行开头
        e.col = 4;
        let lines = e.visual_lines(12.0);
        assert_eq!(
            winbosk_core::text::visual_line_of(&lines, e.caret_index()),
            0
        );
        // 不折行（规则输入框）：恒单行
        e.wrap_w = None;
        assert_eq!(e.visual_lines(12.0).len(), 1);
    }

    /// 折行框内的上下键 = 视觉行移动（保持行内偏移；合成期间不介入；越界不动作）。
    #[test]
    fn visual_line_cursor_moves_between_wrapped_lines() {
        let mut e = InlineEdit {
            target: EditTarget::Item { fence: 0, icon: 0 },
            rect: rect(0.0, 120.0),
            lines: vec!["一二三四五六七八九十".into()], // 10 个全角字
            line: 0,
            col: 1,
            placeholder: String::new(),
            single_line: true,
            wrap_w: Some(48.0), // 每行 4 个字
            focused: true,
            composing: false,
            comp: String::new(),
            committing: false,
        };
        assert_eq!(e.visual_lines(12.0).len(), 3, "10 字按每行 4 字折成 3 行");
        e.move_visual_line(12.0, true);
        assert_eq!(e.col, 5, "下移一行保持行内偏移 1");
        e.move_visual_line(12.0, true);
        assert_eq!(e.col, 9);
        e.move_visual_line(12.0, true);
        assert_eq!(e.col, 9, "已在末行：不动作");
        e.move_visual_line(12.0, false);
        assert_eq!(e.col, 5);
        // 目标行更短时贴行尾（末行只有 2 个字：从第 0 行偏移 3 下移 → 贴到 10）
        e.col = 3;
        e.move_visual_line(12.0, true);
        assert_eq!(e.col, 7, "第二行仍有 4 个字符");
        e.move_visual_line(12.0, true);
        assert_eq!(e.col, 10, "末行只有 2 个字符 → 贴行尾");
        // 合成期间不抢方向键
        e.composing = true;
        e.comp = "甲".into();
        e.col = 0;
        let before = e.col;
        e.move_visual_line(12.0, true);
        assert_eq!(e.col, before);
    }

    /// 折行框内点击定位：y 判行 + 行内 x 定位 + 合成串区间折算。**user 场景**：点第 2 行的
    /// 第 2 个字必须落到那一列，而不是按整串宽度算出的别处。
    #[test]
    fn wrapped_click_lands_on_the_visual_line_under_the_cursor() {
        let (font, pad_x, pad_y) = (12.0, 8.0, 4.0);
        let line_h = font * winbosk_render::EDIT_LINE_H_MULT;
        let rect = RectF {
            x: 100.0,
            y: 50.0,
            w: 80.0,
            h: 70.0,
        };
        let text = "一二三四五六七八九十"; // 10 个全角字
        let visual = winbosk_core::text::wrap_visual_lines(text, 48.0, font); // 每行 4 字 → 3 行
        assert_eq!(visual.len(), 3);
        // 点击 x 必须按**视觉行内**偏移算（文本左缘 = rect.x + pad_x，每行都从该处起排）
        let x_in = |vi: usize, off: usize| {
            let prefix: String = text.chars().skip(visual[vi].start).take(off).collect();
            rect.x + pad_x + crate::scene::label_width(&prefix, font)
        };
        let y_of = |row: f32| rect.y + pad_y + row * line_h;
        // 第 0 行：点第 2 个字的前半格 → 第 1 列
        assert_eq!(
            click_column_wrapped(
                x_in(0, 1),
                y_of(0.5),
                rect,
                text,
                &visual,
                0,
                0,
                font,
                pad_x,
                pad_y,
                line_h
            ),
            1
        );
        // 第 1 行行内第 1 格 → 4 + 1 = 5：**不是**按整串宽度算出来的别处
        assert_eq!(
            click_column_wrapped(
                x_in(1, 1),
                y_of(1.5),
                rect,
                text,
                &visual,
                0,
                0,
                font,
                pad_x,
                pad_y,
                line_h
            ),
            5
        );
        // 第 2 行（末行只有 2 个字）：点在整行右端之外 → 贴到 10
        assert_eq!(
            click_column_wrapped(
                x_in(2, 2) + 100.0,
                y_of(2.5),
                rect,
                text,
                &visual,
                0,
                0,
                font,
                pad_x,
                pad_y,
                line_h
            ),
            10
        );
        // y 落在框下方（越界）→ 末行；y 在框上方 → 第 0 行
        assert_eq!(
            click_column_wrapped(
                x_in(2, 2),
                y_of(99.0),
                rect,
                text,
                &visual,
                0,
                0,
                font,
                pad_x,
                pad_y,
                line_h
            ),
            10
        );
        assert_eq!(
            click_column_wrapped(
                x_in(0, 0),
                y_of(-5.0),
                rect,
                text,
                &visual,
                0,
                0,
                font,
                pad_x,
                pad_y,
                line_h
            ),
            0
        );
        // 合成串「甲乙」插在第 2 列（逻辑列 2）：显示串 = 一二甲乙三四五六七八九十
        let composed = "一二甲乙三四五六七八九十";
        let visual_c = winbosk_core::text::wrap_visual_lines(composed, 48.0, font);
        assert_eq!(visual_c.len(), 3);
        // 点击 x 仍按**该视觉行内**偏移算（每行都从 rect.x + pad_x 起排）
        let x_seg = |vi: usize, off: usize| {
            let prefix: String = composed
                .chars()
                .skip(visual_c[vi].start)
                .take(off)
                .collect();
            rect.x + pad_x + crate::scene::label_width(&prefix, font)
        };
        // 点在合成串之后：显示串下标 = 4（第 1 行起点）+ 1 = 5 → 逻辑列 5 − 2 = 3
        assert_eq!(
            click_column_wrapped(
                x_seg(1, 1),
                y_of(1.5),
                rect,
                composed,
                &visual_c,
                2,
                2,
                font,
                pad_x,
                pad_y,
                line_h
            ),
            3,
            "显示串下标 5 − 合成串 2 = 逻辑列 3"
        );
        // 点进合成串内部（显示串下标 3）→ 回落到合成起点（模型没有「合成内光标」状态）
        assert_eq!(
            click_column_wrapped(
                x_seg(0, 3),
                y_of(0.5),
                rect,
                composed,
                &visual_c,
                2,
                2,
                font,
                pad_x,
                pad_y,
                line_h
            ),
            2
        );
    }

    /// 绘制文本口径：IME 合成串必须插在光标处，且与 `draw_inline_edit` 的拼接一致。
    #[test]
    fn display_text_inserts_composition_at_caret() {
        let mut e = InlineEdit {
            target: EditTarget::FenceTitle { fence: 0 },
            rect: rect(0.0, 80.0),
            lines: vec!["abc".into()],
            line: 0,
            col: 1,
            placeholder: String::new(),
            single_line: true,
            wrap_w: None,
            focused: true,
            composing: true,
            comp: "XY".into(),
            committing: false,
        };
        assert_eq!(e.display_text(), "aXYbc");
        e.comp.clear();
        assert_eq!(e.display_text(), "abc");
        e.col = 99; // 越界光标（异常路径）不得 panic，且等价于行尾
        assert_eq!(e.display_text(), "abc");
    }
}
