//! 领域模型：桌面、栅栏、图标等核心类型。
//!
//! 本模块只包含纯数据定义与默认值，不含任何平台相关逻辑，
//! 以便 `winbosk-core` 保持零 Win32 依赖、可完全单元测试。

use serde::{Deserialize, Serialize};

use std::collections::HashMap;

/// 图标的稳定标识符。
///
/// 由 Shell 层的项指纹（PIDL / 路径哈希）生成，跨重启稳定，
/// 用于持久化成员关系与排序。
pub type ItemId = String;

/// 二维点。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// 矩形（逻辑坐标，与 DPI 无关；渲染时由 Render 层按 DPI 换算像素）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// 右边界。
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// 下边界。
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// 点是否在矩形内（含边界）。
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    /// 向内收缩 `d`（负值向外扩张）。不会缩到负宽高。
    pub fn inset(&self, d: f32) -> Self {
        Self {
            x: self.x + d,
            y: self.y + d,
            w: (self.w - 2.0 * d).max(0.0),
            h: (self.h - 2.0 * d).max(0.0),
        }
    }
}

/// 图标的类别。仅用于渲染表现（如快捷方式角标）与右键菜单，
/// **不参与任何自动归类**——归属完全由用户拖拽决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    /// 应用程序。
    App,
    /// 文件夹。
    Folder,
    /// 文档。
    Doc,
    /// 驱动器/卷。
    Drive,
    /// 快捷方式。
    Link,
    /// 未知。
    Unknown,
}

/// 栅栏的折叠状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FenceState {
    /// 展开：显示栅栏区域与全部图标。
    #[default]
    Expanded,
    /// 折叠：仅显示标题栏。
    Folded,
}

/// 栅栏背景风格（决定背景填充与不透明度）。
///
/// v3 起替代「透明度滑块」：栅栏背景只分几种固定风格，不再用 0..1 滑块细调。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FenceStyle {
    /// 颜色：不透明纯色填充（色调来自「背景色调」；未选时用默认背景色）。
    Filled,
    /// 透明：内部完全透明，仅保留圆角描边。
    Outline,
    /// 玻璃：默认半透明玻璃感（可叠加「背景色调」着色，45% 混合）。
    #[default]
    Glass,
    /// 模糊：背景 = 栅栏底下桌面内容的高斯模糊（磨砂玻璃；CPU 侧降采样 + 高斯）。
    Blur,
}

impl FenceStyle {
    /// 菜单/设置显示名。
    pub fn label(&self) -> &'static str {
        match self {
            FenceStyle::Filled => "颜色",
            FenceStyle::Outline => "透明",
            FenceStyle::Glass => "玻璃",
            FenceStyle::Blur => "模糊",
        }
    }
}

/// 栅栏的布局格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FenceLayout {
    /// 网格：图标自左向右、自上而下按列排布，标签在图标下方。
    Grid,
    /// 列表：图标单列纵向排布，标签在图标右侧。
    List,
    /// 侧边栏：仿 Mac Dock，仅显示图标，悬停放大 + 名称预览。
    Sidebar,
}

impl FenceLayout {
    /// 控制台/菜单显示名。
    pub fn label(&self) -> &'static str {
        match self {
            FenceLayout::Grid => "网格",
            FenceLayout::List => "列表",
            FenceLayout::Sidebar => "侧边栏",
        }
    }

    /// 循环切换到下一个格式。
    pub fn next(self) -> Self {
        match self {
            FenceLayout::Grid => FenceLayout::List,
            FenceLayout::List => FenceLayout::Sidebar,
            FenceLayout::Sidebar => FenceLayout::Grid,
        }
    }
}

/// 侧边栏停靠位置。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidebarPosition {
    /// 屏幕左侧（纵向排列）。
    #[default]
    Left,
    /// 屏幕上侧（横向排列）。
    Top,
    /// 屏幕右侧（纵向排列）。
    Right,
}

impl SidebarPosition {
    pub fn label(&self) -> &'static str {
        match self {
            SidebarPosition::Left => "左侧",
            SidebarPosition::Top => "上侧",
            SidebarPosition::Right => "右侧",
        }
    }
}

/// 栅栏外观配置。
///
/// `#[serde(default)]`：旧版 `desk.json` 缺少新增字段时自动取默认值，
/// 保证配置向后兼容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FenceAppearance {
    /// 背景色 RGBA（0.0..=1.0）；描边模式下不使用。
    pub bg_color: [f32; 4],
    /// 圆角半径（逻辑 px）。
    pub corner_radius: f32,
    /// 是否启用亚克力毛玻璃（Win11 生效，Win10 降级半透明）。
    pub acrylic: bool,
    /// 标题栏高度（逻辑 px）。
    pub title_bar_height: f32,
    /// 栅栏内边距。
    pub padding: f32,
    /// 图标尺寸（逻辑 px）。
    pub icon_size: f32,
    /// 图标间距（逻辑 px）。
    pub gap: f32,
    /// 布局格式（网格 / 列表）。
    pub layout: FenceLayout,
    /// 背景风格（玻璃 / 透明 / 颜色）。v3 起替代透明度滑块，决定填充与不透明度。
    #[serde(default = "default_bg_style")]
    pub bg_style: FenceStyle,
    /// 边框描边宽度（逻辑 px；描边模式用中粗线）。
    pub border_width: f32,
    /// 栅栏背景不透明度（0.0=完全透明，1.0=不透明；滑块调节）。
    ///
    /// v3 起滑块移除，「风格」取代它；字段保留仅供旧配置反序列化兼容，渲染不再读取。
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// 背景色调（RGB 0.0..=1.0）：None = 默认底色；Some = 着色。
    /// 「玻璃」风格下 45% 向色调靠拢；「颜色」风格下作为纯色填充。
    #[serde(default)]
    pub tint: Option<[f32; 3]>,
    /// 侧边栏停靠位置（仅 Sidebar 布局有效）。
    #[serde(default)]
    pub sidebar_pos: SidebarPosition,
}

/// `bg_style` 未持久化时的默认值：玻璃（保持旧版滑块默认的半透明观感）。
fn default_bg_style() -> FenceStyle {
    FenceStyle::Glass
}

/// `opacity` 未持久化时的默认值（与旧版 `bg_color` 的 alpha 一致，迁移平滑）。
fn default_opacity() -> f32 {
    0.55
}

impl Default for FenceAppearance {
    fn default() -> Self {
        Self {
            bg_color: [0.08, 0.08, 0.12, 0.55],
            corner_radius: 12.0,
            acrylic: true,
            title_bar_height: 32.0,
            padding: 12.0,
            icon_size: 48.0,
            gap: 10.0,
            layout: FenceLayout::Grid,
            bg_style: FenceStyle::Glass,
            border_width: 1.75,
            opacity: 0.55,
            tint: None,
            sidebar_pos: SidebarPosition::Left,
        }
    }
}

/// 分类规则预设模板。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CategoryPreset {
    /// 应用程序与快捷方式
    Apps,
    /// 常用文档
    Documents,
    /// 图片、视频与音频媒体
    Media,
    /// 压缩包与磁盘镜像
    Archives,
    /// 文件夹目录
    Folders,
}

impl CategoryPreset {
    pub const ALL: [CategoryPreset; 5] = [
        CategoryPreset::Apps,
        CategoryPreset::Documents,
        CategoryPreset::Media,
        CategoryPreset::Archives,
        CategoryPreset::Folders,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Apps => "应用",
            Self::Documents => "文档",
            Self::Media => "媒体",
            Self::Archives => "压缩",
            Self::Folders => "目录",
        }
    }

    /// 自动创建分类栅栏时的默认专属标题。
    pub fn default_fence_title(self) -> &'static str {
        match self {
            Self::Apps => "常用应用",
            Self::Documents => "办公文档",
            Self::Media => "图片媒体",
            Self::Archives => "压缩文件",
            Self::Folders => "文件目录",
        }
    }

    /// 根据图标属性与扩展名智能判定所属的预设类别。
    pub fn classify_icon(icon: &Icon) -> Option<CategoryPreset> {
        let ext = icon
            .path
            .as_deref()
            .and_then(file_extension)
            .or_else(|| file_extension(&icon.display_name))
            .map(|s| s.to_ascii_lowercase());

        // 1. 文件夹优先判定
        if icon.kind == ItemKind::Folder {
            return Some(CategoryPreset::Folders);
        }

        // 2. 常用应用与快捷方式
        if icon.kind == ItemKind::App
            || icon.kind == ItemKind::Link
            || ext
                .as_deref()
                .map(|e| EXT_APPS.contains(&e))
                .unwrap_or(false)
        {
            return Some(CategoryPreset::Apps);
        }

        // 3. 压缩文件
        if ext
            .as_deref()
            .map(|e| EXT_ARCHIVES.contains(&e))
            .unwrap_or(false)
        {
            return Some(CategoryPreset::Archives);
        }

        // 4. 图片媒体
        if ext
            .as_deref()
            .map(|e| EXT_MEDIA.contains(&e))
            .unwrap_or(false)
        {
            return Some(CategoryPreset::Media);
        }

        // 5. 办公文档（明确在文档扩展名列表中，或无扩展名但属于 Doc）
        if ext
            .as_deref()
            .map(|e| EXT_DOCS.contains(&e))
            .unwrap_or(false)
            || (ext.is_none() && icon.kind == ItemKind::Doc)
        {
            return Some(CategoryPreset::Documents);
        }

        None
    }
}

/// 栅栏绑定的分类规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FenceRule {
    /// 是否启用规则。
    pub enabled: bool,
    /// 预设类别。
    pub preset: Option<CategoryPreset>,
    /// 用户自定义扩展名（例如 ["log", "dump"]）。
    pub custom_extensions: Vec<String>,
    /// 是否在桌面出现新文件时自动捕获进此栅栏。
    pub auto_capture: bool,
}

impl Default for FenceRule {
    fn default() -> Self {
        Self {
            enabled: true,
            preset: None,
            custom_extensions: Vec::new(),
            auto_capture: true,
        }
    }
}

const EXT_APPS: &[&str] = &[
    "lnk",
    "exe",
    "bat",
    "cmd",
    "url",
    "appref-ms",
    "msi",
    "ps1",
    "vbs",
];
const EXT_DOCS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md", "markdown", "wps", "rtf",
    "csv", "epub", "mobi", "pages", "numbers", "key", "html", "htm", "xml", "json", "yaml", "yml",
];
const EXT_MEDIA: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "psd", "ai", "raw", "cr2", "nef",
    "mp4", "mkv", "avi", "mov", "flv", "wmv", "webm", "m4v", "mp3", "wav", "flac", "aac", "m4a",
    "ogg", "wma",
];
const EXT_ARCHIVES: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "dmg", "cab", "tgz",
];

/// 从文件名或路径中提取扩展名（不含点，小写）。
fn file_extension(name_or_path: &str) -> Option<&str> {
    let s = name_or_path.rsplit(['/', '\\']).next()?;
    let dot = s.rfind('.')?;
    if dot == 0 || dot == s.len() - 1 {
        None
    } else {
        Some(&s[dot + 1..])
    }
}

impl FenceRule {
    /// 判断图标是否满足此分类规则。
    pub fn matches_icon(&self, icon: &Icon) -> bool {
        if !self.enabled {
            return false;
        }
        let ext = icon
            .path
            .as_deref()
            .and_then(file_extension)
            .or_else(|| file_extension(&icon.display_name))
            .map(|s| s.to_ascii_lowercase());

        if let Some(ref ext_str) = ext {
            if self
                .custom_extensions
                .iter()
                .any(|ce| ce.trim_start_matches('.').eq_ignore_ascii_case(ext_str))
            {
                return true;
            }
        }

        match self.preset {
            Some(CategoryPreset::Folders) => icon.kind == ItemKind::Folder,
            Some(CategoryPreset::Apps) => {
                icon.kind == ItemKind::App
                    || icon.kind == ItemKind::Link
                    || ext
                        .as_deref()
                        .map(|e| EXT_APPS.contains(&e))
                        .unwrap_or(false)
            }
            Some(CategoryPreset::Documents) => {
                if let Some(ref e) = ext {
                    EXT_DOCS.contains(&e.as_str())
                } else {
                    icon.kind == ItemKind::Doc
                }
            }
            Some(CategoryPreset::Media) => ext
                .as_deref()
                .map(|e| EXT_MEDIA.contains(&e))
                .unwrap_or(false),
            Some(CategoryPreset::Archives) => ext
                .as_deref()
                .map(|e| EXT_ARCHIVES.contains(&e))
                .unwrap_or(false),
            None => false,
        }
    }
}

/// 栅栏。绑定「显式图标成员列表」，可选分类规则。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fence {
    pub id: u64,
    /// 用户自拟标题，可留空。
    pub title: Option<String>,
    /// 所属显示器（逻辑坐标基准）。
    pub monitor_id: u32,
    /// 栅栏几何（物理像素；与 overlay 虚拟屏幕坐标一致，渲染直接用）。
    pub bounds: Rect,
    pub state: FenceState,
    /// 成员顺序即布局顺序。
    pub icon_ids: Vec<ItemId>,
    pub appearance: FenceAppearance,
    /// 内容滚动偏移（物理像素；0 = 未滚动）。内容超出可视区时用滚轮滚动。
    #[serde(default)]
    pub scroll: f32,
    /// 栅栏内项目的自定义存储位置（绝对路径）。None = 使用默认内部库。
    /// 用户可在控制台中更改此路径，已有的库内项会被移动到新位置。
    #[serde(default)]
    pub storage_path: Option<String>,
    /// 侧边栏是否折叠（仅 Sidebar 布局有效）。折叠后只显示一个小箭头。
    #[serde(default)]
    pub sidebar_collapsed: bool,
    /// 分类规则配置。None = 无分类规则。
    #[serde(default)]
    pub rule: Option<FenceRule>,
    /// 栅栏是否收起（折叠仅留标题栏）。
    #[serde(default)]
    pub collapsed: bool,
}

impl Fence {
    /// 碰撞 / 夹屏用的真实高度（物理像素）。
    ///
    /// `bounds.h > 0` = 用户手动缩放过的固定高度；`bounds.h <= 0` = 自动高度，真实高度
    /// 由最近一次布局回写的 `layout_h` 提供（0 高当真实高度用会把碰撞检测与夹屏一起带偏）。
    ///
    /// **收起态不改变本口径**：折叠只影响视觉高度，原矩形始终是碰撞占位的依据——
    /// 否则展开时会与其他栅栏重叠，而展开并不做重新避让。
    pub fn collision_height(&self, layout_h: f32) -> f32 {
        if self.bounds.h > 0.0 {
            self.bounds.h
        } else {
            layout_h
        }
    }

    /// 碰撞 / 夹屏用的真实矩形：左上角与宽度取 `bounds`，高度按 [`Self::collision_height`]。
    ///
    /// 这是全工程唯一的碰撞口径真源（拖动、避让、启动重叠消解、占位框提示共用）。
    pub fn collision_rect(&self, layout_h: f32) -> Rect {
        Rect::new(
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.collision_height(layout_h),
        )
    }
}

/// 图标元数据。核心层只关心标识与展示信息。
///
/// 新增字段均带 `#[serde(default)]`，保证旧版 `desk.json` 能加载。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Icon {
    pub id: ItemId,
    pub display_name: String,
    pub kind: ItemKind,
    /// 文件系统路径；虚拟项为 None。持久化后重启可恢复图标/打开能力。
    #[serde(default)]
    pub path: Option<String>,
    /// 文件类型标签（"文件夹"、"文本文档"…）；空 = 未知/虚拟项。
    #[serde(default)]
    pub type_label: String,
    /// 最近修改时间（unix 秒）；无法读取为 None。
    #[serde(default)]
    pub modified_secs: Option<i64>,
    /// 文件大小（字节）；文件夹/未知为 None。
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// 是否为拖入/粘贴新增的项（非桌面枚举而来）。移除时直接删除，不回桌面。
    #[serde(default)]
    pub added: bool,
}

impl Icon {
    /// 基础构造（详情字段留空，由 `details::enrich` 按路径补齐）。
    pub fn new(id: ItemId, display_name: String, kind: ItemKind) -> Self {
        Self {
            id,
            display_name,
            kind,
            path: None,
            type_label: String::new(),
            modified_secs: None,
            size_bytes: None,
            added: false,
        }
    }
}

/// 图标当前的归属位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconLocation {
    /// 未分组图标区。
    Free,
    /// 位于指定栅栏内。
    Fence(u64),
}

/// 待办事项条目（控制台第一个插件的数据）。二级结构：名称 + 详细信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    /// 稳定 id（行动画/删除定位用；旧数据缺失时回退 0）。
    #[serde(default)]
    pub id: u64,
    /// 事项名称（一级标题）。旧配置字段名 `text`，反序列化时兼容。
    #[serde(alias = "text")]
    pub name: String,
    /// 详细信息（二级副标题）；可为空（只显示名称）。
    #[serde(default)]
    pub detail: String,
    /// 是否已完成。
    #[serde(default)]
    pub done: bool,
}

impl TodoItem {
    pub fn new(id: u64, name: String, detail: String) -> Self {
        Self {
            id,
            name,
            detail,
            done: false,
        }
    }
}

/// 桌面全局状态，唯一的持久化根。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Desk {
    /// 配置版本，用于结构迁移。
    pub version: u32,
    pub settings: crate::config::AppSettings,
    pub fences: Vec<Fence>,
    /// 未分组图标区：不属于任何栅栏的图标，顺序即布局顺序。
    pub free_icons: Vec<ItemId>,
    pub icons: HashMap<ItemId, Icon>,
    /// 待办插件数据（控制台第一个插件）。
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    /// 下一个待办 id（`TodoItem.id` 分配；自增保证唯一）。
    #[serde(default = "default_next_todo_id")]
    pub next_todo_id: u64,
    /// 控制台面板是否显示（右上角插件面板）。
    /// 旧配置缺失该字段时默认打开（用户要求恢复控制台）；关闭后持久化 false。
    #[serde(default = "default_console_open")]
    pub console_open: bool,
    /// 控制台面板左上角（物理像素）。None = 未拖动过，按右上角自动摆放。
    #[serde(default)]
    pub console_pos: Option<Vec2>,
    /// 控制台面板宽高（物理像素）。None = 默认尺寸（宽固定，高随待办条数自适应）。
    /// 用户拖边缘/角缩放后落为具体值；之后高度固定、超出滚动。
    #[serde(default)]
    pub console_size: Option<(f32, f32)>,
    /// 插件注册表：内置插件 + 外部清单插件的统一启用状态与数据。
    /// 旧配置缺失时默认含「待办事项」（保持既有行为）。
    #[serde(default = "default_plugins")]
    pub plugins: Vec<PluginEntry>,
    /// 桌面模式：false = 栅栏接管（隐藏真实图标）；true = 原始桌面（恢复真实图标、
    /// 栅栏淡出隐藏）。控制中心「切换桌面」按钮切换。
    #[serde(default)]
    pub desktop_mode: bool,
}

/// 插件种类：内置实现 / 外部清单（当前只有内置种类有界面实现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Todo,
    Notes,
    External,
}

impl PluginKind {
    pub fn label(self) -> &'static str {
        match self {
            PluginKind::Todo => "待办事项",
            PluginKind::Notes => "便签",
            PluginKind::External => "外部插件",
        }
    }
}

/// 插件注册项：内置插件与外部清单插件的统一持久化状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginEntry {
    pub id: String,
    pub name: String,
    pub kind: PluginKind,
    pub enabled: bool,
    pub version: String,
    pub desc: String,
    /// 便签插件内容（多行文本，持久化）。
    pub note_text: String,
}

impl PluginEntry {
    pub fn builtin_todo() -> Self {
        Self {
            id: "todo".into(),
            name: "待办事项".into(),
            kind: PluginKind::Todo,
            enabled: true,
            version: "1.0".into(),
            desc: "两级待办清单（名称 + 详情）".into(),
            note_text: String::new(),
        }
    }

    pub fn builtin_notes() -> Self {
        Self {
            id: "notes".into(),
            name: "便签".into(),
            kind: PluginKind::Notes,
            enabled: false,
            version: "1.0".into(),
            desc: "随手记多行文本，自动保存".into(),
            note_text: String::new(),
        }
    }
}

impl Default for PluginEntry {
    fn default() -> Self {
        Self::builtin_todo()
    }
}

/// `plugins` 未持久化时（旧配置）的默认注册表：待办事项启用 + 便签未启用。
fn default_plugins() -> Vec<PluginEntry> {
    vec![PluginEntry::builtin_todo(), PluginEntry::builtin_notes()]
}

/// `console_open` 未持久化时（旧配置）默认打开控制台。
fn default_console_open() -> bool {
    true
}

/// `next_todo_id` 未持久化时（旧配置）从 1 起（0 保留给无 id 的旧数据）。
fn default_next_todo_id() -> u64 {
    1
}

impl Desk {
    pub fn new(settings: crate::config::AppSettings) -> Self {
        Self {
            version: 1,
            settings,
            fences: Vec::new(),
            free_icons: Vec::new(),
            icons: HashMap::new(),
            todos: Vec::new(),
            // 首个版本默认打开控制台（用户要求恢复），关闭后持久化 false。
            next_todo_id: 1,
            console_open: true,
            console_pos: None,
            console_size: None,
            plugins: default_plugins(),
            desktop_mode: false,
        }
    }

    pub fn fence(&self, id: u64) -> Option<&Fence> {
        self.fences.iter().find(|f| f.id == id)
    }

    pub fn fence_mut(&mut self, id: u64) -> Option<&mut Fence> {
        self.fences.iter_mut().find(|f| f.id == id)
    }

    /// 下一个可用的栅栏 id（简单自增，稳定即可）。
    pub fn next_fence_id(&self) -> u64 {
        self.fences.iter().map(|f| f.id).max().unwrap_or(0) + 1
    }

    /// 查询图标的归属位置。
    pub fn icon_location(&self, id: &ItemId) -> Option<IconLocation> {
        for f in &self.fences {
            if f.icon_ids.contains(id) {
                return Some(IconLocation::Fence(f.id));
            }
        }
        if self.free_icons.contains(id) {
            Some(IconLocation::Free)
        } else {
            None
        }
    }

    /// 把一个图标从当前归属移到目标位置（None 表示未分组区）。
    /// 幂等：目标位置已包含则原样返回。图标不存在时不做任何事。
    pub fn move_icon(&mut self, id: &ItemId, to: Option<u64>) -> Option<IconLocation> {
        let from = self.icon_location(id)?;
        // 从旧位置移除
        if let Some(fid) = from.fence_id() {
            if let Some(f) = self.fence_mut(fid) {
                f.icon_ids.retain(|x| x != id);
            }
        }
        self.free_icons.retain(|x| x != id);
        // 加入新位置
        match to {
            None => {
                if !self.free_icons.contains(id) {
                    self.free_icons.push(id.clone());
                }
            }
            Some(fid) => {
                if let Some(f) = self.fence_mut(fid) {
                    if !f.icon_ids.contains(id) {
                        f.icon_ids.push(id.clone());
                    }
                } else {
                    // 目标栅栏不存在：退回未分组区
                    if !self.free_icons.contains(id) {
                        self.free_icons.push(id.clone());
                    }
                }
            }
        }
        Some(from)
    }

    /// 校验栅栏成员引用的完整性（存在但无元数据的图标会被剔除）。
    /// 用于加载配置后的防御性清理。
    pub fn validate(&mut self) {
        for f in &mut self.fences {
            f.icon_ids.retain(|id| self.icons.contains_key(id));
        }
        self.free_icons.retain(|id| self.icons.contains_key(id));
    }

    /// 根据各栅栏配置的分类规则自动整理图标。
    ///
    /// `source_fence_id`:
    /// - `Some(fid)`: 从指定栅栏（如默认桌面栅栏）及未分组区（`free_icons`）提取未归类图标；
    /// - `None`: 仅从未分组区（`free_icons`）提取未归类图标。
    ///
    /// 返回移动的列表 `(ItemId, 目标 FenceId)`。
    pub fn organize_icons_by_rules(&mut self, source_fence_id: Option<u64>) -> Vec<(ItemId, u64)> {
        let active_rules: Vec<(u64, FenceRule)> = self
            .fences
            .iter()
            .filter_map(|f| {
                f.rule
                    .as_ref()
                    .filter(|r| {
                        r.enabled && (r.preset.is_some() || !r.custom_extensions.is_empty())
                    })
                    .map(|r| (f.id, r.clone()))
            })
            .collect();

        if active_rules.is_empty() {
            return Vec::new();
        }

        let mut candidates: Vec<ItemId> = self.free_icons.clone();
        if let Some(src_id) = source_fence_id {
            if let Some(src_fence) = self.fence(src_id) {
                for id in &src_fence.icon_ids {
                    let keep = src_fence
                        .rule
                        .as_ref()
                        .map(|r| {
                            self.icons
                                .get(id)
                                .map(|ic| r.matches_icon(ic))
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if !keep && !candidates.contains(id) {
                        candidates.push(id.clone());
                    }
                }
            }
        }

        let mut moved = Vec::new();
        for item_id in candidates {
            let Some(icon) = self.icons.get(&item_id) else {
                continue;
            };
            let current_loc = self.icon_location(&item_id);
            for (target_fid, rule) in &active_rules {
                if current_loc == Some(IconLocation::Fence(*target_fid)) {
                    break;
                }
                if rule.matches_icon(icon) {
                    self.move_icon(&item_id, Some(*target_fid));
                    moved.push((item_id.clone(), *target_fid));
                    break;
                }
            }
        }

        moved
    }

    /// 全自动智能分类整理：扫描待整理图标，按需自动创建缺少的分类栅栏，并将图标分拣归位。
    ///
    /// - `source_fence_id`:
    ///   - `Some(fid)`: 待整理候选包含指定栅栏（如默认桌面栅栏）中未匹配其自身规则的图标，以及未分组区（`free_icons`）；
    ///   - `None`: 仅从未分组区（`free_icons`）提取未归类图标。
    /// - `screen_wa`: 屏幕工作区范围（物理像素，用于新栅栏自动避让排布）。若为空或零尺寸，使用安全默认矩形。
    /// - `scale`: DPI 缩放系数（用于新栅栏初始宽高与间隙计算）。
    pub fn auto_organize_all(
        &mut self,
        source_fence_id: Option<u64>,
        screen_wa: Rect,
        scale: f32,
    ) -> AutoOrganizeReport {
        let eff_scale = if scale <= 0.0 { 1.0 } else { scale };
        let wa = if screen_wa.w > 100.0 && screen_wa.h > 100.0 {
            screen_wa
        } else {
            Rect::new(0.0, 0.0, 1920.0 * eff_scale, 1080.0 * eff_scale)
        };

        // 1. 收集待整理候选图标
        let mut candidates: Vec<ItemId> = self.free_icons.clone();
        if let Some(src_id) = source_fence_id {
            if let Some(src_fence) = self.fence(src_id) {
                for id in &src_fence.icon_ids {
                    let keep = src_fence
                        .rule
                        .as_ref()
                        .map(|r| {
                            self.icons
                                .get(id)
                                .map(|ic| r.matches_icon(ic))
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if !keep && !candidates.contains(id) {
                        candidates.push(id.clone());
                    }
                }
            }
        }

        if candidates.is_empty() {
            return AutoOrganizeReport::default();
        }

        // 2. 检查是否有已存在但 rule 为 None 且标题与 preset 匹配的栅栏，为其赋予该 preset 规则以复用
        for f in &mut self.fences {
            if f.rule.is_none() {
                if let Some(title) = f.title.as_deref() {
                    for preset in &CategoryPreset::ALL {
                        if title == preset.default_fence_title() {
                            f.rule = Some(FenceRule {
                                enabled: true,
                                preset: Some(*preset),
                                custom_extensions: Vec::new(),
                                auto_capture: true,
                            });
                            break;
                        }
                    }
                }
            }
        }

        // 分析各候选图标，检查哪些已有栅栏可接纳，哪些类别需要按需新建栅栏
        let mut needed_presets: Vec<CategoryPreset> = Vec::new();
        for item_id in &candidates {
            let Some(icon) = self.icons.get(item_id) else {
                continue;
            };

            // 检查当前是否已有已配置有效规则的栅栏能够匹配此图标
            let already_covered = self.fences.iter().any(|f| {
                f.rule
                    .as_ref()
                    .map(|r| r.matches_icon(icon))
                    .unwrap_or(false)
            });

            if !already_covered {
                if let Some(preset) = CategoryPreset::classify_icon(icon) {
                    let has_reusable_fence = self.fences.iter().any(|f| {
                        f.rule
                            .as_ref()
                            .map(|r| r.enabled && r.preset == Some(preset))
                            .unwrap_or(false)
                    });

                    if !has_reusable_fence && !needed_presets.contains(&preset) {
                        needed_presets.push(preset);
                    }
                }
            }
        }

        // 3. 为缺失的类别按需创建新分类栅栏（永不重叠，settle_move 智能排布）
        let mut created_fences = 0;
        let fence_w = (320.0 * eff_scale).round();
        let fence_h = (220.0 * eff_scale).round();
        let gap = crate::magnet::FENCE_GAP * eff_scale;

        // 收集所有现有栅栏的碰撞矩形
        let mut occupied_rects: Vec<Rect> = self
            .fences
            .iter()
            .map(|f| {
                let h = if f.bounds.h > 0.0 {
                    f.bounds.h
                } else {
                    fence_h
                };
                Rect::new(f.bounds.x, f.bounds.y, f.bounds.w, h)
            })
            .collect();

        // 保证创建顺序按 CategoryPreset::ALL 的标准顺序
        let mut ordered_new_presets: Vec<CategoryPreset> = Vec::new();
        for p in &CategoryPreset::ALL {
            if needed_presets.contains(p) {
                ordered_new_presets.push(*p);
            }
        }

        for (idx, preset) in ordered_new_presets.iter().enumerate() {
            let fid = self.next_fence_id();
            let start_x = wa.x + 40.0 * eff_scale + (idx as f32 % 3.0) * (fence_w + gap);
            let start_y = wa.y + 60.0 * eff_scale + ((idx as f32 / 3.0).floor()) * (fence_h + gap);
            let start_rect = Rect::new(start_x, start_y, fence_w, fence_h);

            let placed = crate::magnet::settle_move(&start_rect, &occupied_rects, &wa, gap);
            let final_bounds = Rect::new(placed.x.round(), placed.y.round(), fence_w, fence_h);

            // 加入占用，供后续新建栅栏避让
            occupied_rects.push(final_bounds);

            self.fences.push(Fence {
                id: fid,
                title: Some(preset.default_fence_title().to_string()),
                monitor_id: 0,
                bounds: final_bounds,
                state: FenceState::Expanded,
                icon_ids: Vec::new(),
                appearance: FenceAppearance::default(),
                scroll: 0.0,
                storage_path: None,
                sidebar_collapsed: false,
                rule: Some(FenceRule {
                    enabled: true,
                    preset: Some(*preset),
                    custom_extensions: Vec::new(),
                    auto_capture: true,
                }),
                collapsed: false,
            });
            created_fences += 1;
        }

        // 4. 执行图标分拣归位
        let active_rules: Vec<(u64, FenceRule)> = self
            .fences
            .iter()
            .filter_map(|f| {
                f.rule
                    .as_ref()
                    .filter(|r| {
                        r.enabled && (r.preset.is_some() || !r.custom_extensions.is_empty())
                    })
                    .map(|r| (f.id, r.clone()))
            })
            .collect();

        let mut moved_icons = 0;
        for item_id in candidates {
            let Some(icon) = self.icons.get(&item_id) else {
                continue;
            };
            let current_loc = self.icon_location(&item_id);
            for (target_fid, rule) in &active_rules {
                if current_loc == Some(IconLocation::Fence(*target_fid)) {
                    break;
                }
                if rule.matches_icon(icon) {
                    self.move_icon(&item_id, Some(*target_fid));
                    moved_icons += 1;
                    break;
                }
            }
        }

        AutoOrganizeReport {
            created_fences,
            moved_icons,
        }
    }
}

/// 全自动分类整理执行报告。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutoOrganizeReport {
    /// 新创建的栅栏数量。
    pub created_fences: usize,
    /// 成功分拣归位的图标数量。
    pub moved_icons: usize,
}

impl IconLocation {
    /// 栅栏 id；未分组区返回 None。
    pub fn fence_id(&self) -> Option<u64> {
        match self {
            IconLocation::Free => None,
            IconLocation::Fence(id) => Some(*id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desk() -> Desk {
        Desk::new(crate::config::AppSettings::default())
    }

    fn icon(id: &str) -> Icon {
        Icon::new(id.to_string(), id.to_string(), ItemKind::Unknown)
    }

    #[test]
    fn move_icon_between_locations() {
        let mut d = desk();
        d.icons.insert("a".into(), icon("a"));
        d.icons.insert("b".into(), icon("b"));
        d.free_icons = vec!["a".into(), "b".into()];

        let f1 = d.next_fence_id();
        d.fences.push(Fence {
            id: f1,
            title: Some("工作".into()),
            monitor_id: 0,
            bounds: Rect::new(0.0, 0.0, 300.0, 200.0),
            state: FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            rule: None,
            collapsed: false,
        });

        // 移到栅栏
        let from = d.move_icon(&"a".into(), Some(f1));
        assert_eq!(from, Some(IconLocation::Free));
        assert_eq!(d.icon_location(&"a".into()), Some(IconLocation::Fence(f1)));

        // 移回未分组区
        let from = d.move_icon(&"a".into(), None);
        assert_eq!(from, Some(IconLocation::Fence(f1)));
        assert_eq!(d.icon_location(&"a".into()), Some(IconLocation::Free));
    }

    #[test]
    fn move_icon_to_missing_fence_falls_back_to_free() {
        let mut d = desk();
        d.icons.insert("a".into(), icon("a"));
        d.free_icons = vec!["a".into()];
        let from = d.move_icon(&"a".into(), Some(999));
        assert_eq!(from, Some(IconLocation::Free));
        assert_eq!(d.icon_location(&"a".into()), Some(IconLocation::Free));
    }

    #[test]
    fn validate_drops_dangling_members() {
        let mut d = desk();
        d.icons.insert("a".into(), icon("a"));
        d.fences.push(Fence {
            id: 1,
            title: None,
            monitor_id: 0,
            bounds: Rect::default(),
            state: FenceState::Expanded,
            icon_ids: vec!["a".into(), "ghost".into()],
            appearance: FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            rule: None,
            collapsed: false,
        });
        d.free_icons = vec!["ghost2".into()];
        d.validate();
        assert_eq!(d.fences[0].icon_ids, vec!["a".to_string()]);
        assert!(d.free_icons.is_empty());
    }

    #[test]
    fn rule_preset_matching_and_organize() {
        let mut d = desk();
        let mut ic_doc = icon("doc1");
        ic_doc.path = Some("C:\\Users\\Desktop\\report.docx".into());
        let mut ic_pic = icon("pic1");
        ic_pic.path = Some("C:\\Users\\Desktop\\avatar.png".into());
        let mut ic_app = icon("app1");
        ic_app.path = Some("C:\\Users\\Desktop\\Tool.lnk".into());
        let mut ic_folder = icon("fold1");
        ic_folder.kind = ItemKind::Folder;

        d.icons.insert("doc1".into(), ic_doc);
        d.icons.insert("pic1".into(), ic_pic);
        d.icons.insert("app1".into(), ic_app);
        d.icons.insert("fold1".into(), ic_folder);
        d.free_icons = vec!["doc1".into(), "pic1".into(), "app1".into(), "fold1".into()];

        // 栅栏 1：文档
        d.fences.push(Fence {
            id: 1,
            title: Some("文档".into()),
            monitor_id: 0,
            bounds: Rect::default(),
            state: FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            rule: Some(FenceRule {
                enabled: true,
                preset: Some(CategoryPreset::Documents),
                custom_extensions: Vec::new(),
                auto_capture: true,
            }),
            collapsed: false,
        });

        // 栅栏 2：媒体
        d.fences.push(Fence {
            id: 2,
            title: Some("媒体".into()),
            monitor_id: 0,
            bounds: Rect::default(),
            state: FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            rule: Some(FenceRule {
                enabled: true,
                preset: Some(CategoryPreset::Media),
                custom_extensions: Vec::new(),
                auto_capture: true,
            }),
            collapsed: false,
        });

        // 执行整理
        let moved = d.organize_icons_by_rules(None);
        assert_eq!(moved.len(), 2);
        assert_eq!(
            d.icon_location(&"doc1".into()),
            Some(IconLocation::Fence(1))
        );
        assert_eq!(
            d.icon_location(&"pic1".into()),
            Some(IconLocation::Fence(2))
        );
        assert_eq!(d.icon_location(&"app1".into()), Some(IconLocation::Free));
        assert_eq!(d.icon_location(&"fold1".into()), Some(IconLocation::Free));
    }

    #[test]
    fn classify_icon_categories() {
        let mut app_icon = icon("app");
        app_icon.path = Some("C:\\Tools\\IDE.exe".into());
        assert_eq!(
            CategoryPreset::classify_icon(&app_icon),
            Some(CategoryPreset::Apps)
        );

        let mut link_icon = icon("link");
        link_icon.kind = ItemKind::Link;
        assert_eq!(
            CategoryPreset::classify_icon(&link_icon),
            Some(CategoryPreset::Apps)
        );

        let mut doc_icon = icon("doc");
        doc_icon.path = Some("C:\\Files\\notes.md".into());
        assert_eq!(
            CategoryPreset::classify_icon(&doc_icon),
            Some(CategoryPreset::Documents)
        );

        let mut media_icon = icon("pic");
        media_icon.path = Some("C:\\Photos\\test.png".into());
        assert_eq!(
            CategoryPreset::classify_icon(&media_icon),
            Some(CategoryPreset::Media)
        );

        let mut zip_icon = icon("zip");
        zip_icon.path = Some("C:\\Downloads\\pack.7z".into());
        assert_eq!(
            CategoryPreset::classify_icon(&zip_icon),
            Some(CategoryPreset::Archives)
        );

        let mut folder_icon = icon("folder");
        folder_icon.kind = ItemKind::Folder;
        assert_eq!(
            CategoryPreset::classify_icon(&folder_icon),
            Some(CategoryPreset::Folders)
        );

        let mut unknown_icon = icon("unknown");
        unknown_icon.path = Some("C:\\Other\\data.weird_ext".into());
        assert_eq!(CategoryPreset::classify_icon(&unknown_icon), None);
    }

    #[test]
    fn auto_organize_creates_fences_on_demand_and_no_overlaps() {
        let mut d = desk();
        let mut ic_app = icon("app1");
        ic_app.path = Some("C:\\Users\\Desktop\\VSCode.lnk".into());
        let mut ic_doc = icon("doc1");
        ic_doc.path = Some("C:\\Users\\Desktop\\finance.xlsx".into());
        let mut ic_pic = icon("pic1");
        ic_pic.path = Some("C:\\Users\\Desktop\\wallpaper.jpg".into());
        let mut ic_unknown = icon("unk1");
        ic_unknown.path = Some("C:\\Users\\Desktop\\data.bin".into());

        d.icons.insert("app1".into(), ic_app);
        d.icons.insert("doc1".into(), ic_doc);
        d.icons.insert("pic1".into(), ic_pic);
        d.icons.insert("unk1".into(), ic_unknown);
        d.free_icons = vec!["app1".into(), "doc1".into(), "pic1".into(), "unk1".into()];

        let wa = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let report = d.auto_organize_all(None, wa, 1.0);

        // 仅创建了需要的 3 个栅栏（常用应用、办公文档、图片媒体），不应创建压缩文件或文件目录
        assert_eq!(report.created_fences, 3);
        assert_eq!(report.moved_icons, 3);
        assert_eq!(d.fences.len(), 3);

        let titles: Vec<&str> = d
            .fences
            .iter()
            .map(|f| f.title.as_deref().unwrap_or(""))
            .collect();
        assert!(titles.contains(&"常用应用"));
        assert!(titles.contains(&"办公文档"));
        assert!(titles.contains(&"图片媒体"));
        assert!(!titles.contains(&"压缩文件"));
        assert!(!titles.contains(&"文件目录"));

        // 特殊未知文件留在原处
        assert_eq!(d.icon_location(&"unk1".into()), Some(IconLocation::Free));

        // 栅栏之间互不重叠
        for i in 0..d.fences.len() {
            for j in (i + 1)..d.fences.len() {
                let r1 = &d.fences[i].bounds;
                let r2 = &d.fences[j].bounds;
                let overlap_x = (r1.right().min(r2.right()) - r1.x.max(r2.x)).max(0.0);
                let overlap_y = (r1.bottom().min(r2.bottom()) - r1.y.max(r2.y)).max(0.0);
                assert!(
                    overlap_x == 0.0 || overlap_y == 0.0,
                    "Fence {i} and {j} should not overlap"
                );
            }
        }

        // 再次整理应幂等
        let report2 = d.auto_organize_all(None, wa, 1.0);
        assert_eq!(report2.created_fences, 0);
        assert_eq!(report2.moved_icons, 0);
    }

    #[test]
    fn auto_organize_reuses_existing_fence() {
        let mut d = desk();
        let mut ic_doc = icon("doc1");
        ic_doc.path = Some("C:\\Users\\Desktop\\spec.pdf".into());
        let mut ic_zip = icon("zip1");
        ic_zip.path = Some("C:\\Users\\Desktop\\backup.zip".into());

        d.icons.insert("doc1".into(), ic_doc);
        d.icons.insert("zip1".into(), ic_zip);
        d.free_icons = vec!["doc1".into(), "zip1".into()];

        // 已存在一个配置了文档规则的栅栏
        let doc_fence_id = d.next_fence_id();
        d.fences.push(Fence {
            id: doc_fence_id,
            title: Some("我的文档".into()),
            monitor_id: 0,
            bounds: Rect::new(50.0, 50.0, 300.0, 200.0),
            state: FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            rule: Some(FenceRule {
                enabled: true,
                preset: Some(CategoryPreset::Documents),
                custom_extensions: Vec::new(),
                auto_capture: true,
            }),
            collapsed: false,
        });

        let wa = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let report = d.auto_organize_all(None, wa, 1.0);

        // 应该只新建 1 个压缩文件栅栏，并复用已有的文档栅栏
        assert_eq!(report.created_fences, 1);
        assert_eq!(report.moved_icons, 2);
        assert_eq!(
            d.icon_location(&"doc1".into()),
            Some(IconLocation::Fence(doc_fence_id))
        );
        let zip_fence = d
            .fences
            .iter()
            .find(|f| f.title.as_deref() == Some("压缩文件"))
            .expect("应创建压缩文件栅栏");
        assert_eq!(
            d.icon_location(&"zip1".into()),
            Some(IconLocation::Fence(zip_fence.id))
        );
    }

    #[test]
    fn rect_contains_and_inset() {
        let r = Rect::new(10.0, 10.0, 100.0, 50.0);
        assert!(r.contains(Vec2 { x: 10.0, y: 10.0 }));
        assert!(!r.contains(Vec2 { x: 111.0, y: 10.0 }));
        let inner = r.inset(5.0);
        assert_eq!(inner, Rect::new(15.0, 15.0, 90.0, 40.0));
    }

    #[test]
    fn fence_layout_cycles() {
        assert_eq!(FenceLayout::Grid.next(), FenceLayout::List);
        assert_eq!(FenceLayout::List.next(), FenceLayout::Sidebar);
        assert_eq!(FenceLayout::Sidebar.next(), FenceLayout::Grid);
    }

    #[test]
    fn winbosk_appearance_serde_backward_compatible() {
        // 旧版 desk.json 的栅栏外观（无 style / border_width / layout）应能加载并取默认值
        let old = r#"{"bg_color":[0.08,0.08,0.12,0.55],"corner_radius":12.0,"acrylic":true,"title_bar_height":32.0,"padding":12.0,"icon_size":48.0,"gap":10.0}"#;
        let a: FenceAppearance = serde_json::from_str(old).expect("旧配置应可反序列化");
        assert_eq!(a.bg_style, FenceStyle::Glass);
        assert_eq!(a.border_width, 1.75);
        assert_eq!(a.layout, FenceLayout::Grid);
        assert_eq!(a.opacity, 0.55);
        assert_eq!(a.tint, None);
    }

    #[test]
    fn desk_console_fields_serde_backward_compatible() {
        // 旧版 desk.json 无 todos / console_open / console_pos：应取默认（打开控制台）
        let old = r#"{"version":1,"settings":{"show_free_area":true,"free_area_height":90.0,"hotkeys":{},"autostart":false},"fences":[],"free_icons":[],"icons":{}}"#;
        let d: Desk = serde_json::from_str(old).expect("旧配置应可反序列化");
        assert!(d.todos.is_empty());
        assert_eq!(d.next_todo_id, 1);
        assert!(d.console_open);
        assert_eq!(d.console_pos, None);
        assert_eq!(d.console_size, None);
        // 旧配置无插件/桌面模式字段：默认注册表（待办启用）+ 栅栏模式
        assert_eq!(d.plugins.len(), 2);
        assert!(d.plugins.iter().any(|p| p.id == "todo" && p.enabled));
        assert!(!d.desktop_mode);
    }

    #[test]
    fn plugin_entry_roundtrip() {
        let mut p = PluginEntry::builtin_notes();
        p.enabled = true;
        p.note_text = "买牛奶\n拿快递".into();
        let json = serde_json::to_string(&p).unwrap();
        let back: PluginEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "notes");
        assert!(back.enabled);
        assert_eq!(back.note_text, "买牛奶\n拿快递");
    }

    #[test]
    fn plugin_kind_labels() {
        assert_eq!(PluginKind::Todo.label(), "待办事项");
        assert_eq!(PluginKind::Notes.label(), "便签");
        assert_eq!(PluginKind::External.label(), "外部插件");
    }

    #[test]
    fn todo_item_roundtrip() {
        let t = TodoItem::new(7, "写周报".into(), "周五前提交".into());
        let json = serde_json::to_string(&t).unwrap();
        let back: TodoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.name, "写周报");
        assert_eq!(back.detail, "周五前提交");
        assert!(!back.done);
    }

    #[test]
    fn todo_item_old_text_field_backward_compatible() {
        // 旧配置只有 `text`（无 detail）：应映射到 name，detail 取默认空串
        let old = r#"{"id":3,"text":"旧事项","done":true}"#;
        let t: TodoItem = serde_json::from_str(old).expect("旧待办应可反序列化");
        assert_eq!(t.name, "旧事项");
        assert_eq!(t.detail, "");
        assert!(t.done);
    }

    #[test]
    fn fence_collapsed_serde_backward_compatible() {
        // 旧版 JSON（不含 collapsed 字段）
        let old_json = r#"{
            "id": 1,
            "title": "测试栅栏",
            "monitor_id": 0,
            "bounds": {"x": 10.0, "y": 20.0, "w": 300.0, "h": 200.0},
            "state": "Expanded",
            "icon_ids": [],
            "appearance": {}
        }"#;
        let fence: Fence =
            serde_json::from_str(old_json).expect("旧版未包含 collapsed 应可成功反序列化");
        assert!(!fence.collapsed, "旧版缺少 collapsed 时应默认为 false");

        // 新版 JSON（显式包含 collapsed: true）
        let new_json = r#"{
            "id": 2,
            "title": "折叠栅栏",
            "monitor_id": 0,
            "bounds": {"x": 10.0, "y": 20.0, "w": 300.0, "h": 200.0},
            "state": "Expanded",
            "icon_ids": [],
            "appearance": {},
            "collapsed": true
        }"#;
        let fence2: Fence =
            serde_json::from_str(new_json).expect("包含 collapsed: true 应可成功反序列化");
        assert!(fence2.collapsed);
    }

    fn fence_with_h(h: f32, collapsed: bool) -> Fence {
        Fence {
            id: 1,
            title: Some("t".into()),
            monitor_id: 0,
            bounds: Rect::new(10.0, 20.0, 300.0, h),
            state: FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            rule: None,
            collapsed,
        }
    }

    #[test]
    fn collision_rect_prefers_bounds_then_layout_height() {
        // 固定高度（用户手动缩放过）：用 bounds.h
        let fixed = fence_with_h(200.0, false);
        assert_eq!(fixed.collision_height(999.0), 200.0);
        // 自动高度（bounds.h <= 0）：用最近一次布局回写的真实高度
        let auto = fence_with_h(0.0, false);
        assert_eq!(auto.collision_height(180.0), 180.0);
        assert_eq!(
            auto.collision_rect(180.0),
            Rect::new(10.0, 20.0, 300.0, 180.0)
        );
    }

    #[test]
    fn collision_rect_is_unaffected_by_collapse() {
        // 收起只改视觉高度：碰撞口径必须保持原矩形，否则展开时会与邻居重叠
        //（展开不做重新避让）。
        let expanded = fence_with_h(200.0, false);
        let collapsed = fence_with_h(200.0, true);
        assert_eq!(
            expanded.collision_rect(200.0),
            collapsed.collision_rect(200.0)
        );
    }
}
