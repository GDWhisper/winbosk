# 规则接管交互重构：预设只读透明化与继承式自定义实施计划 (14-rule-preset-and-custom-flow)

本计划重构控制中心高级模式下的「规则接管」工作台，解决预设规则与自定义选项混杂、规则黑盒不透明、误修改预设等体验与架构痛点。

---

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals
1. **规则透明化（所见即所得）**：用户在分类栏选中任一预设分类（应用 / 文档 / 媒体 / 压缩 / 目录）时，下方区域直接展示该预设包含的真实后缀与匹配逻辑，杜绝黑盒。
2. **预设防改坏（默认只读）**：预设模式下展示的规则芯片为只读形态（无删除叉号、不显示添加输入入口），保证预设模板干净安全。
3. **继承式自定义（一键克隆）**：
   - 提供显式的「✏️ 编辑规则 / 转为自定义」入口；
   - 用户点击后，自动将当前所选预设的规则（如应用预设的所有内置扩展名）完整继承（克隆）到自定义规则中，并切换至「自定义」模式；
   - 在自定义模式下，才完整展开「包含后缀」「排除后缀」「文件名通配符」三大编辑区，允许自由增删改。
4. **顶部分类栏语义清晰**：
   - 第一个选项从易引起歧义的「全部」更名为「无」（无自动规则，纯手动管理）；
   - 分类栏支持明确展示当前处于「无 / 应用 / 文档 / 媒体 / 压缩 / 目录 / 自定义」何种模式；
   - 用户在自定义模式下随时可以点击上方预设分类快速重置/切回预设。

### Non-Goals
1. 不变动图标的基础数据模型（`Icon`）与存储位置判定口径。
2. 不改动多栅栏规则冲突对话框（`TaskDialogIndirect`）的深层交互逻辑。
3. 不引入外部脚本引擎或正则匹配语法，保持纯 Rust 领域模型与 Glob 简单通配符的高性能与轻量性。

---

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 核心层领域模型 (`winbosk_core::model`)

#### `CategoryPreset` 增强
为预设分类提供单一真源的后缀查询能力，供 UI 只读展示与自定义继承克隆使用：
```rust
impl CategoryPreset {
    pub const ALL: [CategoryPreset; 5] = [
        CategoryPreset::Apps,
        CategoryPreset::Documents,
        CategoryPreset::Media,
        CategoryPreset::Archives,
        CategoryPreset::Folders,
    ];

    /// 获取该预设包含的所有内置扩展名（用于渲染展示与继承克隆）。
    pub fn default_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Apps => EXT_APPS,
            Self::Documents => EXT_DOCS,
            Self::Media => EXT_MEDIA,
            Self::Archives => EXT_ARCHIVES,
            Self::Folders => &[],
        }
    }
}
```

#### `FenceRule` 状态机演进
添加 `is_custom` 显式标识，与 `preset` 共同表达「无规则 / 预设规则 / 自定义规则」三种正交状态：
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FenceRule {
    pub enabled: bool,
    /// 是否为自定义规则模式。
    /// - false: 遵循 preset 分类逻辑（若 preset 为 None 则为无规则）；
    /// - true: 遵循 custom_extensions / exclude_extensions / name_patterns 自定义规则。
    pub is_custom: bool,
    /// 预设类别（当 !is_custom 时生效）。
    pub preset: Option<CategoryPreset>,
    /// 用户自定义扩展名白名单。
    pub custom_extensions: Vec<String>,
    /// 排除的扩展名黑名单。
    pub exclude_extensions: Vec<String>,
    /// 文件名通配符/关键字白名单。
    pub name_patterns: Vec<String>,
    /// 文件名排除通配符/关键字。
    pub exclude_patterns: Vec<String>,
    /// 是否自动捕获桌面新文件。
    pub auto_capture: bool,
}
```

#### 规则有效性与匹配判定
```rust
impl FenceRule {
    /// 规则是否包含实际生效的匹配条件
    pub fn is_effective(&self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.is_custom {
            !self.custom_extensions.is_empty() || !self.name_patterns.is_empty()
        } else {
            self.preset.is_some()
        }
    }

    /// 将指定预设的规则内容继承克隆到当前自定义规则中
    pub fn inherit_from_preset(&mut self, preset: Option<CategoryPreset>) {
        self.is_custom = true;
        self.preset = None;
        if let Some(p) = preset {
            self.custom_extensions = p.default_extensions().iter().map(|s| s.to_string()).collect();
        }
    }
}
```

---

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

1. **常驻后台冲突律 (Daemon Invariant)**：
   - 4s 后台库同步与新文件自动捕获（`auto_capture`）完全依据 `rule.matches_icon(icon)` 判断。
   - 重构后，预设模式只匹配预设分类，自定义模式严格匹配自定义扩展名/通配符，两者互不污染，消除此前“选了应用但残留旧自定义后缀导致意外误抓”的隐性 bug。
2. **空闲性能归零律 (Zero-Idle Invariant)**：
   - 切换预设、点击编辑进入自定义、增删芯片均属于离散事件驱动，由用户操作触发，处理后即时持久化到 `desk.json` 并调用 `inject_rebuild`，无后台轮询开销。
3. **孤儿与物理变更回流 (Orphan Invariant)**：
   - 仅改变规则判定范围，不触碰文件物理路径与桌面还原守卫 `IconGuard`。
4. **语义精准定位律 (Semantic Invariant)**：
   - 预设分类与自定义模式使用显式枚举 `RuleCategoryChoice` 传递，不再将 `None` 歧义地命名为「全部」。
5. **状态全集校验律 (Compound State)**：
   - 复合判定 `is_effective`：必须满足 `enabled && (if is_custom { 有自定义规则 } else { 有预设 })`。
6. **旁路数据对齐律 (Sidecar Invariant)**：
   - 芯片几何与点击命中模型统一在 `scene.rs` 的一次遍历中生成，只读芯片无删除热区，编辑按钮与添加按钮热区精确绑定。

---

## 4. 分层改动清单 (Implementation Steps)

### Step 1: 核心领域模型 (`crates/core/src/model.rs`)
1. 在 `FenceRule` 添加 `pub is_custom: bool` 字段（`#[serde(default)]`）。
2. 在 `CategoryPreset` 实现 `default_extensions()`。
3. 更新 `FenceRule::matches_icon()`：若 `is_custom` 为 true，优先走自定义白名单与通配符；若为 false，走预设或返回 false。
4. 更新 `FenceRule::is_effective()`。
5. 编写纯算法单测：覆盖预设匹配、自定义匹配、继承克隆与向后兼容。

### Step 2: 渲染数据与场景构建 (`crates/render/src/scene.rs` & `crates/app/src/scene.rs`)
1. 在 `crates/render/src/overlay.rs` 和 `scene.rs`：
   - 定义分类选项枚举 `RuleCategoryChoice`：`None`（无）、`Preset(CategoryPreset)`、`Custom`（自定义）；
   - 更新 `ConsoleZone::FenceRuleCategory(RuleCategoryChoice)`（替代旧的 `FenceRulePreset`）；
   - 增加 `ConsoleZone::RuleEnterCustom`（点击编辑按钮进入自定义）；
   - 更新 `SceneRuleEditor` 结构体：
     - `category: RuleCategoryChoice`；
     - `is_custom: bool`；
     - `edit_custom_btn: Option<RectF>`（预设/无模式下的编辑入口）；
     - `readonly_chips: Vec<(String, RectF)>`（预设模式下展示的只读后缀芯片）；
     - `readonly_desc: Option<String>`（预设说明文本，如目录预设）；
2. 在 `crates/app/src/scene.rs` 中的 `build_console`：
   - 顶部生成 7 个分类按钮：`[无]` `[应用]` `[文档]` `[媒体]` `[压缩]` `[目录]` `[自定义]`；
   - 若当前为预设模式：排布只读后缀芯片与「✏️ 编辑规则」按钮，隐藏底下的三大添加按钮与删除叉号；
   - 若当前为自定义模式：高亮「自定义」分类，完整排布「包含后缀」「排除后缀」「文件名通配符」三大编辑区块；
   - 保持下方的「新文件自动捕获」开关与「立即按规则整理当前栅栏」按钮。

### Step 3: D2D 绘制管线 (`crates/render/src/draw.rs`)
1. 绘制顶部分类按钮组（高亮当前选中的无 / 预设 / 自定义）。
2. 绘制预设模式工作区：
   - 绘制小标签「预设规则内容（只读）：」与「✏️ 编辑规则」按钮；
   - 流式绘制优雅的只读规则芯片（无删除叉号，带柔和微描边）；
   - 绘制预设保护引导文案；
3. 绘制自定义模式工作区：
   - 绘制三大带添加与删除按钮的芯片组及内联输入框；
4. 绘制底部的自动捕获、单栅栏整理按钮与规则配置说明卡片。

### Step 4: 消息与事件处理 (`crates/app/src/main.rs`)
1. `ConsoleZone::FenceRuleCategory(choice)`：
   - 若点击 `None`：`rule.is_custom = false; rule.preset = None;`
   - 若点击 `Preset(p)`：`rule.is_custom = false; rule.preset = Some(p);`
   - 若点击 `Custom`：若原本不是 custom，继承当前选中的预设（若为 None 则为空白自定义）；`rule.is_custom = true;`
2. `ConsoleZone::RuleEnterCustom`：
   - 点击「✏️ 编辑规则」：直接将当前预设继承克隆到自定义规则中，切换到自定义模式。
3. 统一触发存储保存并 `inject_rebuild`。

---

## 5. 防御性自查清单 (Defensive Invariants)
- [x] **向后兼容**：旧 `desk.json` 缺少 `is_custom` 字段时自动反序列化为 `false`；若旧数据同时有 preset 和自定义后缀，做合理兼容。
- [x] **D2D 绘图安全**：所有新增矩形与文字度量统一乘以 `theme.scale`，避免 DPI 缩放错位。
- [x] **无空指针/越界**：芯片列表为空或极多时，流式布局自动换行并留出充足空间。

---

## 6. 验证与交付门禁 (Verification Gates)
1. `cargo test -p winbosk-core`：验证规则匹配与继承逻辑全部通过。
2. `cargo build --workspace`：全工作区无编译错误。
3. `cargo clippy --workspace -- -D warnings`：零警告。
4. `cargo fmt --all -- --check`：格式化合格。
5. 运行验证：呼出控制中心，点击各预设分类，验证只读芯片正确展示；点击编辑规则进入自定义，验证后缀继承并可自由编辑删除；切回预设验证恢复只读。
