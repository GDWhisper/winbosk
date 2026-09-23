//! 场景模型：一次绘制所需的全部数据（应用无关）。
//!
//! 坐标全部为**物理像素**（虚拟屏幕坐标），与 overlay 窗口客户端坐标一致。

use winbosk_core::hotkey::HotkeyAction;
use winbosk_core::model::{CategoryPreset, FenceLayout, FenceStyle, SidebarPosition};
use winbosk_core::storage::StorageKind;

use crate::overlay::{ConsoleZone, RectF};

/// 栅栏内的一个图标（位置由 App 层按主题网格/列表排布后填入）。
#[derive(Debug, Clone)]
pub struct SceneIcon {
    /// 图标文字（网格=下方，列表=名称列）。
    pub label: String,
    /// 对应 `IconStore` 中的位图 ID（`IconStore::insert` 返回）。
    pub bitmap_id: u64,
    /// 图标左上角（物理像素，虚拟屏幕坐标）。
    pub x: f32,
    pub y: f32,
    pub size: f32,
    /// 列表详情列文本：类型 / 修改日期 / 大小（网格模式下为空串）。
    pub col_type: String,
    pub col_modified: String,
    pub col_size: String,
    /// 悬停缩放 1.0=常态，>1 = 放大中（App 层按 hover 补间填值，绘制时以中心放大）。
    pub scale: f32,
}

/// 列表布局的详情列位置（绝对虚拟屏幕 x；名称列紧贴图标右侧，不入列）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListColumns {
    pub type_x: f32,
    pub modified_x: f32,
    pub size_x: f32,
    /// 列表列头高度（绘制列头与滚动裁剪共用）。
    pub header_h: f32,
}

/// 一个栅栏。
#[derive(Debug, Clone)]
pub struct SceneFence {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub title: String,
    pub icons: Vec<SceneIcon>,
    /// 布局格式：网格 / 列表。
    pub layout: FenceLayout,
    /// 列表布局的详情列位置；网格布局为 None。
    pub list_cols: Option<ListColumns>,
    /// 网格布局的格宽（物理像素）；列表/侧边栏为 0。网格标签两行排布与
    /// 悬停工具提示是否截断的判断共用（标签横跨整格绘制）。
    pub grid_cell_w: f32,
    /// 当前内容滚动偏移（物理像素）。
    pub scroll: f32,
    /// 可滚动范围（0 = 内容不超出可视区，无滚动条）。
    pub scroll_max: f32,
    /// 内容可视区高度（滚动条比例用）。
    pub scroll_view: f32,
    /// 内容区顶边（绝对 y）：列表列头与网格第一行从这里开始，绘制裁剪用。
    pub content_top: f32,
    /// 内容区左边（绝对 x）：行/列头/滚动条以此为左基准（与栅栏内边距一致）。
    pub content_left: f32,
    /// 当前鼠标悬停的图标下标（布局中的顺序，与 `icons` 一致）；None = 无悬停。
    pub hover_icon: Option<usize>,
    /// 当前选中的图标下标（多选：框选 / Ctrl 单击，与资源管理器一致；顺序与 `icons` 一致）。
    pub selected: Vec<usize>,
    /// 框选橡皮筋矩形（物理像素）；None = 未在框选。绘制时裁剪在本栅栏内容区内。
    pub select_band: Option<RectF>,
    /// 边框描边宽度（物理像素）；0 = 不描边。
    pub border_width: f32,
    /// 边框颜色（直通 alpha）。
    pub border_color: [f32; 4],
    /// 背景填充颜色（直通 alpha）；None = 内部完全透明（仅描边 / 模糊）。
    pub fill_color: Option<[f32; 4]>,
    /// 模糊背景（FenceStyle::Blur）：背景由合成器里独立的 GaussianBlurEffect 视觉
    /// 绘制（GPU，实时），本栅栏内容区保持透明以透出；合成器据此建/删模糊视觉。
    pub blur: bool,
    /// 整体透明度 0..1（桌面切换淡出/淡入用；绘制时乘到所有颜色上）。
    pub alpha: f32,
    /// 侧边栏悬停工具提示矩形（物理像素，由 App 层按屏幕边界计算好，
    /// 可延伸到栅栏之外）；None = 不显示。非侧边栏布局恒为 None。
    pub tooltip_rect: Option<RectF>,
    /// 侧边栏图标拖动排序状态：Some 时绘制拖动中的图标位置。
    pub reorder_drag: Option<ReorderDrag>,
    /// 栅栏是否收起（折叠仅留标题栏）。
    pub collapsed: bool,
    /// 标题栏折叠/展开切换按钮矩形（物理像素）。
    pub collapse_btn: Option<RectF>,
}

/// 侧边栏图标拖动排序的渲染状态。
#[derive(Debug, Clone, Copy)]
pub struct ReorderDrag {
    /// 被拖动的图标在 `icon_ids` 中的下标。
    pub icon_idx: usize,
    /// 当前光标位置（虚拟屏幕物理像素，图标中心跟随此位置）。
    pub cursor_x: f32,
    pub cursor_y: f32,
}

impl SceneFence {
    /// 点是否落在栅栏矩形内（命中测试用）。
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.width && py >= self.y && py <= self.y + self.height
    }
}

/// 收起栅栏的「原大小占位框」（瞬态：仅拖动期间存在，不持久化）。
///
/// 收起只改变视觉高度，碰撞/夹屏仍按展开时的原矩形计算。拖动期间把该矩形画成
/// 虚线淡框，用户才能理解"为什么在这个位置就推不动了"。
///
/// 几何由 App 层按 `fence_collision_rect` 同源给出（禁止渲染层另算）；
/// 颜色直通 alpha，已含占位透明度与桌面切换淡出。
#[derive(Debug, Clone)]
pub struct SceneReserved {
    /// 占位矩形（物理像素，虚拟屏幕坐标）= 该栅栏的碰撞矩形。
    pub rect: RectF,
    /// 来源栅栏下标（`desk.fences`），仅用于日志与断言。
    pub fence: usize,
    /// 虚线描边色（直通 alpha）。
    pub stroke_color: [f32; 4],
    /// 极淡填充色（直通 alpha）；None = 只描边不填充。
    pub fill_color: Option<[f32; 4]>,
}

/// 待办插件的一条（渲染用）。二级结构：名称 + 详细信息。
#[derive(Debug, Clone)]
pub struct SceneTodoRow {
    /// 事项名称（一级）。
    pub name: String,
    /// 详细信息（二级）；空串 = 无副标题。
    pub detail: String,
    pub done: bool,
    /// 行不透明度 0..1（入场淡入 / 删除幽灵淡出）。
    pub alpha: f32,
    /// 完成状态交叉淡化 0..1（0=旧状态，1=新状态）。
    pub done_progress: f32,
}

/// 待办插件渲染数据：条目 + 滚动状态 + 内容区几何。
#[derive(Debug, Clone, Default)]
pub struct SceneTodo {
    pub rows: Vec<SceneTodoRow>,
    /// 列表滚动偏移（物理像素，向上卷出顶部）。
    pub scroll: f32,
    /// 可滚动范围（0 = 不超出可视区）。
    pub scroll_max: f32,
    /// 首行内容的绝对 y（输入行之下）。
    pub rows_top: f32,
    /// 单行高（物理像素）。
    pub row_h: f32,
}

/// 栅栏管理页：可点选的一行（选中后在下方详情区显示控制项）。
#[derive(Debug, Clone)]
pub struct SceneFenceRow {
    pub rect: RectF,
    pub title: String,
    pub selected: bool,
}

/// 栅栏管理页：选中栅栏的详情控制区（分段按钮 + 色调色板）。
#[derive(Debug, Clone)]
pub struct SceneFenceDetail {
    pub rect: RectF,
    pub title: String,
    /// 当前值（绘制时高亮对应分段按钮 / 色板项）。
    pub layout: FenceLayout,
    pub icon_size: f32,
    pub style: FenceStyle,
    pub tint: Option<[f32; 3]>,
    /// 布局：网格 / 列表 / 侧边栏
    pub layout_grid: RectF,
    pub layout_list: RectF,
    pub layout_sidebar: RectF,
    /// 图标大小：小 / 中 / 大
    pub size_s: RectF,
    pub size_m: RectF,
    pub size_l: RectF,
    /// 背景风格：玻璃 / 描边 / 颜色 / 模糊
    pub style_glass: RectF,
    pub style_outline: RectF,
    pub style_filled: RectF,
    pub style_blur: RectF,
    /// 色调：默认（恢复玻璃底色）+ 预设色板（与 App 层 TINT_PRESETS 平行）。
    pub tint_default: RectF,
    pub tints: Vec<RectF>,
    /// 「更改文件位置…」按钮（动作行，右端对齐）。
    pub storage_btn: RectF,
    /// 值行整行矩形（仅作标签列锚点与行带文档，**不参与命中**）。
    /// 「文件位置」标签按它定位，保证"标签与它命名的值在同一行"。
    pub storage_value_row: RectF,
    /// 当前落地模式（状态标签文案与配色）。
    pub storage_kind: StorageKind,
    /// 中段省略后的真实落地路径（App 层按当前 DPI 预算预计算）。
    ///
    /// 绘制层**仍会**二次截断（`draw_text` 的 `truncate_to_fit`）——App 层刻意预留了 2·s 余量
    /// 所以平时不会触发，但那只是兜底，别把它当保证：一旦预算改紧，超出的字会被静默换成「…」。
    /// 因此 `storage_path_hit` 的宽度**不能**取自这里字串的估算宽（见 `TextFormats::measure_console_detail`）。
    pub storage_path_text: String,
    /// 路径文本区（值行；**仅绘制**——决定省略预算与裁剪框，不参与命中）。
    pub storage_path_rect: RectF,
    /// 落地模式状态标签（值行左端；**仅绘制**，不参与命中——它是状态不是按钮）。
    pub storage_chip: RectF,
    /// **路径文本的命中区**（值行；贴着实际字形，不是整段预算）。点它 = 在资源管理器里打开该目录。
    ///
    /// 这一行**刻意没有「打开」按钮**：一个按钮要吃掉 40·s，占路径预算的 1/3（实测会让默认宽下的
    /// 库路径从"完整显示"退化成 23/40 字）。由路径自己承担这个动作，值行因此是零按钮的纯信息行。
    /// `w <= 0.0`（路径被压没）表示不绘制、不参与命中。
    pub storage_path_hit: RectF,
    /// 动作行左侧的后果提示（如「所有栅栏共用 · 删除会真删文件」）。
    /// 空串 = 当前宽度放不下，整条不绘制（不与动作按钮重叠）。
    pub storage_hint_text: String,
    /// 后果提示文本区（动作行左端；**仅绘制**）。`h <= 0.0` 表示不绘制。
    pub storage_hint_rect: RectF,
    /// 值行内 detail 字号文字（行标签 / 状态标签 / 路径）的**共用顶线**（物理像素）。
    ///
    /// 这三段同字号，各自按自己的偏移绘制就会落在三条不同的基线上、整行看起来是歪的，
    /// 故由 App 层几何函数算出唯一顶线，绘制层一律用它（动作行的提示语同理，其 y 也来自
    /// 同一条口径）。
    pub storage_text_top: f32,
    /// 「恢复默认」按钮（解除外部链接，回到应用内部库）。
    /// `h <= 0.0` 表示当前不可回退（不绘制、不参与命中）。
    pub storage_reset: RectF,
    /// 当前侧边栏停靠位置（仅 Sidebar 布局显示）。
    pub sidebar_pos: SidebarPosition,
    /// 侧边栏位置按钮：左 / 上 / 右
    pub sidebar_left: RectF,
    pub sidebar_top: RectF,
    pub sidebar_right: RectF,
    /// 当前分类规则预设（绘制高亮用）
    pub current_preset: Option<CategoryPreset>,
    /// 规则按钮：无 / 应用 / 文档 / 媒体 / 压缩 / 目录
    pub rule_none: RectF,
    pub rule_apps: RectF,
    pub rule_docs: RectF,
    pub rule_media: RectF,
    pub rule_archives: RectF,
    pub rule_folders: RectF,
}

/// 控制台面板（控制中心：栅栏管理）。
///
/// 几何字段同时供绘制（draw_console）与命中模型（hit_model_from）使用，
/// 保证点击区域与视觉一致。
#[derive(Debug, Clone)]
pub struct SceneConsole {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// 标题栏高（拖动把手；关闭按钮与桌面切换按钮位于其内）。
    pub title_h: f32,
    /// 关闭按钮矩形（标题栏右上）。
    pub close: RectF,
    /// 「恢复桌面」按钮矩形（底部，左半）。
    pub desktop_toggle: RectF,
    /// 「开机自启」按钮矩形（底部，右半）。
    pub autostart_toggle: RectF,
    /// 栅栏管理页：可点选栅栏行（与 `desk.fences` 平行）。
    pub fence_rows: Vec<SceneFenceRow>,
    /// 栅栏管理页：列表可视区（行超出部分被裁剪，命中模型据此跳过不可见行）。
    pub fence_list_view: RectF,
    /// 栅栏管理页：选中栅栏的详情控制区。
    pub fence_detail: Option<SceneFenceDetail>,
    /// 栅栏管理页：「添加栅栏」按钮。
    pub add_fence: RectF,
    /// 栅栏管理页：「一键整理」按钮。
    pub organize_btn: RectF,
    /// 栅栏管理页：「删除栅栏」按钮（添加按钮下方）。
    pub remove_btn: RectF,
    pub fill_color: [f32; 4],
    pub border_color: [f32; 4],
    /// 面板展开进度（几何用）：0 = 完全不渲染，1 = 完整面板；展开回弹期间可 >1。
    pub panel: f32,
    /// 面板整体不透明度 0..1：由 `panel` 派生（见 App 层 `CONSOLE_FADE_SPAN`），
    /// 与高度解耦——淡入只在开场一小段完成，其后只有高度在动。
    pub fade: f32,
    /// 当前悬停的控制台控件（App 层经 ConsoleHover 事件写入；绘制高亮用）。
    pub hover_zone: Option<ConsoleZone>,
    /// 是否处于原始桌面模式（标题栏按钮文案与状态）。
    pub desktop_mode: bool,
    /// 是否开启开机自启（按钮文案与状态）。
    pub autostart: bool,
    /// 是否处于高级模式（双栏展开）。
    pub advanced: bool,
    /// 模式切换按钮矩形（标题栏右侧，关闭按钮左边）。
    pub mode_toggle: RectF,
    /// 栅栏高级规则编辑器（高级模式右栏；无选中栅栏时为 None）。
    pub rule_editor: Option<SceneRuleEditor>,
    /// 标题栏设置按钮（⚙，模式切换按钮左侧）。
    pub settings_toggle: RectF,
    /// 当前是否处于设置页面。
    pub is_settings_page: bool,
    /// 设置页面内容几何。
    pub settings_page: Option<SceneSettingsPage>,
}

/// 全局设置页（快捷键配置等）。
#[derive(Debug, Clone)]
pub struct SceneSettingsPage {
    pub rect: RectF,
    pub rows: Vec<SceneHotkeyRow>,
    pub reset_default_btn: RectF,
    pub back_btn: RectF,
}

/// 设置页中的单个快捷键行。
#[derive(Debug, Clone)]
pub struct SceneHotkeyRow {
    pub action: HotkeyAction,
    pub label: &'static str,
    pub desc: &'static str,
    pub rect: RectF,
    pub key_btn: RectF,
    pub clear_btn: Option<RectF>,
    pub key_text: String,
    pub is_recording: bool,
    pub conflict_msg: Option<String>,
}

/// 高级模式下的栅栏规则编辑器（右栏工作台）。
#[derive(Debug, Clone)]
pub struct SceneRuleEditor {
    pub rect: RectF,
    pub fence_title: String,
    pub rule_enabled: bool,
    pub toggle_btn: RectF,
    /// 包含预设类别按钮 (preset, rect, is_selected)
    pub preset_chips: Vec<(Option<CategoryPreset>, RectF, bool)>,
    /// 包含后缀芯片列表 (ext, chip_rect, del_btn_rect)
    pub ext_chips: Vec<(String, RectF, RectF)>,
    pub add_ext_btn: RectF,
    /// 排除后缀黑名单列表 (ext, chip_rect, del_btn_rect)
    pub exclude_chips: Vec<(String, RectF, RectF)>,
    pub add_exclude_btn: RectF,
    /// 通配符/关键字模式列表 (pat, chip_rect, del_btn_rect)
    pub pattern_chips: Vec<(String, RectF, RectF)>,
    pub add_pattern_btn: RectF,
    /// 自动捕获开关
    pub auto_capture_toggle: RectF,
    pub auto_capture_val: bool,
    /// 单栅栏即时应用规则整理按钮
    pub apply_btn: RectF,
    /// 提示引导文本与说明矩形
    pub tip_rect: RectF,
    /// 当前激活的规则输入框矩形（若处于添加后缀/排除/通配符编辑态）
    pub active_edit_rect: Option<RectF>,
}

/// 内联文本编辑的渲染数据（App 层 InlineEdit 的只读快照，绘制用）。
#[derive(Debug, Clone)]
pub struct SceneEdit {
    pub rect: RectF,
    pub lines: Vec<String>,
    pub line: usize,
    pub col: usize,
    pub placeholder: String,
    pub single_line: bool,
    pub focused: bool,
    pub composing: bool,
    pub comp: String,
}

/// 整屏场景（虚拟屏幕）。
#[derive(Debug, Default)]
pub struct Scene {
    /// 虚拟屏幕尺寸（物理像素）。
    pub width: f32,
    pub height: f32,
    pub fences: Vec<SceneFence>,
    /// 当前激活的内联文本编辑（None = 无）。
    pub edit: Option<SceneEdit>,
    /// 控制台面板（插件宿主）；None = 本帧不画。
    pub console: Option<SceneConsole>,
    /// 收起栅栏的原大小占位框（仅拖动期间非空）；绘制在全部栅栏**之下**。
    pub reserved: Vec<SceneReserved>,
}

impl Scene {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            fences: Vec::new(),
            edit: None,
            console: None,
            reserved: Vec::new(),
        }
    }

    /// 全部内容的包围盒（虚拟屏幕坐标：全部栅栏 ∪ 控制台 ∪ 占位框）。
    ///
    /// 合成器据此把合成表面缩到内容大小而非整屏（省内存）。没有内容时返回 None。
    /// **占位框必须并入**：它比收起后的标题栏大得多，漏并会被合成表面裁掉（表现为
    /// 「框只有部分方向可见」，与窗口区域漏并的症状一模一样，极易误判）。
    pub fn content_rect(&self) -> Option<RectF> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut any = false;
        for f in &self.fences {
            min_x = min_x.min(f.x);
            min_y = min_y.min(f.y);
            max_x = max_x.max(f.x + f.width);
            max_y = max_y.max(f.y + f.height);
            any = true;
            // 侧边栏工具提示可延伸到栅栏之外，必须并入表面，否则区域外绘制不可见。
            if let Some(tt) = f.tooltip_rect {
                min_x = min_x.min(tt.x);
                min_y = min_y.min(tt.y);
                max_x = max_x.max(tt.x + tt.w);
                max_y = max_y.max(tt.y + tt.h);
            }
        }
        if let Some(c) = &self.console {
            min_x = min_x.min(c.x);
            min_y = min_y.min(c.y);
            max_x = max_x.max(c.x + c.width);
            max_y = max_y.max(c.y + c.height);
            any = true;
        }
        for r in &self.reserved {
            min_x = min_x.min(r.rect.x);
            min_y = min_y.min(r.rect.y);
            max_x = max_x.max(r.rect.x + r.rect.w);
            max_y = max_y.max(r.rect.y + r.rect.h);
            any = true;
        }
        if !any {
            return None;
        }
        Some(RectF {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_fence() -> SceneFence {
        SceneFence {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 80.0,
            title: "测试".into(),
            icons: vec![],
            layout: FenceLayout::Grid,
            list_cols: None,
            grid_cell_w: 72.0,
            scroll: 0.0,
            scroll_max: 0.0,
            scroll_view: 0.0,
            content_top: 0.0,
            content_left: 0.0,
            hover_icon: None,
            selected: vec![],
            select_band: None,
            border_width: 1.0,
            border_color: [1.0, 1.0, 1.0, 0.1],
            fill_color: Some([0.08, 0.08, 0.12, 0.55]),
            blur: false,
            alpha: 1.0,
            tooltip_rect: None,
            reorder_drag: None,
            collapsed: false,
            collapse_btn: None,
        }
    }

    #[test]
    fn fence_contains_checks_rectangle() {
        let f = test_fence();
        assert!(f.contains(10.0, 20.0));
        assert!(f.contains(110.0, 100.0)); // 右下角（含边界）
        assert!(!f.contains(9.0, 20.0));
        assert!(!f.contains(10.0, 101.0));
    }

    #[test]
    fn fence_style_fields_are_plumbed() {
        let f = test_fence();
        assert_eq!(f.border_width, 1.0);
        assert!(f.fill_color.is_some());
        // 描边模式：无填充
        let outline = SceneFence {
            fill_color: None,
            border_width: 2.5,
            ..test_fence()
        };
        assert!(outline.fill_color.is_none());
        assert_eq!(outline.border_width, 2.5);
    }

    #[test]
    fn fence_scroll_and_columns_fields_exist() {
        let f = test_fence();
        assert_eq!(f.layout, FenceLayout::Grid);
        assert_eq!(f.scroll, 0.0);
        assert_eq!(f.scroll_max, 0.0);
        assert!(f.list_cols.is_none());
        assert!(f.hover_icon.is_none());
    }

    #[test]
    fn list_columns_store_absolute_x() {
        let c = ListColumns {
            type_x: 200.0,
            modified_x: 300.0,
            size_x: 450.0,
            header_h: 26.0,
        };
        assert_eq!(c.type_x, 200.0);
        assert!(c.size_x > c.modified_x);
        assert_eq!(c.header_h, 26.0);
    }

    #[test]
    fn scene_defaults_empty() {
        let s = Scene::new(1920.0, 1080.0);
        assert!(s.fences.is_empty());
        assert!(s.reserved.is_empty());
        assert_eq!(s.width, 1920.0);
        assert!(s.content_rect().is_none());
    }

    #[test]
    fn content_rect_covers_fences() {
        let mut s = Scene::new(3072.0, 1920.0);
        let f1 = test_fence(); // (10,20,100,80)
        let f2 = SceneFence {
            x: 400.0,
            y: 300.0,
            width: 250.0,
            height: 120.0,
            ..test_fence()
        };
        s.fences = vec![f1, f2];
        let r = s.content_rect().expect("有内容应返回包围盒");
        assert_eq!(r.x, 10.0);
        assert_eq!(r.y, 20.0);
        assert_eq!(r.w, 640.0); // (400+250) - 10
        assert_eq!(r.h, 400.0); // (300+120) - 20
    }

    #[test]
    fn content_rect_covers_reserved_frames() {
        // 占位框通常落在栅栏可见体之外（收起后只剩标题栏）→ 必须并入包围盒，
        // 否则合成表面会把它裁掉。
        let mut s = Scene::new(3072.0, 1920.0);
        s.fences = vec![test_fence()]; // (10,20,100,80)
        s.reserved = vec![SceneReserved {
            rect: RectF {
                x: 10.0,
                y: 20.0,
                w: 100.0,
                h: 300.0,
            },
            fence: 0,
            stroke_color: [1.0, 1.0, 1.0, 0.45],
            fill_color: None,
        }];
        let r = s.content_rect().expect("有内容应返回包围盒");
        assert_eq!(r.x, 10.0);
        assert_eq!(r.y, 20.0);
        assert_eq!(r.h, 300.0);
    }
}
