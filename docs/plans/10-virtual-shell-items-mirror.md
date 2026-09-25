# 桌面图标全量归类与「系统应用（虚拟壳项）」镜像 实施计划

**缘起（实证）**：用户要求「确保所有在桌面上的图标都能归类；系统应用（虚拟壳项，如回收站）
默认保留归类在初始的【桌面】栅栏里」。

本计划前两轮只做**纯调研**（未改任何业务代码），把「桌面到底有哪些图标、Windows 到底怎么决定
哪些虚拟项显示在桌面、无路径项能不能干活」全部跑成 ground truth。下面第一、二节先固化这些事实，
再据此设计方案——**所有结论都有实测或一手文档背书，无推断**。

---

## 0. Ground Truth（本节是后续一切设计的前提，禁止改动时推翻）

### 0.1 全集数据（本机实测，`scripts/desktop-probe.py` + shell 枚举）

| 口径 | 数值 | 含义 |
| :--- | :--- | :--- |
| `enumerate_desktop_items()` | **166** | `IShellFolder` 枚举的全部桌面项 |
| DefView（`SysListView32`，真实显示） | **150** | Windows 真正显示给用户的 |
| 有文件系统路径的项 | **157** | 用户桌面 105 + 公共桌面 34 + 真实文件 |
| **无路径的虚拟壳项** | **9** | `path == None`，`kind == Unknown`，`id = "shell:<显示名>"` |

9 个虚拟项（附 `SHGDN_FORPARSING` 解析名实测）：

| 显示名 | 解析名 / CLSID | path | kind | id |
| :--- | :--- | :--- | :--- | :--- |
| 此电脑 | `::{20D04FE0-3AEA-1069-A2D8-08002B30309D}` | None | Unknown | `shell:此电脑` |
| 回收站 | `::{645FF040-5081-101B-9F08-00AA002F954E}` | None | Unknown | `shell:回收站` |
| 控制面板 | `::{26EE0668-A00A-44D7-9371-BEB064C98683}` | None | Unknown | `shell:控制面板` |
| 控制面板 | `::{5399E694-6CE5-4D6C-8FCE-1D8870FDCBA0}` | None | Unknown | `shell:控制面板`（**同名第 2 个**） |
| 库 | `::{031E4825-7B94-4DC3-B131-E946B44C8DD5}` | None | Unknown | `shell:库` |
| Linux | `::{B2B4A4D1-2754-4140-A2EB-9A76D9D7CDC6}` | None | Unknown | `shell:Linux` |
| 图库 | `::{E88865EA-0E1C-4E20-9AA6-EDCD0212C87C}` | None | Unknown | `shell:图库` |
| 网络 | `::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}` | None | Unknown | `shell:网络` |
| 主文件夹 | `::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}` | None | Unknown | `shell:主文件夹` |

**集合核对（实测闭合，无一项对不上）**：`enumerate_desktop_items()` 166 = 有路径 157 + 虚拟 9；
`desktop-probe.py` 读 DefView = 磁盘可见 147（磁盘 151 − 隐藏/系统 2）+ **虚拟项 1（回收站）**
= 148（另有 2 组「同名文件 + 同名 .lnk」在 DefView 上显示名相同，故去重后 148）。
⇒ 当前桌面栅栏比真实桌面**少的正是虚拟壳项**（文件侧已由 plan 08 覆盖）。

### 0.2 可见性判定：只有 HKLM strategy 不能解释（打破 plan 08 的旧假设）

plan 08 §6.3 曾推断「属性位无法区分，需要读 `HKCU\...\HideDesktopIcons\{NewStartPanel,
ClassicStartMenu}`」。本轮把注册表全读完了，**结论更强也更简单**：

| 来源 | 内容 | 实测 |
| :--- | :--- | :--- |
| `HKCU\...\HideDesktopIcons\NewStartPanel` | 仅 1 项：`{5C899A03-…坚果云} = 1` | 隐藏坚果云 |
| `HKCU\...\HideDesktopIcons\ClassicStartMenu` | 不存在 | — |
| `HKCU\...\HideDesktopIcons` (default value) | 未设置 | — |
| `HKLM\...\HideDesktopIcons\NewStartPanel` | **17 个 CLSID 全部 = 1** | 这台机器被策略批量隐藏 |
| `HKLM\...\HideDesktopIcons\ClassicStartMenu` | 2 项：`{871C5380…}.default = 0`、`{9343812E…}=1` | — |

把 9 个虚拟项逐个对照 HKLM 策略（`reg query` 实测）：

| 虚拟项 | 在 HKLM NewStartPanel 策略里？ | DefView 实际显示？ |
| :--- | :--- | :--- |
| 此电脑 | **是（隐藏）** | 否 |
| 回收站 | **否** | **是（唯一显示的虚拟项）** |
| 控制面板(全) `{26EE0668…}` | 否 | 否 |
| 控制面板(小) `{5399E694…}` | **是（隐藏）** | 否 |
| 库 | **是（隐藏）** | 否 |
| Linux | **是（隐藏）** | 否 |
| 图库 | **是（隐藏）** | 否 |
| 网络 | **是（隐藏）** | 否 |
| 主文件夹 | **是（隐藏）** | 否 |

⇒ **口径确定**：`可见(CLSID) = (HKCU 值) XOR (HKLM 策略值)`，缺值按「显示」处理。
即：HKCU 显式 `0` = 强制显示，`1` = 强制隐藏；HKLM 置 `1` = 策略隐藏（需 HKCU 显式 `0` 才能翻盘）；
两边都缺 = 显示。该口径复现本机全部 9 项判定（8 隐藏 + 1 显示），无一例外。

> 「缺值按显示」这条不是猜的：回收站两边都缺值且 DefView 显示了它；而 HKLM 把 17 个 CLSID 设成
> `1`（含此电脑/网络/库）正好对应它们不显示。若缺值按隐藏，回收站也会消失，与实测矛盾。

**控制面板(全) `{26EE0668…}` 缺值却不显示**：它不是「隐藏」，而是本机**从未把它放进桌面**
（未注册的壳入口）。这一点对实现至关重要：**判定必须能返回「不显示 → 不镜像」**，不能因为
「缺值=显示」就把它硬塞进栅栏。缺值=显示→它就会被镜像进桌面栅栏→与真实桌面不符（DefView 150）。

⇒ 因此**必须**有一条「已知虚拟壳项白名单」兜底：只有白名单内的 CLSID 才参与镜像。
（`26EE0668` 不在白名单 → 不镜像，实测与 DefView 一致。）

### 0.2.1 白名单闸门的自我否证（第一版方案写错，实测后纠正）

第一版方案曾写「白名单 + 解析名匹配两道闸」就能排除 `{26EE0668…}`——**这是错的，实测否证**：
`{26EE0668-A00A-44D7-9371-BEB064C98683}` 的解析名就是 `::{26EE0668-…}`，把它列进白名单后
**解析名闸门必然也放它过**，两道闸对它完全同向，**没有任何一道能排除它**。

⇒ 修正后的**唯一可行权威源**是：**只以「DefView / 枚举器真的返回了它」为事实**。
具体做法（放弃用注册表反推「显示」）：

| 事实源 | 用途 | 依据 |
| :--- | :--- | :--- |
| `enumerate_virtual_items()` 的返回集合 | **唯一的事实源**：Windows 返回了它 = 它显示在桌面上 | `IShellFolder::EnumObjects` 的语义；本机返回 9 项（§0.1），其中 DefView 只画回收站 |
| DefView 只画回收站 | 说明「枚举器返回 ≠ DefView 显示」 | `desktop-probe.py` 读 `SysListView32` 实测 150 项，仅回收站 1 个虚拟项 |
| `HideDesktopIcons` 注册表 | **只用于解释 / 预测，不作为准入判据** | 见 0.2 表 |

⇒ **最终判定口径（三层，任一不过即不镜像）**：

1. **白名单闸**：CLSID 在 `MIRRORABLE_VIRTUAL_ITEMS` 内（排除第三方壳扩展意外注册的项；
   `{26EE0668-…}` 故意不在表内，见 §2.1.1）；
2. **策略闸**：`HKCU` 未显式 `1`（隐藏）且 `HKLM NewStartPanel` 未置 `1`
   （策略隐藏，如本机的此电脑 / 网络 / 库 / 图库 / Linux / 主文件夹 / 控制面板小图标）；
3. **DefView 闸（真正的权威）**：该项出现在 `desktop-probe` 口径的「真实桌面项」里。
   实现上**不能**依赖读 `SysListView32`（跨进程 + 每 4s 成本不可接受），改为：
   **「启动一次全量枚举后，与注册表策略闸的并集做差集，只在用户显式点『刷新』时重算」**。

> **关键取舍（为什么不再追求「注册单表精确复现 DefView」）**：`{26EE0668…}` 证明 Windows 的
> 默认桌面集合**不全部由 `HideDesktopIcons` 管辖**。凡是不在管辖范围内的入口，注册表口径是
> **不可判定的**。因此正确做法是：**登记表策略闸 = 「保守隐藏」**（判隐藏就隐藏，判不出也不放行），
> 而「显示」的唯一权威是枚举器返回 + DefView 实测集合。
>
> 落到本机：策略闸把 8 项判隐藏（此电脑/控制面板小/库/Linux/图库/网络/主文件夹/坚果云），
> 剩 1 项回收站 + 1 项 `{26EE0668}` 判不出 → **两者都先按「不镜像」处理（保守），
> 回收站通过「启动期一次性白名单例外」显式放行，`{26EE0668}` 保持不镜像**。
> 即：白名单里**必须带一条显式例外注释**「`26EE0668` 本机不显示，勿放行」，
> 并且**实现上把 `26EE0668` 从「可镜像」集合里剔除**（不是放进白名单）。

### 0.3 无路径项能干活的证据（不是死图标）

三项全部实测通过，因此「回收站放进去是死图标」的旧担忧**已不成立**：

| 能力 | 实现路线 | 实测 |
| :--- | :--- | :--- |
| 图标位图 | `extract_icon`（PIDL → `IShellItemImageFactory`） | 9/9 成功，回收站 **0ms**，其余 0~5ms |
| 双击打开 | `ShellExecuteEx` + `SEE_MASK_INVOKEIDLIST` + `lpIDList` | 文档背书（见下） |
| 右键菜单 | `SHCreateItemFromParsingName` + `BindToHandler(BHID_SFUIObject)` | 4/4 成功（回收站冷启 **191ms**、此电脑 11.5ms、控制面板 10ms、网络 19ms） |

`SEE_MASK_INVOKEIDLIST` 一手依据（Microsoft Learn，`SHELLEXECUTEINFOA`）：

> SEE_MASK_INVOKEIDLIST (0x0000000C) — Use either lpFile to identify the item by its file system
> path or **lpIDList to identify the item by its PIDL** … allows applications to use ShellExecuteEx
> to invoke verbs from shortcut menu extensions instead of the static verbs listed in the registry.
> lpFile: To specify a Shell namespace object, pass the fully qualified parse name and set the
> SEE_MASK_INVOKEIDLIST flag in the fMask parameter.

⇒ 虚拟项用 PIDL 打 `ShellExecuteEx` 即可获得真实「打开」，`sh.exe` 的 `DesktopItem::launch()`
对 `path == None` 直接 return 的旧分支要改成 PIDL 路线。

### 0.4 心跳成本（决定同步策略，禁止破坏空闲 0% CPU）

实测（`cargo test -p winbosk-shell` 打印耗时）：

| 路径 | 成本 |
| :--- | :--- |
| `enumerate_desktop_items()` 全量（166 项，含 `GetAttributesOf`） | **11.9 ~ 14.9 ms** |
| 只扫虚拟项（`Next` + `path_of` + 虚拟项补一次 FORPARSING） | **9.65 ms** |
| plan 08 记录的 `read_dir` + 属性过滤（141 项） | 5.2 ms |

⇒ **禁止**把 `enumerate_desktop_items()` 放进 4s 心跳：现有心跳总预算 4~7 ms（plan 08 实测），
一次全量枚举就会把它撑爆 2~3 倍，直接违反铁律。虚拟项必须走**廉价路径**（见 §2.2）。

---

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

1. **桌面镜像栅栏内容与真实桌面完全一致**：用户桌面文件 + 公共桌面文件 + **可见的虚拟壳项**
   （本机即多出「回收站」1 项；策略不同的机器可能多出此电脑/网络/库等，一律以注册表口径为准）。
2. **虚拟壳项完整可用**：双击打开（回收站/此电脑/控制面板/网络等真实弹出）、真实 Shell 右键菜单、
   图标正常渲染；不再有「死图标」。
3. **虚拟壳项默认保留在初始【桌面】栅栏里**：不被「一键整理」搬走（它们不该被算进任何分类
   预设），删除分类栅栏时也**不**把虚拟项回流进桌面栅栏（它们本来就在桌面栅栏）。
4. **孤儿回收分支对虚拟项完全豁免**：虚拟项没有文件系统路径，任何 `path.exists()` 判定
   **必须跳过**（否则 4s 心跳把它们全部删掉——图标闪一下就没了）。虚
   拟项只在「注册表判定为不可见」或「枚举器已不再返回它」时移除。
5. **保持心跳廉价**：收敛态下 4s 一次同步仍以 `read_dir` + 集合比较为主，零额外系统调用；
   虚拟项同步只在**启动一次** + 注册表判定有变化时做，不做周期性 `IShellFolder` 枚举。
6. **零配置项、零 `desk.json` 结构变更**（虚拟项沿用现有 `shell:<显示名>` id 与 `Icon` 字段）。

### Non-Goals

1. **不内置「创建虚拟壳项」入口**：用户不能在栅栏里手工添加一个「此电脑」；本计划只让
   **真实桌面上已有的**虚拟项被镜像进来。
2. **不改 `storage_path` / 删除语义**：虚拟项不属于 WinBosk 管理区（`is_managed_path` 恒 false），
   右键「删除」走 Shell 原生动词（回收站语义 = 删除到回收站），WinBosk 绝不直删。
3. **不实现「虚拟项移出栅栏」**：`remove_fence_icon_by_id` 对虚拟项返回「不支持」
   （调用方给出提示），因为移出后 ≤4s 同步会把它加回来（见 §3.3）。
4. **不动渲染层 / 命中模型 / 拖拽**：虚拟项复用现有 `Icon`（`path == None`）绘制路径，
   本计划不改 `draw.rs` / `overlay.rs` 的任何几何逻辑。
5. **不做 8.3 短名 / OneDrive 重定向桌面的路径规范化**（plan 08 P2，独立议题）。

---

## 2. 状态契约与接口设计 (Contracts & Data Models)

> 本计划**不新增持久化字段**。新增的核心是一个纯函数族 + 一个启动期的注册表快照。
> 纯函数全部放 `winbosk-core`（零 OS 依赖，可内存单测）；注册表读取放 `winbosk-shell`
> （唯一允许碰 Win32 的层）。

### 2.1 `winbosk-core`：可见性口径（纯逻辑，无 Win32）

> 权威模型见 §0.2.1：**注册表只能「保守隐藏」，不能「放行」**。因此这里的函数**只回答
> 「是否应当隐藏」**，不回答「是否应当显示」。默认（缺值）= 不隐藏，但**缺值 ≠ 显示**
> ——显示与否由 shell 层的白名单例外表决定（§2.2）。这个方向性约束必须写在类型上。

```rust
// crates/core/src/shell_items.rs（新建，纯 Rust，零 Win32）

/// 注册表三态（**不用 Option<bool>**：`None` 与 `Some(false)` 语义不同，
/// plan 06 的「歧义布尔」教训）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyValue {
    /// 该键不存在。
    Unset,
    /// 显式 0。
    Enabled,
    /// 显式 1。
    Disabled,
}

impl PolicyValue {
    /// DWORD → 三态。非 0/1 的脏值按 `Enabled`（=0，不隐藏）处理并记日志。
    pub fn from_dword(v: Option<u32>) -> Self { /* … */ }
}

/// 该 CLSID 是否被「显式隐藏」（**保守方向**：只有拿到明确的隐藏信号才返回 true）。
///
/// 口径：
/// 1. HKCU 显式 `1` → true（用户隐藏；HKCU 显式 `0` → false，用户可覆盖策略）；
/// 2. HKLM 策略 = `1` 且 HKCU 未显式 `0` → true（机器策略隐藏）；
/// 3. 其余（全缺值 / HKLM 缺值 / 脏值）→ **false（不隐藏，但也未获授权显示）**。
///
/// 第 3 条是本函数与「旧版可见性函数」的**本质区别**：缺值绝不意味着「显示」。
/// 本机 `{26EE0668-…}`（控制面板）两边都缺值而 DefView 不显示它（§0.2.1），
/// 若把缺值当显示，它会被错误镜像进栅栏。
pub fn policy_hidden(clsid: &str, hkcu: PolicyValue, hklm: PolicyValue) -> bool {
    match hkcu {
        PolicyValue::Disabled => true,
        PolicyValue::Enabled => false, // 用户显式显示，压过机器策略
        PolicyValue::Unset => hklm == PolicyValue::Disabled,
    }
}
```

**为什么不保留 `virtual_item_visible`**：第一版方案里有这个函数，实测否证（§0.2.1）。
把它留在代码里就是一个**看起来能用、实际会把 `26EE0668` 放行**的陷阱，
按「删除是减重」原则直接不进实现，只在本计划留档否证过程。

### 2.1.1 可镜像白名单（`winbosk-shell` 侧，**必须逐条带实测依据**）

```rust
// crates/shell/src/virtual_items.rs

/// 可镜像的虚拟壳项。**准入由本表决定，注册表只负责否决**（§0.2.1）。
///
/// 本表按本机实测枚举出的 9 个虚拟项录入，`{26EE0668-…}`（控制面板全入口）**故意不在表内**：
/// 它两边注册表都缺值（策略闸不会否决），而 DefView 实测不显示它（§0.2.1），
/// 放进表 = 把桌面镜像里塞进一个用户看不到的东西。⇒ 表内 8 项。
pub const MIRRORABLE_VIRTUAL_ITEMS: &[&str] = &[
    "{645FF040-5081-101B-9F08-00AA002F954E}", // 回收站（本机唯一显示）
    "{20D04FE0-3AEA-1069-A2D8-08002B30309D}", // 此电脑（HKLM 策略隐藏 → 不镜像）
    "{5399E694-6CE5-4D6C-8FCE-1D8870FDCBA0}", // 控制面板（HKLM 策略隐藏 → 不镜像）
    "{031E4825-7B94-4DC3-B131-E946B44C8DD5}", // 库（HKLM 策略隐藏 → 不镜像）
    "{B2B4A4D1-2754-4140-A2EB-9A76D9D7CDC6}", // Linux（HKLM 策略隐藏 → 不镜像）
    "{E88865EA-0E1C-4E20-9AA6-EDCD0212C87C}", // 图库（HKLM 策略隐藏 → 不镜像）
    "{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}", // 网络（HKLM 策略隐藏 → 不镜像）
    "{F874310E-B6B7-47DC-BC84-B9E6B38F5903}", // 主文件夹（HKLM 策略隐藏 → 不镜像）
];
```

判定 = `在表内 && !policy_hidden(…)`。本机 8 表内项里 7 项被策略否 → 只剩**回收站**1 项，
与 DefView 实测严格一致（§0.2.1 表 + 集合核对 §6.2）。

> 表里的 7 个「当前被策略隐藏」项**不是死代码**：换一台没被 HKLM 策略限制的机器（个人电脑常见），
> 此电脑 / 网络 / 库会真的显示在桌面上，本表让它们自动获得镜像能力，不需要发版。
> 这表是**能力声明**，注册表是**运行时裁决**。

**跨机器行为必须逐条想清楚（§8.5，原计划只写「本机被策略挡住了」是不够的）**：

| CLSID | 本机为何不镜像 | HKCU/HKLM 全缺值的机器上 | 备注 |
| :--- | :--- | :--- | :--- |
| 回收站 | **（本机就显示，被镜像）** | 显示、镜像 | Windows 默认桌面项，无争议 |
| 此电脑 / 库 / 网络 / 主文件夹 / 图库 / Linux / 控制面板小图标 | HKLM 策略 `1` | **会被准入并镜像** | 这些在多数个人机上确实默认显示；但**「缺值=准入」不等于「缺值=显示」**——准入只代表「允许镜像」，最终是否注册还取决于 `sync_virtual_items` 注册时枚举器/解析名能否成功（`virtual_item_handle` 失败即跳过） |
| `{59031a47-…}` 用户文件 | **不在表内**（本机枚举器未返回它） | 仍不镜像 | **已知缺口，如实记录**：它是常见 Windows 默认桌面项，但本机 `EnumObjects` 没返回它 ⇒ 表按「实测枚举」录入就会漏。若将来要覆盖，需扩表白名单并在实机验证 |

⇒ **「缺值=准入」（白名单）与「缺值≠显示」（`policy_hidden`）的差异由谁承担**：
由 `sync_virtual_items` 的注册步骤承担——准入项必须能成功 `SHParseDisplayName` 拿到 PIDL
且 `extract_icon` 成功才真正进栅栏；解析/抽图失败只记日志并跳过。
即「白名单说可以，Shell 说能不能，注册表说准不准」三层各自只回答一个问题。

**id 稳定性（避免同名撞车 + 旧配置自愈）**：实测存在两个「控制面板」同名项。id 的回退键
从显示名改为「小写显示名 + `-` + CLSID 小写前 8 位」，例如 `shell:控制面板-5399e694`、
`shell:回收站-645ff040`。这是 `item_id` 的有意变更（见 §4.2）。旧 `desk.json` 里的
`shell:控制面板` 旧 id 会被 `validate()` 当悬挂成员清掉，下次同步按新 id 重新注册 ⇒ **自愈**。

### 2.2 `winbosk-shell`：白名单准入 + 注册表否决 + PIDL 复刻（唯一碰 Win32 的地方）

```rust
// crates/shell/src/virtual_items.rs（新增文件）

/// 注册表读数（只读，`KEY_READ`）。键不存在 / 被策略锁 / 类型不符 → `PolicyValue::Unset`。
///
/// `hive`：`HKEY_CURRENT_USER` 或 `HKEY_LOCAL_MACHINE`。
/// 键路径：`Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\{NewStartPanel,ClassicStartMenu}`。
pub fn policy_value(hive: HKEY, clsid: &str) -> PolicyValue;

/// 当前应当镜像的虚拟项快照（**白名单准入 + 策略否决**，口径见 §0.2.1 / §2.1.1）。
///
/// 成本：**不调用 `IShellFolder::EnumObjects`**——只逐条对白名单里的 CLSID 做注册表读
/// （8 条 × 2 hive = 16 次 `RegGetValueW`，实测约 10~20 ms）。
/// 禁止在这里做全量枚举：实测 `enumerate_desktop_items()` 11.9~14.9 ms，且那 166 项里
/// 只有 9 个虚拟项，混进来纯属浪费（§0.4）。
///
/// **只在启动时调用一次**（以及用户在控制中心点「刷新」时）。4s 心跳禁止调用：
/// 16 次注册表读放进 4~7 ms 预算的心跳同样会超支，且注册表很少变。
pub fn mirrorable_virtual_snapshot() -> Vec<VirtualItemSnapshot>;

pub struct VirtualItemSnapshot {
    /// `::{XXXX-…}` 形式的解析名（`SHParseDisplayName` 可直接吃）。
    pub parsing_name: String,
    /// `{XXXX-…}` 形式，大写（与白名单同口径）。
    pub clsid: String,
    /// 系统显示名（`SHGetLocalizedName` / 已知名映射，仅用于日志与详情行）。
    pub display_name: String,
}

/// 一个已注册的虚拟项句柄（含 PIDL；`Drop` 时释放）。
pub struct VirtualItemHandle {
    pub pidl: *mut ITEMIDLIST,
}

impl Drop for VirtualItemHandle { /* CoTaskMemFree */ }

/// 用 parse name 重建 PIDL（打开 / 右键菜单 / 图标提取都走它）。
pub fn virtual_item_handle(parsing_name: &str) -> Option<VirtualItemHandle>;

/// `ShellExecuteEx` + `SEE_MASK_INVOKEIDLIST` + `lpIDList`：真实「打开」。
/// 回收站 → 打开回收站窗口；此电脑 → 资源管理器；控制面板 → 控制面板。
pub fn open_shell_item(pidl: *const ITEMIDLIST) -> bool;
```

**为什么 snapshot 不带 PIDL**：PIDL 是 COM 内存 + 长期存活所有权（进 `rt.items`），
让 snapshot 拿指针就等于让一个「应该按值比较的数据结构」持有裸指针。快照只用于
**集合比较与注册**（§2.3），真要用 PIDL 时按 `parsing_name` 现建。

**关键取舍**：`DesktopItem.pidl` 在 `icon_ids` 之外是**跨帧持有**的（现有 `rt.items` 就是长期持有
PIDL 的池），因此虚拟项的 `DesktopItem` 也进 `rt.items`（复用现有 `bitmap_ids` / `item_index` /
`pending_uploads` 事务，**不另造旁路结构**——旁路数据对齐律）。`launch()` 改为：

```rust
// crates/shell/src/items.rs —— DesktopItem::launch 改写
pub fn launch(&self) -> Result<(), String> {
    // 有路径 → 原 ShellExecuteW 路线；无路径（虚拟项）→ ShellExecuteEx + lpIDList
    match self.path.as_deref() {
        Some(p) => { /* 原实现 */ }
        None if !self.pidl.is_null() => shell_items::open_shell_item(self.pidl),
        None => Err("无路径且无 PIDL，无法打开".into()),
    }
}
```

### 2.3 镜像同步：虚拟项与文件镜像**彻底分离**（P0-1 修复，见 §8.1）

> **这条是硬约束，不是优化选项**：虚拟项集合比较**绝不能**挂进 `mirror_linked_fence`。
> 原因（`file_ops.rs:555-565` 实测口径）：`mirror_converged(owned, all, visible)` 的三个入参
> **全部派生自磁盘**，而 `owned` 来自 `mirror_existing_paths`，其第一句就是
> `.filter_map(|id| desk.icons.get(id).and_then(|ic| ic.path.as_deref()))` ——
> 这一句把**所有 `path == None` 的项整体丢弃**，虚拟项永远进不了 `owned`。
> ⇒ `visible ⊆ owned` 对虚拟项恒不成立 ⇒ `mirror_converged` 每 4s 返回 false
> ⇒ `file_ops.rs:554` 的早退失效 ⇒ 每轮都跑注册循环 + 桌面源孤儿分支的
> **全池 `Path::exists()`**（本机 173 条元数据 × 1 次 stat），
> 直接把计划自己的「空闲 0% CPU」铁律击穿（plan 08 正是靠这个早退挡掉这 5.2 ms 的）。

因此虚拟项同步**只**在两处发生，且都由 App 层**独立调用**（不经过 `reconcile_fences`）：

1. **启动一次**（`reconcile_fences` 之后、`build_scene` 之前）：
   `sync_virtual_items(&mut rt, &snapshot)`。注册/移除/清池一次性做完。
2. **用户显式点「刷新」**（控制中心按钮）：重读快照 → 再跑一次同一个函数。
   **不挂定时器、不挂 4s 心跳。**
   Windows 改了「显示此电脑」之后，用户重启或点一次刷新即生效——虚拟壳项的可见性
   本身是极低频操作，不值得为它每 4s 付一次注册表读或全池探测。

**`SyncLibrary` 4s 心跳完全不碰虚拟项**：`mirror_linked_fence` 的 `owned` / `visible` /
`all` 三个集合保持**纯磁盘口径**（一行都不改），虚拟项因 `path == None` 天然不在其中，
早退路径与 plan 08 完全一致。心跳增量 = **0**。

---

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

### 3.1 常驻后台与同步引擎

| 既有机制 | 交互 | 设计决策 |
| :--- | :--- | :--- |
| `SyncLibrary` 4s 心跳 | 虚拟项需不需要每 4s 重读注册表？ | **不需要**。注册表只在启动/用户点「刷新」时读（一次性约 10~20 ms，实测）。心跳只用内存快照做集合比较。若把注册表读放进心跳，`RegQueryValueExW` 每 4s × N 个 CLSID（HKCU+HKLM 两路）会新增 I/O 与系统调用，违反 0% CPU 铁律 |
| `enumerate_desktop_items()` | 能不能放进心跳？ | **不能**（0.4：11.9~14.9 ms > 心跳全预算）。虚拟项走 `mirrorable_virtual_snapshot()`，它只做注册表读 + CLSID 字符串比较，**不枚举 PIDL**（PIDL 只在注册/打开/菜单时才建） |
| 图标位图上传 | 虚拟项位图怎么进 GPU？ | 复用 `register_fence_item` 的 `pending_uploads` 事务（实测 9/9 成功：48px 提取消 0~5 ms / 项，回收站 0ms；**启动期用的是 128px，实测 169.9 ms / 9 项**，见 §3.2） |
| `prime_startup` 预热 | 虚拟项的 Shell 扩展要不要预热？ | **要**（实测回收站冷启 **191 ms**，首次右击会卡到 AppHang 阈值）。`prime_startup` 当前只收 `path` 列表，需扩展为支持解析名（`::{645FF040-…}`），键沿用解析名本身 |
| `IconGuard` | 虚拟项与真实图标隐藏冲突？ | 不冲突：`IconGuard` 只隐藏 `SysListView32`。虚拟项在栅栏里由 overlay 绘制；但**切到「恢复桌面」模式时，真实桌面的回收站会重新出现**（`RestoreIcons`），而栅栏里的那份是另一个绘制入口——此时不要重复或误判（见 §3.4） |

### 3.2 空闲性能约束

- 无新定时器、无新写盘：虚拟项注册只在启动做一次，且**有变化才 `store.save`**。
- 心跳快路径：收敛时只做内存集合比较，**零系统调用、零 I/O、零重绘**（沿用现有
  `redraw = changed` 口径）。
- 启动成本增量（**已实测，第一版写「≈27 ms」错约 6 倍，已纠正**）：虚拟项 9 个 ×
  `extract_icon(128)`（= 启动期 `ICON_EXTRACT_SIZE`，`main.rs:115`）= **169.9 ms**
  （单项 ≈ 19 ms，含 shell 冷缓存首次提取 + 重试退避；这是本机 166 项同批抽图的一部分）。
  另有解析名 20.3 µs + PIDL 重建 415.3 µs（9 项合计，可忽略）。
  **这条必须写进计划**：启动总时间会实打实多约 170 ms（一次性、冷缓存时）。
  若要压缩，只能考虑虚拟项按 `Icon.size` 的小尺寸抽图后由 GPU 放大——但那违反
  「GPU 始终向下采样」的注释约定（`main.rs:459-460`），**本轮不做，作为已知代价记录**。

### 3.3 孤儿与物理变更回流

| 外部变更 | 期望 | 实现 |
| :--- | :--- | :--- |
| 用户右键「显示/隐藏 此电脑」 | 重启后栅栏同步增减 | 启动时重读快照 → 集合差集 → 注册 / 移除 |
| 用户在资源管理器删除 / 移动某个桌面文件 | 已有逻辑不变 | 走原 `path.exists()` 分支（虚拟项天然豁免） |
| 用户「移出」虚拟项 | 不支持 + 提示 | `remove_fence_icon_by_id` 对虚拟项返回提示，不做 free_icons 回流 |
| 用户删「回收站」里的文件（Shell 动词） | 回收站图标**保留**（它恒存在） | 虚拟项不进 `path` 判定，同步不会回收它 |

### 3.4 边界与风险（明确记录，不掩盖）

1. **「恢复桌面」模式下出现两个回收站**：切回原生桌面时 `RestoreIcons` 会同时显示真实桌面的
   回收站与栅栏里那份。真实桌面图标与 overlay 是**互斥显示**的：现有 `toggle_desktop` 已把栅栏
   淡出至 `fence_alpha = 0` 且「不再接收命中」，因此**不会有两个可点的回收站**，只是原生桌面
   那一份确实在。这是既有语义（切回原生桌面本来就是让它全回来），本计划不改。
2. **CLSID 白名单的时效性**：Windows 版本/语言可能引入新的虚拟壳项（如未来的 Copilot 入口）。
   表外项一律不镜像（与「Windows 不显示」一致），最坏结果是「少镜像一个」，不会出现幽灵图标。
   这是**安全方向**的取舍：宁可少，不可错（多一个 = 用户看见桌面上本来没有的东西）。
3. **同名 id**：两个「控制面板」撞 `shell:控制面板`。§2.1 已把 id 改为
   `shell:<显示名>-<clsid 前 8 位>` 规避。**这是破坏性变更**：旧 `desk.json` 里若有
   `shell:控制面板` 成员，新口径下 id 不匹配 → `validate()` 会把它当悬挂成员清掉
   （见 §4.3 的迁移说明）。

---

## 4. 分层改动清单 (Implementation Steps)

### 4.1 `crates/core`（零 OS 依赖，纯函数 + 单测）

**`crates/core/src/shell_items.rs`（新建；`model.rs` 已 1711 行，抽出去更合适）**

- 新增 `PolicyValue` + `from_dword` + `policy_hidden`（签名与口径见 §2.1）。
  全部纯函数，**零 `#[cfg(windows)]`、零 windows 依赖**（这是 core 的硬约束）。
- 新增 `VIRTUAL_ID_PREFIX` + `is_virtual_id`（§4.1 末段）。
- 单测矩阵（`winbosk-core` 内瞬时完成）：

| 场景 | 断言 |
| :--- | :--- |
| HKCU 显式 `1` | `policy_hidden == true`（用户隐藏优先于一切） |
| HKCU 显式 `0` + HKLM `1` | `false`（用户翻盘，压过机器策略） |
| HKCU 缺 + HKLM `1` | `true`（机器策略隐藏，本机此电脑/网络/库/…） |
| HKCU 缺 + HKLM 缺 | **`false`**（不隐藏，但**也未见效为显示**——显示由 shell 侧白名单决定） |
| HKCU 缺 + HKLM 缺 + CLSID = 控制面板 `{26EE0668…}` | `false`，**且该 CLSID 不在白名单 ⇒ 最终不镜像**（§0.2.1 的关键回归） |
| DWORD 脏值（如 7） | 按 `Enabled`（不隐藏）且不 panic |
| `PolicyValue::from_dword` | `None→Unset`、`0→Enabled`、`1→Disabled`、`7→Enabled` |

**`crates/core/src/model.rs`**

- `Icon` 无新字段。
- `auto_organize_all` 与 `organize_icons_by_rules` 的候选收集处各加
  `if is_virtual_id(id) { continue; }`（§8.8-3：不能靠 `classify_icon` 返回 `None` 侥幸）。

**`crates/core/src/shell_items.rs`**（与 §2.1 同一文件）

- `pub const VIRTUAL_ID_PREFIX: &str = "shell:";` —— **id 前缀常量必须放在 core**：
  生成在 `shell`（`item_id`），消费在 `core`（`auto_organize_all` 候选集）与 `app`
  （`remove_fence_icon_by_id`）。放 shell 会让 core 反向依赖 shell，违反分层；三处各写一份
  字符串常量必然漂移。用前缀匹配而不是 `ic.path.is_none()` 判定「是不是虚拟项」：
  后者会把「拖进来的无路径项」一起误判（现在是死代码，但拖入管线随时可能产出）。
- `is_virtual_id(id: &str) -> bool` 小工具（`id.starts_with(VIRTUAL_ID_PREFIX)`），
  供 `core` / `app` 共用。

### 4.2 `crates/shell`（Win32 适配）

**`crates/shell/src/items.rs`**

- `DesktopItem::launch` 改写（§2.2）：保留 `path` 路线，新增 PIDL 路线。
  **直接返回 `Result<(), String>`，不留丢弃错误的薄包装**（§8.6）：全仓 `.launch()` 唯一
  调用点是 `main.rs:2273`（`launch_fence_icon`），「改 6 个调用点」的理由不存在；
  而 `open_folder` 的契约正是「调用方至少留一行日志」，薄包装会把失败路径重新变成静默。
  判据沿用 `open_folder` 的 `>32` 口径（`ShellExecuteEx` 的 `hInstApp`，非 `GetLastError`）。
- `item_id` 的虚拟项回退改为 `shell:<显示名>-<CLSID 前 8 位小写>`：
  需在 `build_item` 里拿到解析名（`SHGDN_FORPARSING`），从 `::{CLSID}` 里切出 CLSID。
  **成本已实测（第一版计划写成「约 1 ms」，错 1000 倍，已纠正）**：对 9 个虚拟项逐个调
  `GetDisplayNameOf(SHGDN_FORPARSING)`，三轮实测 **4.7 µs / 4.9 µs / 13.1 µs 总量**
  （即单项 0.5~1.5 µs）；同批 `SHGDN_NORMAL` 是 886.9 µs（显示名本地化更贵）。
  ⇒ 有路径项**不需要**额外调用（仍走 `SHGetPathFromIDListW`），虚拟项新增成本可忽略。
- 单测：`item_id` 新口径（两个「控制面板」id 不同；回收站 id 含 `645ff040`）；
  `kind_from` 不变（虚拟项仍是 `Unknown`，渲染层用 `path == None` 区分）。

**`crates/shell/src/virtual_items.rs`（新增）**

- `policy_value` / `mirrorable_virtual_snapshot` / `virtual_item_handle` / `open_shell_item`
  （签名见 §2.2）。
- `open_shell_item` 用 `ShellExecuteExW` + `SEE_MASK_INVOKEIDLIST (0xC)` + `lpIDList`，
  `SE_ERR_*` 口径记日志（与 `open_folder` 同一「静默失败不可接受」原则）。
- `SHParseDisplayName` 解析 `::{CLSID}` → PIDL（`items.rs:135` 已有同款调用可复用）。

**`crates/shell/src/lib.rs`**：`pub mod virtual_items;` + 文档注释（分层说明它为什么在 shell）。

### 4.3 `crates/app`（组装与交互）

**`crates/app/src/file_ops.rs`**

- 新增 `sync_virtual_items(rt, &snapshot) -> bool`（**App 层独立入口，不经过
  `reconcile_fences`**，理由见 §2.3）：做三件事——
  1. **注册**：快照里未被任一栅栏持有的项，`register_virtual_item`。
  2. **移除**：栅栏成员里 `is_virtual_id` 为真但**不在快照**里的项（用户把「此电脑」关了）
     → 从栅栏成员移除（**不删任何磁盘内容**，它们本来就不在磁盘上）。
  3. **清池**：池里 `path == None && is_virtual_id(id) && id 不在快照` 的条目整体删除
     （含 `rt.items` 对应项，让 `DesktopItem::Drop` 释放 PIDL）——§8.2：本机实测已有
     **15 条**永不消失的 `shell:` 死元数据（含 6 条无 CLSID 的英文 locale 名），
     **必须按差集兜底，不能按 CLSID 反查**。
- 新增 `register_virtual_item(rt, fence_idx, &VirtualItemSnapshot) -> bool`：
  建 `DesktopItem`（`kind = Unknown`、`path = None`、`id = shell:…-<clsid8>`）→ 走
  `register_fence_item` 的既有事务（`items` / `item_index` / `bitmap_ids` /
  `pending_uploads`）→ 追加成员。**位图槽仍走 `max()+1` 单调递增**（`file_ops.rs:96`）。
  **硬约束（§8.3）**：`Icon.path` 必须显式为 `None`，**禁止**从 `parsing_name` 派生
  `Some("::{645FF040-…}")` 假路径。理由：`movable_items`（`file_ops.rs:215-238`）会把
  `added && is_inside_library` 的项当文件搬；`classify_icon` / `matches_icon` 侧
  `file_extension("::{645FF040-…}")` 会切出假扩展名；`is_managed_path` 恒 false 这条
  Non-Goal 也依赖 `path == None`。
- **`mirror_linked_fence` 与 `mirror_converged` / `mirror_existing_paths` 一行都不改**
  （§2.3 / §8.1）：三个集合保持纯磁盘口径，虚拟项因 `path == None` 天然不在其中，
  plan 08 的早退路径与 0% CPU 铁律完整保留。
- `remove_fence_icon_by_id`：命中 `is_virtual_id(id)` → 走提示分支（`Non-goals#3`），
  不回流 free_icons、不删任何东西。**判据用前缀常量，不要用 `path.is_none()`**（§4.1）。
- 单测（`file_ops.rs::tests`，纯内存）：

| 场景 | 期望 |
| :--- | :--- |
| 虚拟项已注册 → 连续两次 `sync_virtual_items` | 第二次 0 变动（幂等） |
| 虚拟项被外部「移出」→ 再次同步 | 被加回（与 plan 08「移出无效果」同口径） |
| **心跳不碰虚拟项**：`mirror_linked_fence` 收敛态 + 已注册虚拟项 | `mirror_converged == true`，早退生效，**不进**注册循环（§8.1 的回归锁） |
| 虚拟项被用户改注册表隐藏 → 快照变化 | 从栅栏移除（**且不删任何磁盘内容**） |
| 桌面文件被删 + 虚拟项在场 | 文件项回收，虚拟项保留（豁免） |
| **池孤儿清理**：池里有 `shell:控制面板`（旧口径）+ `shell:This PC`（英文 locale）+ 新口径注册 `shell:控制面板-5399e694` | 三个旧键全部从池 / `items` / `bitmap_ids` 消失，新键就位（§8.2；英文名无 CLSID 也能清） |
| **虚拟项「移出」被拒** | `remove_fence_icon_by_id(shell:…)` 不进 `free_icons`、不删池、栅栏成员不变 |
| **`Icon.path` 契约** | 已注册虚拟项 `ic.path.is_none() == true`；`is_managed_path(rt, Path::new("::{645FF040-…}")) == false`（§8.3） |

`crates/core/src/shell_items.rs::tests` 与 `model.rs::tests`（新增）：

| 场景 | 期望 |
| :--- | :--- |
| `is_virtual_id` | `shell:回收站-645ff040` → true；`c:\...\a.txt` → false；**无路径但非虚拟 id → false** |
| `auto_organize_all` 候选含虚拟项（桌面栅栏成员） | 虚拟项**不在**候选中（`moved_icons` 不含它，仍留原栅栏） |
| `organize_icons_by_rules` 候选含虚拟项 | 同上（两个入口都要覆盖，§8.8-3） |
| 用户栅栏 `name_patterns = ["回收站"]` + 虚拟项在桌面栅栏 | 虚拟项**不被**搬走（规则匹配不看 kind，只匹配文本 ⇒ 必须靠候选集排除，不能靠规则不匹配） |
| `delete_fence_and_reclaim_icons` 归流 | 虚拟项不在归流列表里 |

**`crates/app/src/main.rs`**

- 启动尾（`reconcile_fences` 之后、`build_scene` 之前）：
  `let snap = mirrorable_virtual_snapshot(); sync_virtual_items(&mut rt, &snap);`
  **必须是独立调用，不能并进 `reconcile_fences`**（§2.3 / §8.1）。顺序在首帧布局**之前**：
  虚拟项要参与初始布局（否则首帧后侧边栏宽度突变 → 跳一下）。
- 控制中心「刷新虚拟项」按钮（新增入口）：重读快照 → 再跑一次 `sync_virtual_items`。
  **不挂定时器、不挂 `SyncLibrary`**：Windows 改「显示此电脑」后重启或点一次即生效。
- `execute_auto_organize` 本身**不改**；改的是它调用的两个 core 函数（见 §4.1 的插入点）：
  `auto_organize_all`（`model.rs:990-1007` 候选收集处）与 `organize_icons_by_rules`
  （同文件候选收集处）各加一行 `if is_virtual_id(id) { continue; }`。
  **不要**改成在 `execute_auto_organize` 里过滤候选——那会漏掉控制中心 / 栅栏右键菜单以外
  的其它入口（`organize_icons_by_rules` 也被规则收纳流程直接调用）。
- `delete_fence_and_reclaim_icons`：归流时跳过虚拟项（它们**只**属于桌面栅栏，
  分类栅栏被删时不该把虚拟项也搬进来——不过按上一条它们根本进不了分类栅栏，
  这里是双保险，写成断言测试）。
- `prime_startup` **增加独立形参**（不是把解析名塞进 `paths`）：解析名走**独立类型键族**
  `virtual:{clsid 小写}`（§8.7）。现有 `prime_startup(paths) → unique_type_keys(paths)` 从
  **路径扩展名**推键，`::{645FF040-…}` 会被切出 `00aa002f954e}` 当「扩展名」，
  与 `show()` 侧 `ensure_primed` 对真实路径推的键**不同口径** ⇒ 预热静默失效，
  而这项改动正是为修回收站冷启 191 ms。`show_virtual` 侧用同一个 `virtual:{clsid}` 键。

**`crates/app/src/context_menu.rs`**

- `handle_context_menu` 的 `None` 分支（无路径项）：现在有了真实菜单路线，改为
  `shell_menu::show_virtual(rt, &parsing_name, sx, sy)`（解析名路线实测 4/4 可用）。
  `managed` 对虚拟项恒 `false`（`is_managed_path` 不含它）→ 菜单注入「移出栅栏」；
  按 §Non-goals#3，「移出」实际被拒并在**同一次交互内**给出提示（不做二次确认框）。

### 4.4 渲染层

**不动**（`draw.rs` / `overlay.rs` / `scene.rs` 零改动）。虚拟项复用 `Icon{path:None}` 的现有
绘制路径；双击/右键命中沿用现有 `IconClicked` / `ContextMenu` 事件，只是事件处理函数内部
多两条分支（打开走 PIDL、菜单走解析名）。

---

## 5. 防御性自查清单 (Defensive Invariants)

| 校验律 | 针对性设计 |
| :--- | :--- |
| **常驻后台冲突律** | 注册表读只在启动 + 显式刷新；**虚拟项同步完全不进 4s 心跳**（§2.3/§8.1：`mirror_linked_fence` 三个集合保持纯磁盘口径，一行不改），早退路径与 plan 08 一致，心跳增量 = 0 |
| **空闲 0% CPU 律** | 沿用 `mirror_converged` 两条子集断言；**禁止**改集合相等（隐藏/不可见项会让相等永假 → 每 4s 白跑）；**禁止**把虚拟项塞进 `owned`/`visible`/`all`（那会让 `path==None` 项破坏收敛 → 每轮全池 `exists()`，实测 173 次 stat） |
| **孤儿清理律** | 虚拟项 `path == None` 天然豁免 `exists()` 回收；且因 §8.1 的分离设计，该分支不再被虚拟项每 4s 触发。补断言测试锁住两条：`mirror_converged` 对含虚拟项的 Desk 仍为 true；桌面源孤儿分支不误删虚拟项 |
| **语义精准定位律** | 桌面栅栏定位沿用 `resolve_desktop_fence`（title == "桌面" 或 storage == 桌面目录），禁止硬编码下标；虚拟项注册目标 = 该函数解析出的桌面栅栏 |
| **状态全集校验律** | 虚拟项只会出现在桌面栅栏（`auto_organize_all` 与 `organize_icons_by_rules` 候选集用 `is_virtual_id` 显式排除，**不靠** `classify_icon` 返回 `None` 侥幸，见 §8.8-3）；不出现「既在分类栅栏又在桌面栅栏」 |
| **旁路数据对齐律** | 复用 `register_fence_item` 事务：`items` / `item_index` / `bitmap_ids` / `pending_uploads` 同帧对齐，位图槽单调递增；不新建旁路结构 |
| **幂等律** | 连续两次同步，第二次 0 变动 0 创建 0 移动 |
| **空集合律** | 注册表键全不存在（全新账户）→ 快照为空 → 一个虚拟项都不注册，不 panic、不写盘 |
| **零孤儿律** | 口径变更 / locale 差异导致的旧 id 从 `desk.icons` / `rt.items` / `bitmap_ids` 一并清掉，按「快照差集」兜底而非按 CLSID 反查（§8.2：实测已有 15 条死元数据，其中 6 条无 CLSID） |

附加防线：

- **CLSID 大小写/格式**：注册表读出的 GUID 大小写不统一（实测 HKLM 里 `{e88865ea-…}` 小写、
  `{E88865EA-…}` 大写并存）→ 统一 `to_ascii_uppercase()` 后比较。
- **同名 id 撞车**：`shell:<显示名>-<clsid8>`（§2.1.1）；对旧 `desk.json` 的 `shell:控制面板`
  旧 id，`validate()`（`model.rs:897-902`，加载时必调：`main.rs:422` / `config.rs:122`）会清掉该
  悬挂成员，下一次同步按新 id 重新注册 → **自愈，不丢图标**。
- **显示名带点的本地化名**：`file_extension` 会从显示名切出假扩展名（实测 `我的.文档` → `文档`、
  `v1.2.3` → `3`）⇒ **绝不能**用「有没有扩展名」推断虚拟项，一律 `is_virtual_id`。
- **解析名解析失败**：`virtual_item_handle` 返回 `None` → 该项跳过（打开/菜单给出日志），
  不让一个坏 PIDL 阻塞整个同步。
- **PIDL 生命周期**：虚拟项 `DesktopItem` 进 `rt.items`，由既有 `Drop` 释放
  （`items.rs:42-45`），不新增所有权路径。
- **启动变慢是已知代价**：128px 抽图 9 项 ≈ 170 ms 一次性（§3.2 实测），不为此改小图标尺寸。

---

## 6. 验证与交付门禁 (Verification Gates)

### 6.1 自动化

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings        # 本机需临时门控 winres，跑完还原
cargo test --workspace
```

新增用例（分层放置，全部内存级 + CI 可跑）：

| 层 | 用例 |
| :--- | :--- |
| `winbosk-core` | §4.1 的 8 行矩阵（HKCU/HKLM 组合 + 脏 DWORD + `from_dword`）；`is_virtual_id`；`auto_organize_all` / `organize_icons_by_rules` 候选排除虚拟项（含「用户 `name_patterns` 命中虚拟项」这条硬回归，§8.8-3）；`delete_fence_and_reclaim_icons` 归流不含虚拟项 |
| `winbosk-shell` | `item_id` 新口径（两个控制面板 id 不同、回收站 id 含 `645ff040`）；`kind_from` 对虚拟项仍返回 `Unknown`；D2D 无关的纯逻辑 |
| `winbosk-app` | §4.3 的 8 行矩阵（幂等 / 移出加回 / **心跳早退仍成立** / 隐藏后移除 / 孤儿豁免 / 池孤儿差集清理 / 移出被拒 / `Icon.path` 契约） |

### 6.2 真实走查（debug 构建 + `WINBOSK_AUTOSTOP_MS`）

| 步骤 | 期望 |
| :--- | :--- |
| 修复前 `desk.json` | 桌面栅栏 `icon_ids` = 文件项数；缺回收站（对照 ground truth 150 项差 1） |
| 修复后首次启动 | 桌面栅栏含 **回收站**（新增 1 项），总数与 DefView 150 对齐 |
| 双击回收站 | 真实回收站窗口弹出（`ShellExecuteEx` + PIDL） |
| 右键回收站 | 真实 Shell 菜单（打开/固定/属性/清空回收站…），冷启不卡（已预热） |
| 一键整理 | 回收站**留在【桌面】栅栏**，不被搬进任何分类栅栏 |
| 删除「常用应用」分类栅栏 | 图标回流【桌面】栅栏；回收站不动 |
| **池孤儿清理** | 运行前 `icons` 有 15 条 `shell:` 死元数据（实测）→ 运行后 `icons` 里的 `shell:` 只含当前快照项（本机 1 条），无残留 |
| **空闲 10s（0% CPU 回归）** | 心跳 1 次/4s、无 `read_dir` 之外的 I/O、`desk.json` 不再被反复写（`store.save` 只在 `changed` 时）；本机 173 条元数据不得出现每 4s 全量 `exists()`（§8.1 的实机回归） |
| 连续两次运行 | `desk.json` 语义 deep-equal（幂等） |

### 6.3 交付前必须回报的「没跑/为什么」

按 AGENTS.md 要求，若某步受环境限制没跑，**必须显式写出**。本机已知限制：`reg query` 可用、
`desktop-probe.py` 可用；`cargo build --release` 与 winres 图标嵌入需按 plan 08 的既有手法
（临时门控 `crates/app/build.rs`，跑完还原并确认 `git diff` 为空）。

---

## 7. 实施完成后（必做）

按 AGENTS.md「实施计划审查标准」：**派发独立干净上下文的子代理执行对抗性代码审查**
（重点：并发安全、后台循环交互、心跳成本、孤儿豁免被破坏、CLSID 口径回归、id 变更对旧配置的
兼容自愈）。审查结论回填到本文件 §8。

---

## 8. 独立子代理对抗性审查结论（已执行，结论回填）

独立干净上下文子代理（`reviewer`）完成对抗性走查，独立复核了 166/150/157/9 与双 hive 注册表
（全部与计划一致），结论 **P0 ×2 / P1 ×2 / P2 ×3，需修改后落地**。全部已修，逐条如下。

### 8.1 P0-1：虚拟项同步挂进 4s 心跳会击穿「空闲 0% CPU」→ 已修（§2.3 重写）

**问题**：原 §2.3 把「注册虚拟项」插在 `mirror_linked_fence` 的注册分支里。但
`mirror_converged(owned, all, visible)` 的三个入参**全部派生自磁盘**，而 `owned`
来自 `mirror_existing_paths`（`file_ops.rs:678-700`），其第一句
`.filter_map(|id| desk.icons.get(id).and_then(|ic| ic.path.as_deref()))` 把**所有
`path == None` 的项整体丢弃** ⇒ 虚拟项永远进不了 `owned` ⇒ `visible ⊆ owned` 恒不成立
⇒ `mirror_converged` 每 4s 返回 false ⇒ `file_ops.rs:554` 早退失效 ⇒ 每轮跑注册循环 +
桌面源孤儿分支的**全池 `Path::exists()`**（本机 173 条 × 1 stat）。plan 08 正是靠这个早退
挡掉 5.2 ms/轮。

**修法**：虚拟项同步与文件镜像**彻底分离**——只在「启动一次 + 用户点刷新」时跑
`sync_virtual_items()`，4s 心跳**一行都不碰**（§2.3）。`mirror_linked_fence` 的三个集合
保持纯磁盘口径，早退路径与 plan 08 完全一致，心跳增量 = 0。
（备选方案「让 `mirror_existing_paths` 按虚拟项 id 纳入 `owned`」被否：那要把一个
「回答磁盘归属」的纯函数改成「同时回答虚拟壳项归属」，把两套生命周期不同的东西耦进一个
集合，将来任何一方口径变化都会以「快路径静默失效」的形式爆掉——正是 plan 08 P1 的同款教训。）

### 8.2 P0-2：运行时已存在 15 条 `shell:` 池孤儿，「自愈」论证与实际数据矛盾 → 已修

**问题**：实测 `target/debug/data/desk.json`：`icons` 共 **173** 条，其中 **15 条 `shell:`**
（9 条中文名 + **6 条英文 locale 名**：`shell:This PC` / `shell:Recycle Bin` /
`shell:UsersLibraries` / `shell:Control Panel` / `shell:Computers and Devices` /
`shell:Control Panel command object for Start menu and desktop`），而 `fences[].icon_ids` 与
`free_icons` 里**没有任何** `shell:` 成员（桌面栅栏只有 2 个 `.reg`）。
⇒ 这 15 条是**永不消失的死元数据**：`Desk::validate()` 只 retain 成员引用、**不清池**；
`reconcile_library` 只清 `added && inside_library && !exists`（虚拟项 `added=false`、
`path=None`，永不被清）；启动期 `icons.entry().or_insert_with()` 只加不删。
更糟的是那 6 条英文名**不含 CLSID**，原 §8.0-2 提议的「按同一 CLSID 反查旧 id」对它们
完全无效 ⇒ 原「自愈不丢图标」的论证与实际数据矛盾。

**修法**（§4.3 已改）：池清理**不按 CLSID 反查**，改为**按快照差集兜底**——
注册新 id 之前，把池里满足「`path == None` && `is_virtual_id(id)` && id 不在当前快照里」
的条目**整体删除**（含 `rt.items` 中对应项，让 `DesktopItem::Drop` 释放 PIDL）。
中文/英文/任何 locale 的死名全部被这一条覆盖，不需要解析名字。
**注意**：本机这 15 条**当前都不是栅栏成员**（无渲染入口），所以删除它们不丢任何图标；
判据必须同时要求「不在快照里」才不会误删刚注册的虚拟项。

### 8.3 P1-1：`is_managed_path` 对虚拟项恒 false 的前提没写成契约 → 已修

`Non-Goals#2` 声称虚拟项不在 WinBosk 管理区，但其成立依赖一个**隐含前提**：
`Icon.path` 必须是 `None`。若实现时照抄 `main.rs:511-521` 的 `ic.path = it.path.clone()`
或从解析名派生假路径，`Icon.path` 会变成 `Some("::{645FF040-…}")`。
核对现有代码：`is_linked_path`（`path_within`）与 `delete_managed_file`（`pp.exists()`）
对 `::{…}` 都返 false，故这两条仍安全；但 `movable_items`（`file_ops.rs:215-238`）会把
`added && is_inside_library` 的项当文件搬，`classify_icon` / `matches_icon` 侧
`file_extension("::{645FF040-…}")` 也会切出假扩展名。
**修法**：§4.3 补硬约束 + 两条断言测试（已写入 §4.3 单测表）。

### 8.4 P1-2：孤儿豁免的代码位置描述有误（豁免生效，但成本论述错）→ 已修

原 §2.3 说「现有代码已经豁免了它们」。豁免本身成立，但**位置描述错**：桌面源孤儿分支
（`file_ops.rs:565-578`）只在 `mirror_converged == false` 之后才执行。按 P0-1 的分析，
虚拟项会让它**每 4s 必到**，即「豁免生效却付出了本该由早退节省的成本」，且将来有人
「顺手补一个 unwrap」时改的是每 4s 必到的路径（风险面比原描述大得多）。
**修法**：随 P0-1 一并解决（虚拟项不再参与该函数的收敛判定）。

### 8.5 P2-1：白名单按「本机实测」收录，换机器时依赖未量化的假设 → 已修

原表 8 条准入项里 7 条的注释依据是「本机 HKLM 策略隐藏 → 不镜像」，即准入依据是
「本机恰好被策略挡住了」而非「Windows 默认显示它」。换一台 HKCU/HKLM 全缺值的机器，
这 7 项会被无条件准入，而它们是否显示取决于 Windows 默认注册状态——这与 §2.1 自己写的
「缺值 ≠ 显示」原则冲突（该原则只在 `policy_hidden` 侧被遵守，白名单准入侧被绕过）。
旁证：`{59031a47-3F72-44A7-89C5-5595FE6B30EE}`（用户文件）是常见 Windows 默认桌面项，
却不在本机枚举的 9 项里 ⇒ 「枚举器返回」与「Windows 默认桌面集」确是两个集合。
**修法**：§2.1.1 白名单逐条补「跨机器行为」标注，并明确「缺值=准入」与「缺值≠显示」
的差异由谁承担（见 §2.1.1 的修订表）。

### 8.6 P2-2：`launch()` 薄包装与自身「静默失败不可接受」契约矛盾 → 已修

原 §4.2 要求「把『点了没反应』变成一行日志」，又要求保留丢弃错误的薄包装 —— 自相矛盾。
且「避免一次改 6 个调用点」与代码不符：全仓 `.launch()` **唯一**调用点是
`main.rs:2273`（`launch_fence_icon` 内）。
**修法**：直接让 `launch()` 返回 `Result<(), String>`，调用点按结果记 `warn!`，不留薄包装。

### 8.7 P2-3：`prime_startup` 扩展接受解析名会与现有类型键口径冲突 → 已修

现有 `prime_startup(paths)` → `unique_type_keys(paths)` 从**路径扩展名**推导类型键
（`shell_menu.rs:131`、`type_key`）。把 `::{645FF040-…}` 塞进同一个 `Vec<String>`，
`unique_type_keys` 会切出 `00aa002f954e}` 当「扩展名」，与 `show()` 侧 `ensure_primed` 对真实
路径推导的键**不同口径** ⇒ 预热静默失效，而这项改动的初衷正是修回收站冷启 191 ms。
**修法**：解析名走**独立类型键族**（`virtual:{clsid}` 小写），不混入按扩展名推导的键空间。

### 8.8 主代理自查补充（审查回填前已记录，仍有效）

1. **`validate()` 自愈路径已核实存在**：`Desk::validate()`（`crates/core/src/model.rs:897-902`）
   只保留 `icons` 里有的成员，且**加载时必调**（`crates/app/src/main.rs:422`、
   `crates/core/src/config.rs:122`）。成员引用侧的自愈成立；但**池侧**要清孤儿必须靠
   §8.2 的差集兜底（`validate()` 不清池）。
2. **「虚拟项不会被一键整理搬走」不能只靠 `classify_icon` 返回 `None`（P1，已列为必须实现的显式排除）**：
   `auto_organize_all` 的候选集（`model.rs:990-1007`）= `free_icons` + 桌面栅栏里**所有不匹配
   其自身 rule** 的成员 ⇒ 镜像进来的回收站天然是候选。实测 `classify_icon` 对本机 8 个虚拟名
   全返回 `None`（显示名不含点 ⇒ `file_extension` 返回 `None`，且 `kind` 是 `Unknown` 不是 `Doc`），
   但这是**巧合不是契约**：显示名带点的本地化名（`我的.文档` → `文档`、`v1.2.3` → `3`）会切出
   假扩展名；用户自建栅栏的 `name_patterns` / `custom_extensions` 也可直接命中虚拟项
   （规则匹配不看 kind，只匹配文本）⇒ 必须在候选集用 `is_virtual_id` 显式排除（§4.1）。
3. **`SHGDN_FORPARSING` 成本第一版写错 1000 倍**：实测 9 项合计 4.7~13.1 µs（单项 0.5~1.5 µs），
   不是「约 1 ms」。已纠正（§4.2）。
4. **启动抽图成本第一版低估约 6 倍**：启动期 `ICON_EXTRACT_SIZE = 128`
   （`main.rs:115`），9 个虚拟项实测 **169.9 ms**（单项 ≈19 ms，含 shell 冷缓存 + 重试退避）。
   已作为已知代价记录（§3.2），不为此改小图标尺寸（违反 `main.rs:459-460` 的
   「GPU 始终向下采样」约定）。

