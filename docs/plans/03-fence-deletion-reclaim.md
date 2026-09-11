# 分类栅栏删除与图标安全归流实施计划

本实施计划覆盖 `docs/TODO.md` 中的 **Task #4（删除分类栅栏图标自动回归【桌面】栅栏）**。

---

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals
1. **图标生命周期全闭环**：彻底杜绝“删除分类栅栏导致图标退入不可见 `free_icons` 黑洞”的问题；删除分类栅栏等价于“撤销该分类”，内部图标无损回归基础【桌面】栅栏；
2. **语义化靶向归流**：依据栅栏语义（优先标题为「桌面」、或关联桌面物理目录、或无规则基础栅栏）精准匹配受纳栅栏，杜绝盲目下标硬编码；
3. **零栅栏兜底保护**：若用户删除了桌面上最后一个栅栏，系统自动重置生成默认的基础「桌面」栅栏，确保图标始终可见；
4. **旁路与持久化对齐**：删除栅栏时，原子化同步清理 `rt.last_layout_h`、修正 `rt.selected_fence`，并立即持久化 `desk.json`；
5. **统一删除行为通道**：将控制中心内的「删除栅栏」（`ConsoleZone::RemoveFence`）与栅栏右键菜单的「删除栅栏」（`FenceMenuAction::Delete`）收拢至同一个安全删除函数。

### Non-Goals
1. 不变动磁盘物理文件（删除分类栅栏只是改变逻辑归属，不删除物理文件，`Icon.path` 保持不变）；
2. 不篡改用户已配置的其它栅栏的分类规则。

---

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 语义定位目标栅栏函数
在 [`crates/app/src/main.rs`](file:///g:/Codes/sylva/crates/app/src/main.rs) 中实现语义定位：
```rust
/// 在桌面上寻找用于受纳回流图标的目标「桌面」栅栏
pub(crate) fn resolve_desktop_fence(desk: &Desk, exclude_id: Option<u64>) -> Option<u64> {
    // 1. 语义优先：标题为「桌面」且非排除项
    if let Some(f) = desk.fences.iter().find(|f| {
        exclude_id != Some(f.id) && f.title.as_deref() == Some("桌面")
    }) {
        return Some(f.id);
    }
    // 2. 规则优先：未配置分类规则且未绑定外部目录的通用栅栏
    if let Some(f) = desk.fences.iter().find(|f| {
        exclude_id != Some(f.id) && f.rule.is_none() && f.storage_path.is_none()
    }) {
        return Some(f.id);
    }
    // 3. 兜底：任意其他存活的栅栏
    desk.fences.iter().find(|f| exclude_id != Some(f.id)).map(|f| f.id)
}
```

### 2.2 原子删除与图标归流函数
```rust
/// 安全删除指定索引的栅栏，并将其中的图标完整归流至「桌面」栅栏
pub(crate) fn delete_fence_and_reclaim_icons(rt: &mut Runtime, fence_idx: usize) {
    let Some(fence) = rt.desk.fences.get(fence_idx) else {
        return;
    };
    let deleting_id = fence.id;
    let icon_ids = fence.icon_ids.clone();

    // 1. 寻找或兜底创建受纳栅栏
    let target_fid = match resolve_desktop_fence(&rt.desk, Some(deleting_id)) {
        Some(fid) => fid,
        None => {
            // 桌面上所有栅栏均被删除，自动重置出一个标准默认「桌面」栅栏
            let wa = work_area_rect(rt.origin.0, rt.origin.1, rt.vw, rt.vh);
            let fallback_id = rt.desk.next_fence_id();
            let fallback = Fence {
                id: fallback_id,
                title: Some("桌面".to_string()),
                monitor_id: 0,
                bounds: Rect::new(wa.x + 40.0, wa.y + 40.0, 320.0, 0.0),
                state: FenceState::Normal,
                icon_ids: Vec::new(),
                appearance: FenceAppearance::default(),
                scroll: 0.0,
                storage_path: None,
                sidebar_collapsed: false,
                collapsed: false,
                rule: None,
            };
            rt.desk.fences.push(fallback);
            fallback_id
        }
    };

    // 2. 将被删除栅栏内的图标无损移入目标栅栏
    for id in icon_ids {
        rt.desk.move_icon(&id, Some(target_fid));
    }

    // 3. 物理移除被删栅栏
    if fence_idx < rt.desk.fences.len() {
        rt.desk.fences.remove(fence_idx);
    }

    // 4. 旁路缓存对齐（Sidecar Invariant）
    if fence_idx < rt.last_layout_h.len() {
        rt.last_layout_h.remove(fence_idx);
    }

    // 5. 选中状态与滚动修正
    rt.selected_fence = rt
        .selected_fence
        .saturating_sub(1)
        .min(rt.desk.fences.len().saturating_sub(1));

    // 6. 立即持久化
    if let Err(e) = rt.store.save(&rt.desk) {
        tracing::warn!("删除栅栏持久化失败: {e}");
    }
}
```

---

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

1. **孤儿状态回收律 (Orphan Invariant)**：
   - 过去调用 `rt.desk.move_icon(&id, None)` 会将图标放进 `free_icons`；
   - 但因为 Sylva 接管桌面隐藏了 Explorer 的真实 `SysListView32`，导致进入 `free_icons` 的文件直接从视觉上“消失”；
   - 本方案强制把图标全部回流至 `IconLocation::Fence(desktop_fid)`，保证 100% 始终在某个可见栅栏中渲染，彻底杜绝孤儿项。
2. **旁路数据对齐律 (Sidecar Invariant)**：
   - `rt.last_layout_h` 是按 `fence_idx` 下标与 `rt.desk.fences` 严格一一对应的旁路高度缓存；
   - 过去在删除栅栏时遗漏了 `rt.last_layout_h.remove(fence_idx)`，导致数组长度失配和下标越界隐患；
   - 现将其收拢到 `delete_fence_and_reclaim_icons` 中执行同步 `remove`，确保主从数据结构时刻严格同构。
3. **常驻后台冲突律 (Daemon Invariant)**：
   - 图标转移仅更新内存中的 `IconLocation`，未变动物理路径 `Icon.path`，后台 4s 一次的 `SyncLibrary` 库同步不会发生误判或误删。

---

## 4. 分层改动清单 (Implementation Steps)

### 一、应用组装层（`crates/app/`）
- **[`crates/app/src/main.rs`](file:///g:/Codes/sylva/crates/app/src/main.rs)**：
  - 实现 `resolve_desktop_fence` 与 `delete_fence_and_reclaim_icons`；
  - 重构 `ConsoleZone::RemoveFence` 分支，直接调用 `delete_fence_and_reclaim_icons(rt, i)`；
- **[`crates/app/src/context_menu.rs`](file:///g:/Codes/sylva/crates/app/src/context_menu.rs)**：
  - 重构 `FenceMenuAction::Delete` 分支，直接调用 `delete_fence_and_reclaim_icons(rt, fence)`，彻底消除冗余重复代码。

### 二、测试与验证（`crates/app/` 或 `crates/core/`）
- 编写单元测试覆盖以下核心场景：
  - 存在「桌面」栅栏和其他分类栅栏，删除分类栅栏后，图标全部移入「桌面」栅栏；
  - 不存在「桌面」栅栏但存在无规则栅栏，删除分类栅栏后，图标移入该无规则栅栏；
  - 仅剩一个栅栏被删除时，自动生成回退「桌面」栅栏，图标完整转移，`free_icons` 保持为空。

---

## 5. 防御性自查清单 (Defensive Invariants)

- [x] **语义精准定位律**：优先按标题「桌面」或规则为空语义查找，禁止简单粗暴假定 `fences[0]` 为桌面。
- [x] **孤儿状态回收律**：兜底自动创建默认桌面栅栏，禁止任何情况下向 `free_icons` 泄漏图标。
- [x] **旁路数据对齐律**：删除栅栏时，`rt.desk.fences` 与 `rt.last_layout_h` 在同一个代码块内原子同步移除。

---

## 6. 验证与交付门禁 (Verification Gates)

1. **测试用例**：
   ```powershell
   cargo test --workspace
   ```
2. **代码质量**：
   ```powershell
   cargo clippy --workspace -- -D warnings
   cargo fmt --all -- --check
   ```
3. **真实走查验证**：
   - 桌面放置各类文件，点击「⚡ 一键整理桌面」，生成「常用应用」、「办公文档」等分类栅栏；
   - 在控制中心选中「常用应用」栅栏，点击「删除栅栏」；
   - 验证被删栅栏内的图标全部即刻出现在「桌面」基础栅栏中，无一丢失；
   - 在「办公文档」栅栏空白处右键，点击「删除栅栏」；
   - 验证文档图标同样完整无损地回归「桌面」栅栏；
   - 重启程序，验证 `desk.json` 数据与图标展示完全正常。
