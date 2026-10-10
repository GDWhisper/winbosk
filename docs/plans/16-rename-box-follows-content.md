# 实施记录 16：就地重命名输入框——宽度有界 + 内容折行完整显示

> 两轮用户实测把口径逼到位：**①内容不能截断 → ②框不能越出栅栏卡片 → ③超宽内容换行显示**。
> 本文是该口径的唯一记录，含对抗性审查结论与否决项（§9/§10）。

## 1. 背景与根因

就地重命名（右键「重命名」/ 双击标题）三个连续缺陷：

| # | 现象（用户实测） | 根因 |
| :-- | :--- | :--- |
| 1 | 长名字在输入框里被**静默切掉**，看不到全貌 | 框宽 = 一个网格格宽（`grid_cell_w` ≈ 1.5×图标宽），且**打开后再不更新**；绘制走 `D2D1_DRAW_TEXT_OPTIONS_CLIP`，超出即裁（连「…」都没有） |
| 2 | 框**越出栅栏卡片**（截图：第 0 列的框左半边在卡片外） | 修①时采用「以图标中心对称生长」且无上限 → 框宽只受虚拟屏幕约束 |
| 3 | 需要的是**多行显示**，不是把框撑大 | 修②时框宽被卡片上限截住后，仍有内容放不下（只能折行） |

另有两道**独立**的裁剪口，缺一即「框看不见 / 被切一截」（同 plan 07 的占位框教训）：

1. **窗口区域** `overlay::build_region`（`SetWindowRgn`）：原先已并入 `model.edit_rect` ✓；
2. **合成表面** `Scene::content_rect`：**原先漏并编辑框** → 框长出栅栏包围盒（外加 `SURFACE_GROW` 余量）后
   被表面切掉。本次补上（框的**高度**同样会超出单行，所以这一条对折行更关键）。

---

## 2. 目标与非目标

**Goals**

- 输入框内**完整显示**当前全部文本（含 IME 合成串），任何时刻都不裁字；
- 框宽随文本生长，但**上限是栅栏卡片横向内缘**（侧边栏挂在 dock 之外，上限为虚拟屏幕）；
- 到达上限后**折行**继续显示（框高随视觉行数增长），而不是裁字 / 缩字号 / 横向滚动；
- 折行后光标、鼠标点击、IME 候选窗、上下方向键都落在**所见的那一行那个字**上。

**Non-Goals**

- 不改文件名标签的省略策略（网格两行中段省略、保住扩展名，见 `draw.rs::wrap_two_lines`）；
- 不改规则输入框（`RuleExtension` 等）几何——它们的矩形由控制台布局逐帧回写，**不折行**；
- 不做单词/标点级避让（与标签折行同策略：按上界宽度的贪心断行）；
- 不引入横向滚动条（Windows 的 `EDIT` 在极窄时的兜底行为；本工程用折行覆盖同一场景）。

**残留边界**（有意不覆盖，逐条都可复现，见 §10 审查记录）：

1. **卡片比折行框还矮时，框纵向必然下溢出卡片**：能在卡片内收下就整框上移贴住内下缘，
   收不下（例：200px 宽卡片 + 100 字中文名 → 9 行 → 框高 170px）时贴卡片上缘向下溢出，
   由屏幕钳制兜底。**横向不越卡片**在数学上成立（`w ≤ cap ≤ 卡片内宽`），纵向不成立。
2. **行内光标/点击横向仍用平均口径**（`click_column` / `edit_caret_point` 用 `estimate_width`，
   绘制光标用 DirectWrite 实测）：折行把误差限制在**单个视觉行内**（宽 ASCII 行尾最多偏 1~3 个字形）；
   这是本特性之前就有的口径差，不阻断。
3. **IME 合成串正好跨视觉行时点击落点可能差一行**：点在下一行行首得到的位置若落在合成串区间内，
   会折算回合成串起点（上一行）。仅「合成期间 + 折行点恰好落在合成串内」可复现。
4. **合成表面重建失败**（极端 GPU 拓扑）会沿用旧表面，此时编辑框新并入的部分不画（既有降级行为）。

---

## 3. 状态契约与几何口径

```rust
// core（App 与 Render 共用的唯一折行口径）
pub struct VisualLine { pub start: usize, pub end: usize }        // 字符区间，左闭右开
pub fn wrap_visual_lines(text: &str, max_w: f32, font_size: f32) -> Vec<VisualLine>;
pub fn visual_line_of(lines: &[VisualLine], col: usize) -> usize;  // 行尾光标停在本行

// app
pub(crate) struct InlineEdit { …, pub wrap_w: Option<f32>, … }     // Some = 折行（重命名）
pub(crate) struct RenameBox { rect: RectF, wrap_w: f32, lines: usize }
pub(crate) fn rename_edit_rect(rt, target, text) -> Option<RenameBox>;   // 几何唯一真源
pub(crate) fn refresh_rename_rect(rt);                                   // 逐帧回写 rect + wrap_w
fn rename_box_geometry(base, dir, text, metrics, fence_h/vspan, view) -> RenameBox;  // 纯几何

// render
pub struct SceneEdit { …, pub wrap_w: Option<f32>, … }             // 与 InlineEdit 同值
pub const EDIT_LINE_H_MULT: f32 = 1.5;                             // 行距的唯一真源
OverlayEvent::EditCaret { x: f32, y: f32 }                         // 折行后必须带 y 判行
```

**宽度**：`need = 上界宽 × TEXT_WIDTH_SLACK + 2·pad_x`；
`w = clamp(max(need, 锚点宽), 1, 卡片内宽 ∩ 屏幕可用宽)`；锚点宽下限保证短名字的框仍是一个整格。
**上界宽**：`winbosk_core::text::upper_width`（逐字形上界：`W/@` 1.05、`M` 1.0、`m` .95、`%` .9、`&` .88、
`O/Q/N/G/D/H/U/B/R/P/C/K/V/X/Y/Z/w` .83、emoji/符号 1.5、其余 1.0/0.62）。
容器尺寸**禁止**用平均口径 `estimate_width`（0.62 em）——全大写或 `W/M/@` 类名字实测平均字宽可达
0.9 em 以上，用平均口径算出来的框会让 DirectWrite 画出的字形溢出被静默裁掉（审查 F1）。
**折行预算**：`wrap_w = (w − 2·pad_x) ÷ WRAP_WIDTH_SLACK(1.05)`（下限半个字宽）；折行逐字宽度
本身已取上界，故只留 5% 余量，不再叠乘 `TEXT_WIDTH_SLACK`（两档叠乘会让折行过早、框无谓变高）。
**行距/框高**：`line_h = font × EDIT_LINE_H_MULT`；`lines ≥ 2` 时 `h = max(锚点高, lines·line_h + 2·pad_y)`，
向下生长；`lines == 1` 时沿用锚点高度（短名字的框不会变高）。
**定位方向**（`RenameGrow`）：网格/横向 dock 以锚点中心、其它保持左缘、右停靠侧边栏保持右缘。
**钳制顺序**：宽度先受卡片上限 → 屏幕横向钳制 → 收进卡片横向内缘（只平移，不压缩）→
纵向优先留在卡片内（够高时整框上移贴住下缘）→ 屏幕纵向钳制。

---

## 4. 隐性机制与系统动力学推演

- **逐帧幂等**：`refresh_rename_rect` 只从「模型 + 文本 + 主题 + 栅栏几何」算结果，从不读旧 `rect`；
  `(rect, wrap_w)` 无变化即早退 → 不存在「每帧自己长一点」的正反馈。
- **同一帧多个消费者**：绘制（`present`）、窗口区域（`build_region`）、合成表面（`content_rect`）、
  命中/点击热区（`HitModel::edit_rect`）、IME 定位在同一帧看到同一个 `rect + wrap_w`。
  表面按 `SURFACE_GROW = 256` 一次撑开，输入过程中只在跨越余量时重建，不会每敲一字重建一次。
- **折行口径单源**：`winbosk_core::text::wrap_visual_lines` 同时被 App（光标/点击/IME/上下键）与
  Render（逐视觉行绘制）调用，字号同取 `label.size`。任何一处改用「自己数宽度」都会让
  光标与所见错位——这是本特性最脆的耦合点。
- **IME 合成串参与折行**：按 `display_text()`（光标处插入 `comp`）折行，光标下标 = `col + comp 长度`，
  合成期间文本变长会自动把光标挤到下一视觉行，`ImeCompose` 每帧重算，不残留。
- **键盘**：Enter 仍提交（`single_line = true` 不受折行影响）、Esc 取消、粘贴换行转空格；
  上下键在折行框内按**视觉行**移动（保持行内偏移；合成期间不介入，方向键归 IME）。
- **空闲性能**：无新增定时器/周期唤醒；编辑结束 `rt.edit = None` 后 `refresh_rename_rect` 立即空转。
- **物理变更回流**：编辑期间 4s 库同步可能移除被编辑项 → 几何函数一律 `get()` 系列返回 `None`，
  保持原矩形（不 panic）；提交路径仍由既有 `apply_rename` 按语义下标校验收口。
- **纵向钳制读真实高度**：`bounds.h <= 0` 的自动高度栅栏必须读旁路表 `last_layout_h`
  （AGENTS.md 第 8 条），直接读 `bounds.h` 会把卡片判成 0 高、纵向约束失效。

---

## 5. 分层改动清单

1. [`crates/core/src/text.rs`](file:///g:/Codes/sylva/crates/core/src/text.rs)：`VisualLine` / `wrap_visual_lines`
   （贪心、绝不切字节、单字超预算独占一行、1% 字号容差抵消浮点累计误差、逐字宽度取**上界**）/
   `visual_line_of`（行尾归本行）/ `char_upper_unit` + `upper_width`（容器上界口径，F1）+ 单测。
2. [`crates/render/src/theme.rs`](file:///g:/Codes/sylva/crates/render/src/theme.rs)、[`lib.rs`](file:///g:/Codes/sylva/crates/render/src/lib.rs)：
   新增 `EDIT_LINE_H_MULT`（行距唯一真源，App 与 Render 共用）。
3. [`crates/render/src/compositor.rs`](file:///g:/Codes/sylva/crates/render/src/compositor.rs)：新增 `set_theme`
   （重建 `TextFormats`；F2 —— 否则 DPI 变化后 App/Render 字号分叉）。
3. [`crates/render/src/scene.rs`](file:///g:/Codes/sylva/crates/render/src/scene.rs)：`SceneEdit::wrap_w`；
   `Scene::content_rect()` 并入 `scene.edit.rect`（合成表面裁剪口）。
4. [`crates/render/src/draw.rs`](file:///g:/Codes/sylva/crates/render/src/draw.rs)：`draw_inline_edit` 逐视觉行排布
   （单视觉行仍整框垂直居中，保持短名字的既有观感）；光标画在**它所在的那一视觉行**；
   `edit_visual_lines` 与 App 同口径；DC 渲染目标测试补「折行帧」。
5. [`crates/render/src/overlay.rs`](file:///g:/Codes/sylva/crates/render/src/overlay.rs)：`EditCaret { x, y }`
   （折行后必须带 y 判行）+ 发射点。
6. [`crates/app/src/editing.rs`](file:///g:/Codes/sylva/crates/app/src/editing.rs)：`InlineEdit::wrap_w`、
   `display_text` / `caret_index` / `visual_lines` / `move_visual_line`；`RenameBox` 几何
   （定宽 + 折行 + 框高 + 双向钳制）；`edit_caret_point` / `edit_click(x, y)` 按视觉行定位；
   `start_inplace_rename` / `refresh_rename_rect` 共用 `rename_edit_rect`。
7. [`crates/app/src/scene.rs`](file:///g:/Codes/sylva/crates/app/src/scene.rs)：`build_scene` 在快照前
   `refresh_rename_rect`；`SceneEdit` 带上 `wrap_w`；`TEXT_WIDTH_SLACK` 常量（**上界**口径的安全余量，
   与工具提示同源；`sidebar_tooltip_rect` 与 `label_width` 一并收敛到 core 的两个口径）。
8. [`crates/app/src/main.rs`](file:///g:/Codes/sylva/crates/app/src/main.rs)：`EditCaret { x, y }` 分支；
   `DpiChanged` 分支调用 `compositor.set_theme`（F2）。

---

## 6. 防御性自查（对照六律）

| 律 | 本次对应设计 |
| :--- | :--- |
| 常驻后台冲突 | 不新增后台任务；4s 同步删项时几何安全退化（`None` 保持原矩形） |
| 空闲性能归零 | 无新定时器；`(rect, wrap_w)` 无变化即空转；编辑结束不再重绘 |
| 孤儿状态回收 | 编辑结束 `rt.edit = None` → 无残留矩形（矩形不进任何旁路表） |
| 语义精准定位 | 目标用 `EditTarget::{Item,FenceTitle}`（栅栏/图标下标）；越界一律 `None` 而非 panic |
| 状态全集校验 | 只有 `Item`/`FenceTitle` 参与定宽折行，规则输入框显式排除（返回 `None`） |
| 旁路数据对齐 | 绘制 / 窗口区域 / 合成表面 / 点击热区 / IME / 上下键六处同帧共用 `rect + wrap_w`；纵向钳制读 `last_layout_h` |

---

## 7. 验证与门禁

- `cargo build --workspace` / `cargo test --workspace`（**292** 项）/ `cargo clippy --workspace -- -D warnings`
  / `cargo fmt --all -- --check` 全绿。
- 新增测试（**26** 项）覆盖：折行覆盖全文/不丢字/不超预算、**上界口径 ≥ 平均口径且宽字形必须折行**、
  退化输入（空文本、预算 0、单字超宽）、行尾归本行、定宽锚点与幂等、卡片上限、
  **宽字形名字（全大写 / `W M @ m w`）在 cap∈{800,300,200,120} 下全部不裁字**、
  **窄卡片长名字折行后整框在卡片内且每行都不超可用宽**、一行装得下不折行、
  纵向钳制（够高上移 / 太矮仍留屏幕内）、侧边栏只受屏幕约束、
  **上限必须先与屏幕取交（钳制顺序回归点）**、折行预算保守性、
  `display_text` 合成串、`visual_lines` + `caret_index`、**折行框内点击落点行**（含合成串区间折算）、
  上下键视觉行移动、`click_column` 是宽度累计的逆、`Scene::content_rect` 并入编辑框、
  **DC 渲染目标真跑折行帧**。
- 手工验证（需桌面会话）：窄栅栏里重命名 20+ 字的中文名 → 框**留在卡片内**并折成 2~3 行、文字完整；
  点第二行的某个字 → 光标落在该字；上下键在行间移动；Enter 提交、Esc 取消；列表/标题/侧边栏同样成立。
- **自审发现并已修的两处**（写在这里，防止回归）：
  1. 宽度上限必须先与屏幕可用宽取交，否则折行按旧宽算完、横向钳制又把框改窄 → 最后一行仍会溢出（已补
     `cap_never_exceeds_screen_so_wrap_matches_final_width`）；
  2. 逐视觉行排布的行序号必须**跨逻辑行累加**（原写法用行内下标，多逻辑行便签会把所有行叠在第 0 行；
     App 侧对应新增 `caret_row`）——当前 `single_line` 恒为 true 时不可达，但属于必须修掉的潜伏缺陷。

---

## 8. 对抗性审查记录（第一轮：宽度无界生长版，已被本轮取代）

第一轮审查确认：两道裁剪口同帧闭环、`covering_decision` 以当前 bbox 为基点是稳定不动点、
按下判定顺序与双击守卫、点框外提交、越界 `None`、规则输入框互斥、无新增空闲唤醒。

其**主缺陷主张经复核不成立**并记录于此，避免日后重复误判：报告称「居中生长后 `edit_click` 会点到错误
字符（偏差≈2/3 名字长）」，依据是「基准矩形才是文本容器」。实际上绘制 `lr.left`（`draw.rs:3394`）、
光标 `caret_x`（`:3422`）、`edit_click`、`edit_caret_point` 四处读的都是 `rect.x + input_pad_x`，
而 `rect` 只有一个来源（`rt.edit.rect`），框生长时四者同变，不存在两套文本原点。

**成立并已采纳**的两条：①新增单测没钉住「文本容器 = `rect.x + pad_x`」→ 抽出可测内核
`click_column` 并补契约测试；②「不截断」的目标句与屏幕钳制口径冲突 → 目标/边界显式写明。

第一轮报告中的「文本会随框生长而移动」属实，但被用户实测的**更严重**问题（框越出卡片）覆盖：
本轮改为「宽度有界 + 折行」，文本位置只在折行重排时变化（每次重排都伴随行数变化，用户可预期）。

---

## 9. 第二轮审查记录（定宽 + 折行版）

第二轮审查结论：**无阻断缺陷**；同帧闭环、折行宽度 == 最终宽度、折行口径两侧同源、无 panic 面、
表面不振荡、命中/穿透正确、编辑框不被合成表面裁——逐条核对通过。发现 2 条真实缺陷与 7 条非阻断项，
处置如下（全部落库，不靠"下次再说"）：

| 编号 | 审查结论 | 处置 |
| :--- | :--- | :--- |
| **F1** | **真缺陷**：宽字形名（全大写 / `W M @ m w`）在折行后仍被裁，且卡片够宽时**根本不折行**——折行只在"平均口径认为装不下"时发生，救不了"平均口径低估"的文本 | **已修**：新增上界口径 `char_upper_unit`/`upper_width`，容器宽度与**折行逐字宽度**都改用它；折行预算改用 `WRAP_WIDTH_SLACK = 1.05`（上界口径下不再叠乘 1.2）。补 `wide_glyph_names_fit_without_clipping` 覆盖审查报告里的实测反例（`W×20`、`W×36.txt`、`@×30`、`M×10.doc`、`m×18.txt`、全大写工程名 × cap∈{800,300,200,120}） |
| **F2** | **真缺陷**（根因早于本轮）：DPI 变化后 App 的 `rt.theme` 更新、`Compositor` 持有的 theme/`TextFormats` 未更新 → 两侧字号分叉，框宽/折行/光标全错 | **已修**：`Compositor::set_theme`（重建 `TextFormats` + 替换 theme），`DpiChanged` 分支同帧调用；失败只告警并沿用旧字号（有日志） |
| F3 | 宽度口径有 3 份拷贝，是 F1 的温床 | **已收敛**：单位表只留 `core::text`（`estimate_width` 平均 / `upper_width` 上界）；`app::scene::label_width` 改为委托 `estimate_width`；工具提示也改用 `upper_width` |
| F4 | `single_line = false` 无构造点（多行便签分支当前不可达） | 记录（非阻断）：折行的"多视觉行"路径**在用**（`wrap_w = Some` + `lines > 1`）；仅"多逻辑行"无构造点，`edit_click` 对它的早退是既有行为 |
| F5 | 合成串跨视觉行时点击落点可能差一行 | 记录为残留边界（§2 第 3 条），非阻断 |
| F6 | 行内光标/点击用平均口径、绘制用 DWrite 实测 | 记录为残留边界（§2 第 2 条）；折行已把误差限制在单个视觉行内 |
| F7 | 卡片比折行框矮时纵向溢出卡片 | 记录为残留边界（§2 第 1 条），非阻断（横向在数学上已关闭） |
| F8 | 合成表面重建失败会沿用旧表面 | 记录为残留边界（§2 第 4 条），既有降级行为 |
| F9 | `RENAME_SCREEN_MARGIN` 未随 `theme.scale` 缩放 | 非阻断：与既有 `item_label_rect` 侧边栏分支硬编码 4.0 同一口径，保持一致 |

审查报告里**已被并发修复覆盖**的两条（避免重复劳动）：draw 多逻辑行叠在第 0 行、`edit_click` 未抽可测内核
——前者由本轮 `row` 累加 + `caret_row` 修复，后者由 `click_column_wrapped` 修复并补测。

> 审查方法说明：该代理只读审查，未跑构建（工作区正被并发修改）；F1 的数值来自其用 GDI+
> `GenericTypographic` 对 Microsoft YaHei UI advance 的实测与对 `wrap_visual_lines`/`rename_box_geometry`
> 的手写复刻。本轮把这些反例直接写成单测（`wide_glyph_names_fit_without_clipping`），
> 但**上界表仍是估计**：字体族换成比表中值更宽的字形时仍可能裁字——若要绝对保证，
> 需改为「拿 DirectWrite 实测宽 + 缓存」（当前不做，见 §2 残留边界）。
