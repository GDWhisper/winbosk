//! 文件操作：添加路径、库同步、链接文件夹镜像、库内复制、剪贴板文件读取。

use std::collections::HashSet;
use std::os::windows::fs::MetadataExt;

use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM};

use crate::*;
pub(crate) fn new_path_for_rename(old_path: &str, new_name: &str) -> Option<PathBuf> {
    let p = Path::new(old_path);
    let parent = p.parent()?;
    let old_file = p.file_name()?.to_string_lossy().into_owned();
    let lower = old_file.to_ascii_lowercase();
    let stripped_ext = ["lnk", "url", "appref-ms"].iter().copied().find(|ext| {
        let dot = format!(".{ext}");
        lower.ends_with(&dot) && old_file.len() > ext.len() + 1
    });
    let final_name = match stripped_ext {
        Some(ext) if !new_name.to_ascii_lowercase().ends_with(&format!(".{ext}")) => {
            format!("{new_name}.{ext}")
        }
        _ => new_name.to_string(),
    };
    if final_name.is_empty() {
        return None;
    }
    Some(parent.join(final_name))
}

/// 把任意文件/文件夹/快捷方式路径加入指定栅栏（拖入 / 粘贴共用）。
///
/// 目标目录 = 栅栏的链接文件夹（`storage_path`，有则用）否则内部库。链接栅栏由此实现
/// 「栅栏 → 文件夹」方向：拖入/粘贴的文件落进文件夹而非内部库。源已在目标目录内
/// （幂等粘贴/跨栅栏移动）直接复用。位图随后在 `handle_event` 末尾随场景上传。
pub(crate) fn add_paths_to_fence(rt: &mut Runtime, fence: usize, paths: &[String]) {
    let dest_dir = rt
        .desk
        .fences
        .get(fence)
        .and_then(|f| f.storage_path.clone())
        .map(PathBuf::from);
    for src in paths {
        let src_path = Path::new(src);
        // 先物理复制进目标目录：栅栏索引的是「目录内副本」，目录/库内删除 → 栅栏项同步删。
        let target: PathBuf = match &dest_dir {
            Some(d) if path_within(d, src_path) => src_path.to_path_buf(),
            Some(d) => match copy_into_dir(src_path, d) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(src, "复制进链接文件夹失败: {e}");
                    continue;
                }
            },
            None if is_inside_library(rt, src_path) => src_path.to_path_buf(),
            None => match copy_into_library(rt, src_path) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(src, "复制进库失败: {e}");
                    continue;
                }
            },
        };
        register_fence_item(rt, fence, &target.to_string_lossy());
    }
}

/// 把一个已落在目标目录内的路径注册为栅栏图标：建 `DesktopItem`、录入元数据、
/// 分配位图槽、追加成员、提取图标（进 `pending_uploads`）。已在本栅栏则幂等跳过。
/// 拖入/粘贴（`add_paths_to_fence`）与链接文件夹镜像（`mirror_linked_fence`）共用。
pub(crate) fn register_fence_item(rt: &mut Runtime, fence: usize, path: &str) -> bool {
    let lower_path = path.to_ascii_lowercase();

    // 优先复用已有的 DesktopItem 与位图，防止每周期重复 push 导致 items 数组泄露膨胀
    let existing_id = if rt.desk.icons.contains_key(&lower_path) {
        Some(lower_path.clone())
    } else {
        rt.item_index
            .get(&lower_path)
            .and_then(|&idx| rt.items.get(idx).map(|it| it.id.clone()))
    };

    let id = if let Some(id) = existing_id {
        id
    } else {
        let item = match winbosk_shell::items::item_from_path(path) {
            Ok(it) => it,
            Err(e) => {
                tracing::warn!(path, "无法创建图标项: {e}");
                return false;
            }
        };
        let item_id = item.id.clone();
        // 位图槽必须**单调递增**分配，不能用 `rt.items.len()`：`remove_icon_entirely`
        // 会从 items 池里移除元素并重建下标，之后 `items.len()` 会与既有槽号重合，
        // 导致新项覆盖旧项的图标位图（与 `editing.rs` 改名路径同一口径）。
        let new_bitmap = rt.bitmap_ids.values().copied().max().unwrap_or(0) + 1;
        let mut ic = Icon::new(item_id.clone(), item.display_name.clone(), item.kind);
        ic.path = Some(path.to_string());
        ic.added = true;
        winbosk_core::details::enrich(&mut ic, path);
        rt.desk.icons.insert(item_id.clone(), ic);

        let idx = rt.items.len();
        rt.items.push(item);
        rt.item_index.insert(item_id.clone(), idx);
        rt.bitmap_ids.insert(item_id.clone(), new_bitmap);

        match winbosk_shell::icons::extract_icon(&rt.items[idx], ICON_EXTRACT_SIZE) {
            Ok(data) => rt.pending_uploads.push((new_bitmap, data)),
            Err(e) => tracing::warn!(path, "图标提取失败: {e}"),
        }
        item_id
    };

    // 自动捕获：仅当源栅栏是桌面栅栏时，才允许分流到开启了 auto_capture 的其它栅栏中。
    // 「是否桌面镜像栅栏」统一走 `dir_eq`（忽略大小写/分隔符/尾分隔符），与
    // `mirror_linked_fence`、`reset_fence_storage` 保持同一口径。
    let is_desktop_fence = rt
        .desk
        .fences
        .get(fence)
        .and_then(|f| f.storage_path.as_deref())
        .zip(shell_desktop_path())
        .map(|(sp, d)| dir_eq(Path::new(sp), Path::new(&d)))
        .unwrap_or(false);

    let target_fence = if !is_desktop_fence
        || rt
            .desk
            .fences
            .get(fence)
            .and_then(|f| f.rule.as_ref())
            .map(|r| r.matches_icon(&rt.desk.icons[&id]))
            .unwrap_or(false)
    {
        fence
    } else {
        rt.desk
            .fences
            .iter()
            .enumerate()
            .position(|(idx, f)| {
                idx != fence
                    && f.rule
                        .as_ref()
                        .map(|r| r.enabled && r.auto_capture && r.matches_icon(&rt.desk.icons[&id]))
                        .unwrap_or(false)
            })
            .unwrap_or(fence)
    };

    let Some(f) = rt.desk.fences.get_mut(target_fence) else {
        return false;
    };
    if f.icon_ids.contains(&id) {
        return false;
    }
    f.icon_ids.push(id.clone());
    // 状态全集校验：同一项不会「既在未分组区又在栅栏里」。未分组区在渲染层没有任何绘制入口，
    // 残留只会让镜像刚加回的桌面项仍被当成「未分组」，语义自相矛盾。
    rt.desk.free_icons.retain(|x| x != &id);
    true
}

/// 虚拟壳项的「池」视图：`Runtime` 里与虚拟项登记相关的那几块字段。
///
/// 单独抽象出来的唯一原因是**可单测**：真实 `Runtime` 需要 GPU `Compositor` /
/// overlay 窗口句柄，单元测试里造不出来；而本模块的判定口径（清池差集、幂等、
/// 位图槽单调、不入 `free_icons`）必须锁死。字段与 `Runtime` 一一对应，`Runtime`
/// 版本只是转发。
pub(crate) struct VirtualPool<'a> {
    pub(crate) desk: &'a mut Desk,
    pub(crate) items: &'a mut Vec<DesktopItem>,
    pub(crate) item_index: &'a mut HashMap<String, usize>,
    pub(crate) bitmap_ids: &'a mut HashMap<String, u64>,
}

impl<'a> VirtualPool<'a> {
    /// 与 `remove_icon_entirely` 同口径：从池/未分组区/位图表/`items` 里整体摘除。
    fn remove(&mut self, id: &str) {
        self.desk.icons.remove(id);
        self.desk.free_icons.retain(|x| x != id);
        for f in &mut self.desk.fences {
            f.icon_ids.retain(|x| x != id);
        }
        self.bitmap_ids.remove(id);
        if let Some(i) = self.items.iter().position(|it| it.id == *id) {
            self.items.remove(i);
        }
        *self.item_index = self
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| (it.id.clone(), i))
            .collect();
    }

    /// 位图槽分配：**单调递增**，不用 `items.len()`（`remove` 会重建下标，`len()`
    /// 会与既有槽号重合，导致新项覆盖旧项位图）。
    fn next_bitmap_slot(&self) -> u64 {
        self.bitmap_ids.values().copied().max().unwrap_or(0) + 1
    }

    /// 把一个虚拟壳项快照注册进指定栅栏（下标）。
    ///
    /// 与 `register_fence_item` 同款事务口径（`items` / `item_index` / `bitmap_ids`
    /// 同帧对齐），但有两点本质差异：
    ///
    /// 1. **`Icon.path == None`**（H4）。虚拟项不在任何管理区：`movable_items` 不会
    ///    把它当文件搬，`file_extension` 不会从 `::{…}` 切出假扩展名，
    ///    `is_managed_path` 恒 false（删除只走原生动词，WinBosk 绝不直删）。
    /// 2. 位图槽单调且**不与既有槽号重合**（见 [`Self::next_bitmap_slot`]）。
    ///
    /// 三块状态（元数据 / DesktopItem / 位图槽）各自独立补齐：它们可能单独缺失
    /// （`validate()` 剔元数据、启动枚举重建 items 池），只判一个会让另一种缺失时
    /// 成员挂上去又被静默剔掉。
    ///
    /// 抽图失败只 warn、不阻断注册（图标缺帧可后续补，栅栏成员不能丢）。
    /// 返回是否真的新增（已在本栅栏 → false，供调用方判定幂等）。
    fn register(
        &mut self,
        fence: usize,
        snap: &VirtualItemSnapshot,
        pending: &mut Vec<(u64, IconData)>,
    ) -> bool {
        let id = winbosk_shell::virtual_items::virtual_item_id(&snap.display_name, &snap.clsid);

        // 分开判「元数据在不在」与「DesktopItem 在不在」：两者都可能单独缺失
        // （`validate()` 会剔除池里没有元数据的成员引用；启动枚举会重建 items 池）。
        // 只判一个会让另一种缺失时既不补元数据也不补 PIDL —— 成员挂上去后被
        // `validate()` 静默剔掉，回收站时有时无。
        let has_meta = self.desk.icons.contains_key(&id);
        let has_item = self.item_index.contains_key(&id);
        if !has_meta {
            let mut ic = Icon::new(
                id.clone(),
                snap.display_name.clone(),
                winbosk_core::model::ItemKind::Unknown,
            );
            ic.path = None; // H4：显式 None，绝不从解析名派生
            ic.added = true; // 非真实桌面文件：不走镜像回收语义
            self.desk.icons.insert(id.clone(), ic);
        }
        if !has_item {
            let item = winbosk_shell::virtual_items::virtual_item(snap);
            self.items.push(item);
            let idx = self.items.len() - 1;
            self.item_index.insert(id.clone(), idx);
        }
        // 位图槽 + 抽图以 `bitmap_ids` 为准（渲染层真正的查表口径）：缺槽才补，
        // 缺槽**不一定**伴随缺 DesktopItem。
        let slot = self.next_bitmap_slot();
        if !self.bitmap_ids.contains_key(&id) {
            self.bitmap_ids.insert(id.clone(), slot);
            if let Some(&idx) = self.item_index.get(&id) {
                // 抽图（进 pending_uploads，随首帧一起上传）；失败不阻断注册
                match winbosk_shell::icons::extract_icon(&self.items[idx], ICON_EXTRACT_SIZE) {
                    Ok(data) => pending.push((slot, data)),
                    Err(e) => tracing::warn!(id, "虚拟壳项图标提取失败: {e}"),
                }
            }
        }
        // **两条路径都要强制 `added = true`**：启动时的元数据补齐循环会把枚举到的
        // 每一项无条件写成 `added = false`（`main.rs` 的 `for it in &items`），而回收站
        // 就在枚举结果里 —— 若这里不纠正，「删除」会走 `move_icon(id, None)` 把它丢进
        // 没有绘制入口的未分组区，图标凭空消失；Shell 菜单还会注入一个空转的「移出栅栏」。
        if let Some(ic) = self.desk.icons.get_mut(&id) {
            ic.added = true;
            ic.path = None; // 同样强制：不得被任何回填逻辑塞进文件路径
        }

        let Some(f) = self.desk.fences.get_mut(fence) else {
            return false;
        };
        if f.icon_ids.contains(&id) {
            return false;
        }
        f.icon_ids.push(id.clone());
        // 状态全集校验：虚拟项与文件项一样，不能「既在未分组区又在栅栏里」。
        self.desk.free_icons.retain(|x| x != &id);
        true
    }

    /// 虚拟壳项一次性同步：清池 → 注册 → 移除。
    ///
    /// 绝不要挂进 `SyncLibrary`(4s)，也绝不要并进 `reconcile_fences`：
    /// `mirror_converged` 的三个入参全派生自磁盘，`mirror_existing_paths` 首句
    /// `.filter_map(|ic| ic.path.as_deref())` 会丢弃所有 `path == None` 的项 ⇒
    /// 虚拟项永远进不了 `owned` ⇒ 每 4s 恒不收敛 ⇒ 早退失效、每轮跑全池
    /// `Path::exists()`（本机 173 次 stat）。详见 H1/H2。
    fn sync(&mut self, snap: &[VirtualItemSnapshot], pending: &mut Vec<(u64, IconData)>) -> bool {
        // 1. 清池（先清后注册）：任何旧口径死元数据一次性清掉。
        //    判据用「快照差集」而非按 CLSID 反查旧 id——现网 desk.json 已有 15 条死元数据，
        //    其中 6 条是英文 locale 名且不含 CLSID，反查无效（H3）。
        let live: HashSet<String> = snap
            .iter()
            .map(|s| winbosk_shell::virtual_items::virtual_item_id(&s.display_name, &s.clsid))
            .collect();
        let stale: Vec<String> = self
            .desk
            .icons
            .iter()
            .filter(|(id, ic)| ic.path.is_none() && winbosk_core::shell_items::is_virtual_id(id))
            .filter(|(id, _)| !live.contains(*id))
            .map(|(id, _)| id.clone())
            .collect();
        let mut changed = false;
        for id in stale {
            self.remove(&id);
            changed = true;
        }

        // 2. 注册：目标栅栏 = 桌面栅栏（复用 `resolve_desktop_fence` 口径）
        let target = resolve_desktop_fence(self.desk, None);
        let Some(fid) = target else {
            tracing::warn!("未找到桌面栅栏，虚拟壳项同步跳过（本切片不兜底创建栅栏）");
            return false;
        };
        let Some(idx) = self.desk.fences.iter().position(|f| f.id == fid) else {
            return false;
        };
        for s in snap {
            if self.register(idx, s, pending) {
                changed = true;
            }
        }

        // 3. 移除：栅栏成员里`是虚拟 id`但不在快照里 → 从成员移除（不删任何磁盘内容）
        for f in &mut self.desk.fences {
            let before = f.icon_ids.len();
            f.icon_ids
                .retain(|id| !winbosk_core::shell_items::is_virtual_id(id) || live.contains(id));
            if f.icon_ids.len() != before {
                changed = true;
            }
        }

        changed
    }
}

/// 虚拟壳项一次性同步（`Runtime` 入口）。**只在启动与控制中心「刷新」时调用。**
///
/// 注册口径见 [`VirtualPool::register`]（`sync` 内部走同一实现，注册 = 同步的第 2 步）。
pub(crate) fn sync_virtual_items(rt: &mut Runtime, snap: &[VirtualItemSnapshot]) -> bool {
    let mut pending = Vec::new();
    let changed = {
        let mut pool = VirtualPool {
            desk: &mut rt.desk,
            items: &mut rt.items,
            item_index: &mut rt.item_index,
            bitmap_ids: &mut rt.bitmap_ids,
        };
        pool.sync(snap, &mut pending)
    };
    rt.pending_uploads.extend(pending);
    changed
}

/// 目标目录不可作为栅栏存储位置的原因（具名结果，不用歧义布尔值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageReject {
    /// 不是已存在的文件夹。
    NotADirectory,
    /// 与应用内部库重叠（库本身 / 其子目录 / 其祖先目录）。
    InsideLibrary,
    /// 栅栏下标越界（详情区已不在，防御分支）。
    NoSuchFence,
}

impl StorageReject {
    /// 面向用户的拒绝原因（告警框文案）。
    ///
    /// 与变体定义同处一地：将来新增变体会被 `match` 的穷尽性强制补文案，
    /// 不会再出现"拒绝了但用户不知道"的静默失败。
    pub(crate) fn reason(self) -> &'static str {
        match self {
            StorageReject::NotADirectory => "所选路径不是已存在的文件夹。",
            StorageReject::InsideLibrary => {
                "不能选应用内部库本身、它的子目录或它的上级目录——否则这个栅栏会把\
                 整个共享库（含其它栅栏的文件）镜像进来。"
            }
            StorageReject::NoSuchFence => "目标栅栏已不存在。",
        }
    }
}

/// 校验目标目录能否作为栅栏存储位置（**纯判定，无副作用**）。
///
/// 与 `change_fence_storage` 内部的守卫**共用同一真源**：控制中心在弹确认框之前先调它，
/// 把此前"点了没反应、只有一行日志"的静默失败换成明确的告警框。
pub(crate) fn validate_storage_dir(rt: &Runtime, new_dir: &str) -> Result<(), StorageReject> {
    let new_path = Path::new(new_dir);
    if !new_path.is_dir() {
        return Err(StorageReject::NotADirectory);
    }
    // 拒绝把存储位置设为应用内部库本身、其子目录或其祖先目录。
    // 否则该栅栏会通过 `mirror_linked_fence` 把**整个共享库**（含其它栅栏的项）镜像进来，
    // 同一文件同时出现在多个栅栏；祖先目录还会连带镜像 desk.json 等内部文件。
    if is_inside_library(rt, new_path) || path_within(new_path, &rt.library) {
        return Err(StorageReject::InsideLibrary);
    }
    Ok(())
}

/// 需要随「更改文件位置…」搬走的库内项：`added == true`（栅栏自建的副本）且文件当前在内部库内。
///
/// 与 `change_fence_storage` 的实际搬移集合**同源**——控制中心用它决定"要不要弹确认框"：
/// 数量为 0 时保持无模态（基础操作不加摩擦），> 0 时才提示"这些文件会被搬走"。
pub(crate) fn movable_items(rt: &Runtime, fence_idx: usize) -> Vec<String> {
    let Some(fence) = rt.desk.fences.get(fence_idx) else {
        return Vec::new();
    };
    fence
        .icon_ids
        .iter()
        .filter(|id| {
            rt.desk
                .icons
                .get(*id)
                .map(|ic| {
                    ic.added
                        && ic
                            .path
                            .as_ref()
                            .map(|p| is_inside_library(rt, Path::new(p)))
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// [`movable_items`] 的数量（控制中心确认框只需计数）。
pub(crate) fn count_movable_items(rt: &Runtime, fence_idx: usize) -> usize {
    movable_items(rt, fence_idx).len()
}

/// 更改栅栏的存储位置：将栅栏内所有库内项移动到新目录，更新路径引用。
/// 桌面枚举项（added=false）不受影响（它们的文件由系统管理）。
///
/// **这是真搬文件**（复制到新目录 + 删除旧副本），故控制中心在调用前会弹一次确认；
/// 返回 `Err` 时**零副作用**——全部校验都前置于任何写入。
pub(crate) fn change_fence_storage(
    rt: &mut Runtime,
    fence_idx: usize,
    new_dir: &str,
) -> Result<(), StorageReject> {
    validate_storage_dir(rt, new_dir)?;
    let new_path = Path::new(new_dir);
    let (fence_id, old_dir) = match rt.desk.fences.get(fence_idx) {
        Some(f) => (f.id, f.storage_path.clone()),
        None => return Err(StorageReject::NoSuchFence),
    };
    // 收集需要移动的库内项（added=true 且路径在旧库内）
    let items_to_move = movable_items(rt, fence_idx);
    if items_to_move.is_empty() {
        // 没有库内项需要移动，直接更新 storage_path
        if let Some(f) = rt.desk.fences.get_mut(fence_idx) {
            f.storage_path = Some(new_dir.to_string());
        }
        // 换链接：清理旧链接文件夹里的残留镜像项（文件保留在旧文件夹，不删）
        if let Some(old) = &old_dir {
            clear_stale_linked_items(rt, fence_idx, old, Some(new_path));
        }
        // 链接：把文件夹里已有的文件立即镜像进栅栏（此后增删由后台 SyncLibrary 持续同步）
        if reconcile_fences(rt) {
            tracing::info!(fence = fence_id, "链接后新增了文件夹里的栅栏项");
        }
        let _ = rt.store.save(&rt.desk);
        tracing::info!(fence = fence_id, new_dir, "无库内项需移动，仅更新存储路径");
        return Ok(());
    }
    // 确保目标目录存在
    let _ = std::fs::create_dir_all(new_path);
    let mut moved = 0usize;
    for id in &items_to_move {
        let old_path_str = match rt.desk.icons.get(id).and_then(|ic| ic.path.clone()) {
            Some(p) => p,
            None => continue,
        };
        let old_path = std::path::Path::new(&old_path_str);
        let file_name = match old_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let dest = unique_library_path(new_path, &file_name);
        // 移动文件/文件夹
        let move_ok = if old_path.is_dir() {
            copy_dir_all(old_path, &dest).is_ok()
        } else {
            std::fs::copy(old_path, &dest).is_ok()
        };
        if !move_ok {
            tracing::warn!(old_path_str, "移动文件失败");
            continue;
        }
        // 删除旧文件（移动语义）
        if old_path.is_dir() {
            let _ = std::fs::remove_dir_all(old_path);
        } else {
            let _ = std::fs::remove_file(old_path);
        }
        // 更新 Icon.path
        let new_path_str = dest.to_string_lossy().into_owned();
        if let Some(ic) = rt.desk.icons.get_mut(id) {
            ic.path = Some(new_path_str.clone());
        }
        // 更新 DesktopItem 路径（用于后续打开/提取图标）
        if let Some(&idx) = rt.item_index.get(id) {
            if let Some(item) = rt.items.get_mut(idx) {
                item.path = Some(new_path_str.clone());
            }
        }
        moved += 1;
    }
    // 更新栅栏的存储路径
    if let Some(f) = rt.desk.fences.get_mut(fence_idx) {
        f.storage_path = Some(new_dir.to_string());
    }
    // 换链接：清理旧链接文件夹里的残留镜像项（文件保留在旧文件夹，不删）
    if let Some(old) = &old_dir {
        clear_stale_linked_items(rt, fence_idx, old, Some(new_path));
    }
    // 链接：把文件夹里已有的文件立即镜像进栅栏（此后增删由后台 SyncLibrary 持续同步）
    if reconcile_fences(rt) {
        tracing::info!(fence = fence_id, "链接后新增了文件夹里的栅栏项");
    }
    let _ = rt.store.save(&rt.desk);
    // 若旧路径在内部库下且已空，自动删除（避免残留空文件夹）
    if let Some(old) = &old_dir {
        let old_path = std::path::Path::new(old);
        if old_path.is_dir()
            && old_path.starts_with(&rt.library)
            && old_path
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(old_path);
            tracing::info!("已清理空的旧库子文件夹: {}", old);
        }
    }
    tracing::info!(
        fence = fence_id,
        new_dir,
        moved,
        total = items_to_move.len(),
        "栅栏存储位置已更改"
    );
    Ok(())
}

/// 换链接后清理旧链接文件夹里的残留镜像项：`added` 项路径在旧文件夹内、
/// 不在新文件夹内，说明它属于旧链接而非新链接。从栅栏整体摘下（引用删除、
/// 文件保留在旧文件夹磁盘上，用户仍可从资源管理器访问），使栅栏内容与
/// 新链接文件夹保持一致，不再残留旧文件夹的项。
///
/// `new_path == None` 表示**解除链接**（不再有新文件夹）：旧文件夹内的镜像项
/// 全部摘下，栅栏回到纯收纳状态。两种情况都**只删引用，磁盘文件一个不动**。
///
/// **归属保护**：若某成员同时落在**另一个栅栏**的链接文件夹内（两个栅栏的存储位置
/// 互为父子目录时会出现），不得摘下——否则会连带注销那个栅栏的成员，≤4s 后被它的
/// 后台镜像重新注册，表现为图标反复消失/重建。
fn clear_stale_linked_items(
    rt: &mut Runtime,
    fence_idx: usize,
    old_dir: &str,
    new_path: Option<&Path>,
) {
    let stale: Vec<String> = rt
        .desk
        .fences
        .get(fence_idx)
        .map(|f| {
            f.icon_ids
                .iter()
                .filter(|id| {
                    rt.desk
                        .icons
                        .get(*id)
                        .map(|ic| {
                            ic.added
                                && ic
                                    .path
                                    .as_ref()
                                    .map(|p| {
                                        let p = Path::new(p);
                                        path_within(Path::new(old_dir), p)
                                            && !new_path
                                                .map(|np| path_within(np, p))
                                                .unwrap_or(false)
                                            && !claimed_by_other_fence(rt, fence_idx, p)
                                    })
                                    .unwrap_or(false)
                        })
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    for id in stale {
        tracing::info!(
            fence = fence_idx,
            id,
            "换链接：旧文件夹残留项移出栅栏（文件保留在磁盘）"
        );
        remove_icon_entirely(rt, &id);
    }
}

/// `path` 是否落在**除 `fence_idx` 之外**某个栅栏的链接文件夹内。
fn claimed_by_other_fence(rt: &Runtime, fence_idx: usize, path: &Path) -> bool {
    rt.desk.fences.iter().enumerate().any(|(j, f)| {
        j != fence_idx
            && f.storage_path
                .as_deref()
                .map(|sp| path_within(Path::new(sp), path))
                .unwrap_or(false)
    })
}

/// 把栅栏的文件位置恢复为应用内部库（解除外部文件夹链接）。
///
/// **契约**：不移动、不复制、不删除任何磁盘文件。原外部文件夹内的镜像成员从栅栏摘除
/// （文件仍留在该文件夹里，用户可从资源管理器访问），栅栏回到「应用内部」纯收纳模式。
///
/// 桌面镜像栅栏（`storage_path == 桌面目录`）**拒绝执行**：解除链接会让栅栏清空，
/// 而真实桌面图标此刻正被壳层接管隐藏，用户会看到「桌面全空」。
/// UI 已隐藏该按钮，此处是第二道防线。
pub(crate) fn reset_fence_storage(rt: &mut Runtime, fence_idx: usize) {
    let Some(fence) = rt.desk.fences.get(fence_idx) else {
        return;
    };
    let fence_id = fence.id;
    // `None` = 已是应用内部模式：幂等，0 变动（连续两次点击第二次无事发生）
    let Some(old_dir) = fence.storage_path.clone() else {
        return;
    };
    if let Some(desktop) = shell_desktop_path() {
        if dir_eq(Path::new(&old_dir), Path::new(&desktop)) {
            tracing::warn!(fence = fence_id, "桌面镜像栅栏不允许恢复默认");
            return;
        }
    }
    if let Some(f) = rt.desk.fences.get_mut(fence_idx) {
        f.storage_path = None;
    }
    // 原文件夹内的镜像项摘下（只删引用，磁盘文件一个不动）
    clear_stale_linked_items(rt, fence_idx, &old_dir, None);
    let _ = rt.store.save(&rt.desk);
    tracing::info!(fence = fence_id, old_dir, "存储位置已恢复为应用内部库");
}

/// 库同步：检查所有内部库项，`library` 里的文件已被外部删除时，栅栏对应项同步移除。
/// 返回是否有项被移除（调用方可据此决定持久化；重绘由事件尾部统一完成）。
pub(crate) fn reconcile_library(rt: &mut Runtime) -> bool {
    let missing: Vec<String> = rt
        .desk
        .icons
        .iter()
        .filter(|(_, ic)| ic.added)
        .filter(|(_, ic)| {
            ic.path
                .as_ref()
                .map(|p| is_inside_library(rt, Path::new(p)) && !Path::new(p).exists())
                .unwrap_or(false)
        })
        .map(|(id, _)| id.clone())
        .collect();
    let mut changed = false;
    for id in missing {
        changed = true;
        remove_icon_entirely(rt, &id);
    }
    if changed {
        tracing::info!(removed = changed, "库内文件被删，同步移除栅栏项");
    }
    changed
}

/// 双向同步总入口：内部库删除同步 + 链接栅栏文件夹镜像。
/// 返回是否有任何项被增/删（调用方据此持久化；重绘由事件尾部统一完成）。
pub(crate) fn reconcile_fences(rt: &mut Runtime) -> bool {
    let mut changed = reconcile_library(rt);
    for idx in 0..rt.desk.fences.len() {
        if mirror_linked_fence(rt, idx) {
            changed = true;
        }
    }
    changed
}

/// 镜像一个链接栅栏的存储文件夹（文件夹 → 栅栏方向）：资源管理器里对文件夹的
/// 新增/删除/改名 ≤ 后台 `SyncLibrary` 周期（4s）反映到栅栏。栅栏即文件夹——
/// 目录内容与栅栏成员互为差集：多出的路径注册进栅栏，消失的路径移除对应图标。
///
/// 廉价快路径：只用一次 `read_dir` 得到两套磁盘视图，与「已归属」集合做**两条互不干扰的
/// 子集断言**（`mirror_converged`）——收敛即立即返回，不进昂贵的 `DesktopItem` 构建/移除。
/// 这里**不能**用「集合相等」做判据：已归属但被外部置为隐藏/系统属性的文件会出现在
/// 「全部条目」里、却永远不在「可见条目」里，集合相等会永久不成立 → 每 4s 白跑一遍全量
/// 注册 + 全量 `exists()` 探测（破坏空闲 0% CPU 铁律）。
/// 文件夹不可用（被删/断连）时不动（防御）。
fn mirror_linked_fence(rt: &mut Runtime, idx: usize) -> bool {
    let Some(dir) = rt.desk.fences.get(idx).and_then(|f| f.storage_path.clone()) else {
        return false;
    };
    let dir = PathBuf::from(dir);
    if !dir.is_dir() {
        return false;
    }
    // 链接 = 双向：先把栅栏里仍留在内部库的旧项物理迁进链接文件夹（栅栏里的东西 →
    // 文件夹），之后栅栏才真正镜像该文件夹。一次性迁移，完成后路径都在文件夹内，
    // 之后不再触发。文件已不存在的不迁（交给移除分支处理）。
    let mut changed = rehome_linked_library_items(rt, idx, &dir);

    let is_desktop = shell_desktop_path()
        .map(|d| dir_eq(Path::new(&d), &dir))
        .unwrap_or(false);

    // 源目录：桌面镜像 = 用户桌面 + 公共桌面（Windows 桌面上显示的是两者并集）；
    // 普通链接栅栏 = 它自己的目录。`dir` 自身仍是栅栏的 `storage_path`（新文件落盘位置、
    // 删除语义、控制中心展示都依赖它），公共桌面只进「扫描集合」，不成为 storage_path。
    let roots: Vec<PathBuf> = if is_desktop {
        desktop_source_dirs(&dir, shell_public_desktop_path().map(PathBuf::from))
    } else {
        vec![dir.clone()]
    };

    // 两套磁盘视图（同一次 `read_dir`，不额外产生系统调用）：
    // - `all_set`：目录列出的**全部**条目（含隐藏/系统）→ 用于判「已归属项是否还在磁盘上」；
    // - `visible`：应当镜像的条目（跳过隐藏/系统，与资源管理器默认一致）→ 用于注册。
    let mut all_set: HashSet<String> = HashSet::new();
    let mut visible: Vec<PathBuf> = Vec::new();
    for d in &roots {
        for e in list_dir_entries(d) {
            let p = e.path();
            all_set.insert(p.to_string_lossy().to_ascii_lowercase());
            if should_mirror(&e) {
                visible.push(p);
            }
        }
    }

    // 「已归属」路径集合（小写，限定在源目录内）——口径见 `mirror_existing_paths`。
    let owned = mirror_existing_paths(&rt.desk, idx, &roots, is_desktop);
    if mirror_converged(&owned, &all_set, &visible) {
        return changed; // 无新项要注册、无孤儿要回收 → 无事可做（可能刚迁移过库内项）
    }

    // 文件夹 → 栅栏：新出现的文件/子文件夹注册进栅栏（含改名产生的新路径）
    for p in &visible {
        let lower = p.to_string_lossy().to_ascii_lowercase();
        if !owned.contains(&lower) && register_fence_item(rt, idx, &p.to_string_lossy()) {
            changed = true;
        }
    }

    // 栅栏 → 文件夹：路径在该目录下但文件已不存在 → 移除（外部删除/改名）
    if is_desktop {
        // 桌面源：若桌面上的物理文件被删，清理所有持有该桌面路径的图标（即使它已被分类整理到其他收纳栅栏中）
        let all_ids: Vec<String> = rt.desk.icons.keys().cloned().collect();
        for id in all_ids {
            let Some(p) = rt.desk.icons.get(&id).and_then(|ic| ic.path.clone()) else {
                continue;
            };
            // 「桌面」= 用户桌面 + 公共桌面：任一根目录内的文件消失都要回收
            if roots.iter().any(|r| path_within(r, Path::new(&p))) && !Path::new(&p).exists() {
                remove_icon_entirely(rt, &id);
                changed = true;
            }
        }
    } else {
        // 普通目录镜像栅栏：仅清理该栅栏内的成员
        let ids: Vec<String> = rt
            .desk
            .fences
            .get(idx)
            .map(|f| f.icon_ids.clone())
            .unwrap_or_default();
        for id in ids {
            let Some(p) = rt.desk.icons.get(&id).and_then(|ic| ic.path.clone()) else {
                continue;
            };
            if !path_within(&dir, Path::new(&p)) {
                continue;
            }
            if !Path::new(&p).exists() {
                remove_icon_entirely(rt, &id);
                changed = true;
            }
        }
    }
    changed
}

/// 桌面镜像栅栏的源目录集合：用户桌面 + 公共桌面。
///
/// Windows 桌面上显示的是两者的并集（用户桌面文件 + `C:\Users\Public\Desktop` 的公共快捷方式），
/// 只扫用户桌面会让那批公共快捷方式在真实桌面被壳层接管隐藏后凭空消失。`public` 由调用方注入
/// （壳层解析结果），便于纯函数单测。
fn desktop_source_dirs(user: &Path, public: Option<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = vec![user.to_path_buf()];
    if let Some(p) = public {
        push_unique_dir(&mut dirs, p);
    }
    dirs
}

/// 目录去重追加：忽略 ASCII 大小写与尾部分隔符（Windows 路径语义），已在集合内则不追加。
fn push_unique_dir(dirs: &mut Vec<PathBuf>, extra: PathBuf) {
    if !dirs.iter().any(|d| dir_eq(d, &extra)) {
        dirs.push(extra);
    }
}

/// 两个路径是否指向同一处目录：忽略分隔符差异（`/` 与 `\`）、尾部分隔符与 ASCII 大小写。
pub(crate) fn dir_eq(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    }
    let (x, y) = (norm(a), norm(b));
    !x.is_empty() && x == y
}

/// 读一个目录下的全部条目（**不做**任何属性过滤，含隐藏/系统文件，如 `desktop.ini`）。
/// 读取失败（不存在/被占用）视为空。是否镜像由调用方用 `should_mirror` 过滤。
///
/// 返回 `DirEntry` 而不是 `PathBuf`：隐藏/系统判断要用**条目自带的属性缓存**
/// （见 `should_mirror`），先转成路径就把那份缓存丢了。
fn list_dir_entries(dir: &Path) -> Vec<std::fs::DirEntry> {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).collect())
        .unwrap_or_default()
}

/// 镜像是否已收敛（本轮无事可做）——**两条互不干扰的子集断言**：
///
/// 1. `owned ⊆ all`：所有已归属项都还在磁盘上 → 没有孤儿要回收；
/// 2. `visible ⊆ owned`：所有应当镜像的条目都已归属 → 没有新项要注册。
///
/// 反例（为什么不用「集合相等」）：某项被外部加上隐藏/系统属性后，它仍在 `all` 里、却不在
/// `visible` 里；只要它已归属，`owned != visible` 会**永久**成立 → 每 4s 都白跑一遍注册与
/// 全量 `exists()` 探测，违反空闲 0% CPU 铁律。两条子集断言则各自只关心自己那一侧。
fn mirror_converged(owned: &HashSet<String>, all: &HashSet<String>, visible: &[PathBuf]) -> bool {
    owned.is_subset(all)
        && visible
            .iter()
            .all(|p| owned.contains(&p.to_string_lossy().to_ascii_lowercase()))
}

/// 计算镜像栅栏的「已归属」路径集合（小写，限定在源目录 `roots` 内）。
///
/// - `is_desktop == true`（**桌面镜像栅栏**）：**归属口径**——凡已被任一栅栏持有的桌面文件都算
///   「已归属」，因此「一键整理」把图标搬进分类栅栏后不会被镜像抢回桌面栅栏。
///   **禁止**改回「读全局图标池 `desk.icons`」：启动时的元数据补齐会把**枚举到的每一项**
///   无条件写进池（无论有无归属），于是「池里有」被当成「栅栏里有」，所有项全被判为已归属
///   → 桌面栅栏永远为空（线上事故，详见 `docs/plans/08-desktop-mirror-ownership-and-public-desktop.md`）。
/// - `is_desktop == false`（普通目录镜像栅栏）：只取**本栅栏成员**，严格维持当前成员。
///
/// `free_icons`（未分组区）**不计入**归属：它在渲染层没有任何绘制入口，把它当归属会让
/// 「移出栅栏」的桌面项彻底从视野里消失（真实桌面图标已被接管隐藏）；不计入则镜像会把它
/// 加回栅栏，最坏结果只是「移出无效果」，优于图标凭空不见。
///
/// 本函数**不做**可见性（隐藏/系统属性）过滤——它要回答的是「这项是否已被某栅栏持有」，
/// 与磁盘上「这项是否被隐藏」无关；两侧口径的差异由 `mirror_converged` 的**两条子集断言**
/// 消化（见其文档，别改成集合相等）。
fn mirror_existing_paths(
    desk: &Desk,
    idx: usize,
    roots: &[PathBuf],
    is_desktop: bool,
) -> HashSet<String> {
    let owned_ids: Vec<&String> = if is_desktop {
        desk.fences.iter().flat_map(|f| f.icon_ids.iter()).collect()
    } else {
        desk.fences
            .get(idx)
            .into_iter()
            .flat_map(|f| f.icon_ids.iter())
            .collect()
    };
    owned_ids
        .into_iter()
        .filter_map(|id| desk.icons.get(id).and_then(|ic| ic.path.as_deref()))
        .filter(|p| roots.iter().any(|r| path_within(r, Path::new(p))))
        .map(|p| p.to_ascii_lowercase())
        .collect()
}

/// 链接栅栏的旧库内项迁移：把「路径仍在内部库」的栅栏图标物理移到链接文件夹，
/// 实现「栅栏里的东西 → 文件夹」。一次性——迁移后路径都在文件夹内不再触发。
fn rehome_linked_library_items(rt: &mut Runtime, idx: usize, dir: &Path) -> bool {
    let ids: Vec<String> = rt
        .desk
        .fences
        .get(idx)
        .map(|f| f.icon_ids.clone())
        .unwrap_or_default();
    let mut changed = false;
    for id in ids {
        let in_library = rt
            .desk
            .icons
            .get(&id)
            .and_then(|ic| ic.path.clone())
            .map(|p| is_inside_library(rt, Path::new(&p)))
            .unwrap_or(false);
        if in_library && rehome_item(rt, &id, dir) {
            changed = true;
        }
    }
    changed
}

/// 把单个图标对应的磁盘文件从旧路径移动到 `dest_dir`（同名冲突自动改名），
/// 更新图标/项的路径引用。id（= 小写路径）保持旧值——`register_fence_item` 的
/// 路径级判重兜底陈旧 id，避免镜像重复添加。返回是否真的移动了。
fn rehome_item(rt: &mut Runtime, id: &str, dest_dir: &Path) -> bool {
    let Some(old_path_str) = rt.desk.icons.get(id).and_then(|ic| ic.path.clone()) else {
        return false;
    };
    let old_path = Path::new(&old_path_str);
    if !old_path.exists() {
        return false; // 文件已不存在：交给移除分支
    }
    let Some(file_name) = old_path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let dest = unique_library_path(dest_dir, file_name);
    let move_ok = if old_path.is_dir() {
        copy_dir_all(old_path, &dest).is_ok()
    } else {
        std::fs::copy(old_path, &dest).is_ok()
    };
    if !move_ok {
        tracing::warn!(old_path_str, "迁移到链接文件夹失败");
        return false;
    }
    if old_path.is_dir() {
        let _ = std::fs::remove_dir_all(old_path);
    } else {
        let _ = std::fs::remove_file(old_path);
    }
    let new_path_str = dest.to_string_lossy().into_owned();
    if let Some(ic) = rt.desk.icons.get_mut(id) {
        ic.path = Some(new_path_str.clone());
    }
    if let Some(&idx) = rt.item_index.get(id) {
        if let Some(item) = rt.items.get_mut(idx) {
            item.path = Some(new_path_str.clone());
        }
    }
    tracing::info!(id, from = %old_path_str, to = %new_path_str, "栅栏库内项已迁移到链接文件夹");
    true
}

/// 该目录项是否应在栅栏镜像中显示：跳过隐藏/系统文件（与资源管理器默认一致，
/// 免 `desktop.ini`、缩略图缓存等混入）。属性读取失败（被占用/已删）视为不显示。
///
/// **必须走 `DirEntry` 自带的属性缓存，不要改回按路径 `GetFileAttributesW`。**
/// Windows 上 `DirEntry::metadata()` 直接取枚举时 `WIN32_FIND_DATAW` 里的属性、
/// **不额外发系统调用**（std 文档明确保证）；而按路径逐个查询会让每 4s 的心跳变成
/// `O(目录项数)` 次系统调用——本机实测 141 项 ≈ 5.2 ms（单次约 37 µs，被 AV 过滤驱动
/// 放大），而整个心跳才 4~7 ms，即**这层过滤就是心跳的全部成本**。
fn should_mirror(entry: &std::fs::DirEntry) -> bool {
    let Ok(md) = entry.metadata() else {
        return false;
    };
    md.file_attributes() & (FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0) == 0
}

/// `path` 是否位于内部库文件夹内（组件级大小写不敏感前缀比较，含边界）。
pub(crate) fn is_inside_library(rt: &Runtime, path: &Path) -> bool {
    let lib: Vec<String> = rt
        .library
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    if lib.is_empty() {
        return false;
    }
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    comps.len() >= lib.len() && lib.iter().zip(comps.iter()).all(|(a, b)| a == b)
}

/// `path` 是否**严格位于** `root` 目录内（组件级大小写不敏感前缀比较）。
/// 组件边界保证 `C:\lib` 不会匹配 `C:\library\f.txt`；路径即根本身不算「在内」。
pub(crate) fn path_within(root: &Path, path: &Path) -> bool {
    let root_c: Vec<String> = root
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    if root_c.is_empty() {
        return false;
    }
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    comps.len() > root_c.len() && root_c.iter().zip(comps.iter()).all(|(a, b)| a == b)
}

/// 路径是否位于某个栅栏的链接存储文件夹内（「移出栅栏 = 删除文件」判定用）。
pub(crate) fn is_linked_path(rt: &Runtime, path: &str) -> bool {
    let p = Path::new(path);
    rt.desk.fences.iter().any(|f| {
        f.storage_path
            .as_ref()
            .map(|d| path_within(Path::new(d), p))
            .unwrap_or(false)
    })
}

/// 是否属于 WinBosk 管理区：内部库或任一栅栏的链接存储文件夹。管理区内的文件删除
/// 会真实删到磁盘（删除/移出动作），管理区外（桌面源文件）只动引用不碰文件。
pub(crate) fn is_managed_path(rt: &Runtime, path: &Path) -> bool {
    is_inside_library(rt, path) || is_linked_path(rt, &path.to_string_lossy())
}

/// 删除 WinBosk 管理区内的磁盘文件/文件夹（内部库或链接文件夹）；不在管理区则不碰。
/// 供「删除」动作使用：管理区内删文件，桌面源文件保留。
pub(crate) fn delete_managed_file(rt: &Runtime, id: &str) {
    let Some(p) = rt.desk.icons.get(id).and_then(|ic| ic.path.clone()) else {
        return;
    };
    let pp = Path::new(&p);
    if !is_managed_path(rt, pp) || !pp.exists() {
        return;
    }
    if pp.is_dir() {
        let _ = std::fs::remove_dir_all(pp);
    } else {
        let _ = std::fs::remove_file(pp);
    }
}

/// 把文件/文件夹复制进指定目录；返回目录内目标路径。同名自动改名 `name (1).ext`。
pub(crate) fn copy_into_dir(src: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("item");
    let dest = unique_library_path(dir, name);
    if src.is_dir() {
        copy_dir_all(src, &dest)?;
    } else {
        std::fs::copy(src, &dest)?;
    }
    Ok(dest)
}

/// 把文件/文件夹复制进内部库；返回库内目标路径。
pub(crate) fn copy_into_library(rt: &Runtime, src: &Path) -> std::io::Result<PathBuf> {
    copy_into_dir(src, &rt.library)
}

/// 库内不重名路径：存在则 `name (n).ext` 递增。
pub(crate) fn unique_library_path(lib: &Path, name: &str) -> PathBuf {
    let cand = lib.join(name);
    if !cand.exists() {
        return cand;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for i in 1..1000 {
        let cand = lib.join(format!("{stem} ({i}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    cand
}

/// 递归复制目录（不跟符号链接，按常规文件处理）。
pub(crate) fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// 把一个图标从 WinBosk 中整体移除（仅删引用，不碰磁盘文件）。
/// 用于「拖入/粘贴新增项」的移除——它们不属于真实桌面，移出栅栏即删除。
pub(crate) fn remove_icon_entirely(rt: &mut Runtime, id: &str) {
    rt.desk.icons.remove(id);
    rt.desk.free_icons.retain(|x| x != id);
    for f in &mut rt.desk.fences {
        f.icon_ids.retain(|x| x != id);
    }
    rt.bitmap_ids.remove(id);
    // 从 items 池移除对应 DesktopItem（持有 PIDL，Drop 时释放），并重建下标
    if let Some(i) = rt.items.iter().position(|it| it.id == *id) {
        rt.items.remove(i);
    }
    rt.item_index = rt
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| (it.id.clone(), i))
        .collect();
}

/// 剪贴板格式：CF_HDROP（拖放文件列表）。windows-rs 0.62 将其定义在
/// `Win32_System_Ole` 里；这里用文档稳定值 15，避免引入整个 Ole 功能集。
pub(crate) const CF_HDROP: u32 = 15;

/// 读剪贴板里的文件列表（CF_HDROP）。
pub(crate) fn clipboard_file_paths() -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        if OpenClipboard(None).is_err() {
            return out;
        }
        if let Ok(handle) = GetClipboardData(CF_HDROP) {
            if !handle.is_invalid() {
                let hdrop = HDROP(handle.0);
                let n = DragQueryFileW(hdrop, u32::MAX, None);
                for i in 0..n {
                    let len = DragQueryFileW(hdrop, i, None);
                    let mut buf = vec![0u16; len as usize + 1];
                    DragQueryFileW(hdrop, i, Some(&mut buf));
                    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                    out.push(String::from_utf16_lossy(&buf[..end]));
                }
            }
        }
        let _ = CloseClipboard();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_within_component_aware_case_insensitive() {
        assert!(path_within(
            Path::new(r"C:\lib"),
            Path::new(r"C:\lib\a.txt")
        ));
        assert!(path_within(
            Path::new(r"C:\Lib"),
            Path::new(r"c:\lib\sub\b.txt")
        ));
        // 组件边界：C:\lib 不匹配 C:\library\f.txt
        assert!(!path_within(
            Path::new(r"C:\lib"),
            Path::new(r"C:\library\f.txt")
        ));
        // 路径即根本身：不算「在内」（严格在内）
        assert!(!path_within(Path::new(r"C:\lib"), Path::new(r"C:\lib")));
        // 父级不是子的前缀
        assert!(!path_within(Path::new(r"C:\lib\a"), Path::new(r"C:\lib")));
    }

    #[test]
    fn path_within_empty_root_is_never_within() {
        assert!(!path_within(Path::new(""), Path::new(r"C:\x\y.txt")));
    }

    // ---- 桌面镜像的「已存在」口径（plan 08） ----

    const USER_DESK: &str = r"C:\Users\me\Desktop";
    const PUBLIC_DESK: &str = r"C:\Users\Public\Desktop";

    fn test_fence(id: u64, icon_ids: &[&str]) -> Fence {
        Fence {
            id,
            title: Some(format!("栅栏{id}")),
            monitor_id: 0,
            bounds: Rect::default(),
            state: FenceState::Expanded,
            icon_ids: icon_ids.iter().map(|s| s.to_string()).collect(),
            appearance: FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            rule: None,
            collapsed: false,
        }
    }

    /// 登记一个图标：id 即小写路径（与 `item_id` 同口径）。
    fn desktop_icon(desk: &mut Desk, path: &str) {
        let id = path.to_ascii_lowercase();
        let mut ic = Icon::new(
            id.clone(),
            id.clone(),
            winbosk_core::model::ItemKind::Unknown,
        );
        ic.path = Some(path.to_string());
        desk.icons.insert(id, ic);
    }

    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from(USER_DESK), PathBuf::from(PUBLIC_DESK)]
    }

    /// P0 回归：元数据池里有、但没有任何栅栏持有 → **不能**算「已存在」。
    /// 历史 bug：这里读全局池，而启动时枚举到的每一项都被无条件写进池，于是所有项
    /// 全被误判为已存在 → 桌面栅栏永远为空（图标全体消失）。
    #[test]
    fn desktop_existing_ignores_unowned_metadata_pool() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        let f = test_fence(1, &[]);
        desk.fences.push(f);
        desktop_icon(&mut desk, r"C:\Users\me\Desktop\a.txt");
        desktop_icon(&mut desk, r"C:\Users\me\Desktop\b.png");

        let existing = mirror_existing_paths(&desk, 0, &roots(), true);
        assert!(
            existing.is_empty(),
            "无归属的元数据不得算已存在（会导致栅栏恒空）: {existing:?}"
        );
        // 幂等：连续两次结果一致
        assert_eq!(existing, mirror_existing_paths(&desk, 0, &roots(), true));

        // 一旦归属本栅栏 → 立刻算已存在
        desk.fences[0]
            .icon_ids
            .push(r"c:\users\me\desktop\a.txt".to_string());
        let existing = mirror_existing_paths(&desk, 0, &roots(), true);
        assert_eq!(existing.len(), 1);
        assert!(existing.contains(r"c:\users\me\desktop\a.txt"));
    }

    /// 归属它栅栏（「一键整理」搬走）的桌面文件算已存在 → 不被镜像抢回。
    #[test]
    fn desktop_existing_covers_icons_owned_by_other_fences() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(test_fence(1, &[]));
        desk.fences
            .push(test_fence(2, &[r"c:\users\me\desktop\doc.pdf"]));
        desktop_icon(&mut desk, r"C:\Users\me\Desktop\doc.pdf");

        let existing = mirror_existing_paths(&desk, 0, &roots(), true);
        assert!(existing.contains(r"c:\users\me\desktop\doc.pdf"));
    }

    /// 公共桌面同样是「桌面」的一部分：归属判定不受 storage_path 只指向用户桌面的限制。
    #[test]
    fn desktop_existing_covers_public_desktop() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences
            .push(test_fence(1, &[r"c:\users\public\desktop\chrome.lnk"]));
        desktop_icon(&mut desk, r"C:\Users\Public\Desktop\Chrome.lnk");

        let existing = mirror_existing_paths(&desk, 0, &roots(), true);
        assert!(existing.contains(r"c:\users\public\desktop\chrome.lnk"));
    }

    /// 未分组区（渲染层无绘制入口）**不算归属**：否则「移出栅栏」的桌面项会彻底消失。
    #[test]
    fn desktop_existing_ignores_free_icons() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(test_fence(1, &[]));
        desktop_icon(&mut desk, r"C:\Users\me\Desktop\loose.txt");
        desk.free_icons
            .push(r"c:\users\me\desktop\loose.txt".to_string());

        assert!(mirror_existing_paths(&desk, 0, &roots(), true).is_empty());
    }

    /// 源目录之外的路径一律不进集合（例如库内项、桌面之外的分区）。
    #[test]
    fn desktop_existing_scopes_to_source_roots() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences
            .push(test_fence(1, &[r"c:\app\data\library\copy.txt"]));
        desktop_icon(&mut desk, r"C:\app\data\library\copy.txt");

        assert!(mirror_existing_paths(&desk, 0, &roots(), true).is_empty());
    }

    /// 普通目录镜像栅栏维持「本栅栏成员」口径：别的栅栏持有同一目录下的项不受影响。
    #[test]
    fn plain_mirror_existing_only_counts_own_members() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(test_fence(1, &[r"c:\portal\mine.txt"]));
        desk.fences.push(test_fence(2, &[r"c:\portal\other.txt"]));
        desktop_icon(&mut desk, r"C:\portal\mine.txt");
        desktop_icon(&mut desk, r"C:\portal\other.txt");

        let existing = mirror_existing_paths(&desk, 0, &[PathBuf::from(r"C:\portal")], false);
        assert_eq!(existing.len(), 1);
        assert!(existing.contains(r"c:\portal\mine.txt"));
    }

    /// 收敛态必须保持快路径：可见项全部已归属、且已归属项都还在磁盘上 → 直接返回。
    /// 若这条不成立，每 4s 的同步都会退化成全量注册 + 全量 `exists()` 探测。
    #[test]
    fn desktop_converged_holds_fast_path() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        let a = r"C:\Users\me\Desktop\a.txt";
        let b = r"C:\Users\Public\Desktop\b.lnk";
        desk.fences.push(test_fence(
            1,
            &[&a.to_ascii_lowercase(), &b.to_ascii_lowercase()],
        ));
        desktop_icon(&mut desk, a);
        desktop_icon(&mut desk, b);

        let roots = roots();
        let owned = mirror_existing_paths(&desk, 0, &roots, true);
        let visible = vec![PathBuf::from(a), PathBuf::from(b)];
        let all: HashSet<String> = visible
            .iter()
            .map(|p| p.to_string_lossy().to_ascii_lowercase())
            .collect();
        assert!(mirror_converged(&owned, &all, &visible));
    }

    /// 快路径判据矩阵：两条子集断言各自只负责一侧。
    #[test]
    fn mirror_converged_subset_matrix() {
        let owned: HashSet<String> = [r"c:\d\a.txt", r"c:\d\b.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let vis = |p: &str| PathBuf::from(p);
        let all_of = |ps: &[&str]| -> HashSet<String> {
            ps.iter()
                .map(|s| s.to_string().to_ascii_lowercase())
                .collect()
        };

        // 收敛：可见项都已归属，已归属项都在磁盘上
        assert!(mirror_converged(
            &owned,
            &all_of(&[r"C:\d\a.txt", r"C:\d\b.txt"]),
            &[vis(r"C:\d\a.txt"), vis(r"C:\d\b.txt")]
        ));

        // 新文件出现在磁盘上（可见但未归属）→ 未收敛（要去注册）
        assert!(!mirror_converged(
            &owned,
            &all_of(&[r"C:\d\a.txt", r"C:\d\b.txt", r"C:\d\c.txt"]),
            &[vis(r"C:\d\a.txt"), vis(r"C:\d\b.txt"), vis(r"C:\d\c.txt")]
        ));

        // 已归属项被外部删除 → 未收敛（要去回收孤儿）
        assert!(!mirror_converged(
            &owned,
            &all_of(&[r"C:\d\a.txt"]),
            &[vis(r"C:\d\a.txt")]
        ));

        // 已归属项被外部加上隐藏属性：仍在「全部条目」里、不在「可见条目」里
        // → 必须仍判为收敛（否则每 4s 白跑一轮，破坏空闲 0% CPU）
        assert!(mirror_converged(
            &owned,
            &all_of(&[r"C:\d\a.txt", r"C:\d\b.txt"]),
            &[vis(r"C:\d\a.txt")]
        ));

        // 普通栅栏：隐藏成员同理不得造成抖动
        assert!(mirror_converged(
            &owned,
            &all_of(&[r"C:\d\a.txt", r"C:\d\b.txt"]),
            &[]
        ));

        // 空目录 + 空归属（新链接的空文件夹）：收敛，不得 panic
        assert!(mirror_converged(&HashSet::new(), &HashSet::new(), &[]));
    }

    #[test]
    fn desktop_source_dirs_adds_public_and_dedupes() {
        // 公共桌面存在 → 两个目录
        let dirs = desktop_source_dirs(
            Path::new(r"C:\Users\me\Desktop"),
            Some(PathBuf::from(r"C:\Users\Public\Desktop")),
        );
        assert_eq!(dirs.len(), 2);

        // 不可用（None）→ 退化为只扫用户桌面
        let dirs = desktop_source_dirs(Path::new(r"C:\Users\me\Desktop"), None);
        assert_eq!(dirs, vec![PathBuf::from(r"C:\Users\me\Desktop")]);

        // 与用户桌面同一目录（大小写 / 尾分隔符差异）→ 只保留一份
        let dirs = desktop_source_dirs(
            Path::new(r"C:\Users\me\Desktop"),
            Some(PathBuf::from(r"c:\users\ME\desktop\")),
        );
        assert_eq!(dirs.len(), 1);
    }

    #[test]
    fn dir_eq_ignores_case_and_trailing_separator() {
        assert!(dir_eq(Path::new(r"C:\A\B"), Path::new(r"c:\a\b\")));
        assert!(dir_eq(Path::new(r"C:/A/B"), Path::new(r"C:\A\B")));
        assert!(!dir_eq(Path::new(r"C:\A\B"), Path::new(r"C:\A\BC")));
        assert!(!dir_eq(Path::new(""), Path::new("")));
    }

    // ---- 虚拟壳项同步（plan 10.1 Step 3） ----

    use winbosk_shell::virtual_items::VirtualItemSnapshot;

    /// 本机回收站快照（CLSID 大写，与白名单同值）。
    fn bin_snapshot() -> VirtualItemSnapshot {
        VirtualItemSnapshot {
            parsing_name: "::{645FF040-5081-101B-9F08-00AA002F954E}".into(),
            clsid: "{645FF040-5081-101B-9F08-00AA002F954E}".into(),
            display_name: "回收站".into(),
        }
    }

    /// 桌面栅栏（title=「桌面」，无 storage_path）。
    fn desktop_fence(id: u64) -> Fence {
        let mut f = test_fence(id, &[]);
        f.title = Some("桌面".into());
        f
    }

    /// 池里登记一条虚拟项元数据（模拟上次启动落盘的 desk.json 条目）。
    fn put_virtual_icon(desk: &mut Desk, id: &str, name: &str, path: Option<&str>) {
        let mut ic = Icon::new(
            id.to_string(),
            name.to_string(),
            winbosk_core::model::ItemKind::Unknown,
        );
        ic.path = path.map(|s| s.to_string());
        ic.added = true;
        desk.icons.insert(id.to_string(), ic);
    }

    /// 旧口径死元数据一次性清掉（中文名 / 英文 locale 名都不含 CLSID ⇒ 反查无效，
    /// 只有快照差集能覆盖（H3））。
    #[test]
    fn sync_clears_stale_virtual_metadata_and_registers_new_id() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        put_virtual_icon(&mut desk, "shell:控制面板", "控制面板", None);
        put_virtual_icon(&mut desk, "shell:This PC", "This PC", None);
        put_virtual_icon(&mut desk, "shell:Recycle Bin", "Recycle Bin", None);
        // 一个真实文件项：绝不能被虚拟同步碰
        put_virtual_icon(
            &mut desk,
            r"c:\users\me\desktop\a.txt",
            "a.txt",
            Some(r"C:\Users\me\Desktop\a.txt"),
        );

        let mut items = Vec::new();
        let mut item_index = HashMap::new();
        let mut bitmap_ids = HashMap::new();
        let mut pending = Vec::new();
        {
            let mut p = VirtualPool {
                desk: &mut desk,
                items: &mut items,
                item_index: &mut item_index,
                bitmap_ids: &mut bitmap_ids,
            };
            let changed = p.sync(&[bin_snapshot()], &mut pending);
            assert!(changed, "清掉 3 条死元数据 + 注册 1 条新项");
        }

        for dead in ["shell:控制面板", "shell:This PC", "shell:Recycle Bin"] {
            assert!(!desk.icons.contains_key(dead), "{dead} 应被清掉");
        }
        assert!(desk.icons.contains_key(r"c:\users\me\desktop\a.txt"));
        assert!(desk.icons.contains_key("shell:回收站-645ff040"));
        assert_eq!(
            desk.fences[0].icon_ids,
            vec!["shell:回收站-645ff040".to_string()]
        );
        assert!(items.is_empty() || items.iter().all(|it| it.path.is_none()));
    }

    /// 幂等：连续两次同步，第二次 0 变动。
    #[test]
    fn sync_virtual_items_is_idempotent() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        let mut items = Vec::new();
        let mut item_index = HashMap::new();
        let mut bitmap_ids = HashMap::new();
        let mut pending = Vec::new();
        let mut p = VirtualPool {
            desk: &mut desk,
            items: &mut items,
            item_index: &mut item_index,
            bitmap_ids: &mut bitmap_ids,
        };
        assert!(p.sync(&[bin_snapshot()], &mut pending));
        assert!(!p.sync(&[bin_snapshot()], &mut pending), "第二次应 0 变动");
    }

    /// 桌面栅栏不存在：不 panic、不创建栅栏、返回 false。
    #[test]
    fn sync_without_desktop_fence_is_noop() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        let mut items = Vec::new();
        let mut item_index = HashMap::new();
        let mut bitmap_ids = HashMap::new();
        let mut pending = Vec::new();
        let mut p = VirtualPool {
            desk: &mut desk,
            items: &mut items,
            item_index: &mut item_index,
            bitmap_ids: &mut bitmap_ids,
        };
        assert!(!p.sync(&[bin_snapshot()], &mut pending));
        assert!(desk.fences.is_empty(), "不得兜底创建栅栏");
        assert!(desk.icons.is_empty());
    }

    /// H4 锁：虚拟项 `Icon.path` 恒 None、"假装"的解析名不得被当成管理区路径。
    #[test]
    fn virtual_item_path_is_none_and_never_managed() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        let mut items = Vec::new();
        let mut item_index = HashMap::new();
        let mut bitmap_ids = HashMap::new();
        let mut pending = Vec::new();
        {
            let mut p = VirtualPool {
                desk: &mut desk,
                items: &mut items,
                item_index: &mut item_index,
                bitmap_ids: &mut bitmap_ids,
            };
            assert!(p.register(0, &bin_snapshot(), &mut pending));
        }
        let ic = &desk.icons["shell:回收站-645ff040"];
        // H4 锁：path 必须显式 None。它同时锁死三件事——
        // 1. `movable_items`（added && path.is_some()）不会把它当文件搬；
        // 2. `file_extension("::{…}")` 无从切出假扩展名（根本进不了那个函数）；
        // 3. `is_managed_path` 恒 false ⇒「删除」只走原生动词，WinBosk 绝不直删。
        assert!(ic.path.is_none(), "H4：虚拟项 path 必须显式 None");
        assert!(ic.added, "虚拟项非真实桌面文件");
        assert_eq!(
            desk.free_icons.len(),
            0,
            "虚拟项不得落在未分组区（无绘制入口）"
        );
    }

    /// 回归：虚拟项 id **已被启动时的元数据补齐循环登记过**（`added = false`），
    /// 此时 register 的 `already` 快路径不得保留那个错误值。
    ///
    /// 真机实测（第一轮走查）：回收站在 `enumerate_desktop_items()` 结果里，元数据
    /// 补齐先把它写成 `added=false`；随后 `sync_virtual_items` 走 `already` 分支、
    /// 不改 `added` ⇒ desk.json 落盘 `added=false` ⇒ 用户「删除」时走
    /// `move_icon(id, None)` 把回收站丢进无绘制入口的未分组区，图标凭空消失；
    /// Shell 菜单还会注入一个空转的「移出栅栏」。
    #[test]
    fn register_repairs_added_flag_of_preexisting_virtual_entry() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        // 模拟枚举元数据补齐后的状态：键已存在、added=false、无路径
        let mut ic = Icon::new(
            "shell:回收站-645ff040".into(),
            "回收站".into(),
            winbosk_core::model::ItemKind::Unknown,
        );
        ic.path = None;
        ic.added = false;
        desk.icons.insert(ic.id.clone(), ic);
        // items 池也已有这一项（枚举产物）
        let mut items: Vec<DesktopItem> =
            vec![winbosk_shell::virtual_items::virtual_item(&bin_snapshot())];
        let mut item_index: HashMap<String, usize> = HashMap::new();
        item_index.insert("shell:回收站-645ff040".into(), 0);
        let mut bitmap_ids: HashMap<String, u64> = HashMap::new();
        bitmap_ids.insert("shell:回收站-645ff040".into(), 1);

        let mut pending = Vec::new();
        let mut p = VirtualPool {
            desk: &mut desk,
            items: &mut items,
            item_index: &mut item_index,
            bitmap_ids: &mut bitmap_ids,
        };
        assert!(p.register(0, &bin_snapshot(), &mut pending));
        assert!(
            p.desk.icons["shell:回收站-645ff040"].added,
            "虚拟项必须被纠正为 added=true，否则删除时会被丢进未分组区"
        );
        assert!(p.desk.icons["shell:回收站-645ff040"].path.is_none());
        // items 池不得膨胀
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.bitmap_ids.len(), 1, "不得重复占位图槽");
    }

    /// 槽号不与既有槽号重合（即使 `items.len()` 更小）。真实危害：用
    /// `rt.items.len()` 分配时，`remove_icon_entirely` 会把 items 池缩短，
    /// 之后的新项拿到与旧项相同的槽号 → 覆盖旧项已上传的位图。
    #[test]
    fn bitmap_slot_never_collides_with_existing_slot() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        let mut items: Vec<DesktopItem> = Vec::new();
        let mut item_index: HashMap<String, usize> = HashMap::new();
        let mut bitmap_ids: HashMap<String, u64> = HashMap::new();
        // 一个不产生位图的旧项：items.len()==1，但既有槽号是 5
        let mut old = winbosk_shell::virtual_items::virtual_item(&bin_snapshot());
        old.id = "shell:无位图".into();
        item_index.insert(old.id.clone(), items.len());
        items.push(old);
        bitmap_ids.insert("shell:别的".into(), 5);

        let mut pending = Vec::new();
        let mut p = VirtualPool {
            desk: &mut desk,
            items: &mut items,
            item_index: &mut item_index,
            bitmap_ids: &mut bitmap_ids,
        };
        assert!(p.register(0, &bin_snapshot(), &mut pending));
        assert_eq!(
            p.bitmap_ids["shell:回收站-645ff040"], 6,
            "槽号必须避开既有槽号（items.len()=1 会发出冲突的 1）"
        );
        // 重复注册幂等：不膨胀 items 池、不重复追加成员
        assert!(
            !p.register(0, &bin_snapshot(), &mut pending),
            "已在栅栏内 → false"
        );
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.desk.fences[0].icon_ids.len(), 1);
    }

    /// 三块状态各自独立补齐：元数据在、DesktopItem 不在（`validate()` 剔过成员引用后
    /// 又要求恢复）→ register 必须把 items 池补回来，否则右键/双击都摸不到 PIDL。
    #[test]
    fn register_repairs_missing_desktop_item() {
        if winbosk_shell::com::init().is_err() {
            eprintln!("COM init failed, skip");
            return; // 本仓库是 Windows-only，正常不会走到这里
        }
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        let mut ic = Icon::new(
            "shell:回收站-645ff040".into(),
            "回收站".into(),
            winbosk_core::model::ItemKind::Unknown,
        );
        ic.path = None;
        ic.added = true;
        desk.icons.insert(ic.id.clone(), ic);
        // items 池为空（DesktopItem 缺失）
        let mut items: Vec<DesktopItem> = Vec::new();
        let mut item_index: HashMap<String, usize> = HashMap::new();
        let mut bitmap_ids: HashMap<String, u64> = HashMap::new();
        let mut pending = Vec::new();
        let mut p = VirtualPool {
            desk: &mut desk,
            items: &mut items,
            item_index: &mut item_index,
            bitmap_ids: &mut bitmap_ids,
        };
        assert!(p.register(0, &bin_snapshot(), &mut pending));
        assert_eq!(p.items.len(), 1, "DesktopItem 必须被补回池");
        assert_eq!(p.item_index["shell:回收站-645ff040"], 0);
        assert_eq!(p.bitmap_ids["shell:回收站-645ff040"], 1, "位图槽应同步补");
        assert_eq!(pending.len(), 1, "补池的同时必须安排一次位图上传");
    }

    /// 位图槽缺失但 DesktopItem 在（历史数据里没落 bitmap_ids）→ 也要补，
    /// 否则渲染层查不到槽，图标位置空白。
    #[test]
    fn register_repairs_missing_bitmap_slot() {
        if winbosk_shell::com::init().is_err() {
            eprintln!("COM init failed, skip");
            return; // 本仓库是 Windows-only，正常不会走到这里
        }
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        let mut ic = Icon::new(
            "shell:回收站-645ff040".into(),
            "回收站".into(),
            winbosk_core::model::ItemKind::Unknown,
        );
        ic.path = None;
        ic.added = true;
        desk.icons.insert(ic.id.clone(), ic);
        let mut items: Vec<DesktopItem> =
            vec![winbosk_shell::virtual_items::virtual_item(&bin_snapshot())];
        let mut item_index: HashMap<String, usize> = HashMap::new();
        item_index.insert("shell:回收站-645ff040".into(), 0);
        let mut bitmap_ids: HashMap<String, u64> = HashMap::new(); // 缺槽
        let mut pending = Vec::new();
        let mut p = VirtualPool {
            desk: &mut desk,
            items: &mut items,
            item_index: &mut item_index,
            bitmap_ids: &mut bitmap_ids,
        };
        assert!(p.register(0, &bin_snapshot(), &mut pending));
        assert_eq!(p.items.len(), 1, "不得重复 push items");
        assert_eq!(p.bitmap_ids["shell:回收站-645ff040"], 1);
        assert_eq!(pending.len(), 1, "补槽必须伴随一次位图上传");
    }

    /// 移除后重建下标：`item_index` 与 `items` 必须保持一致（否则双击打不开/错位）。
    #[test]
    fn remove_rebuilds_item_index_consistently() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        let mut items: Vec<DesktopItem> = Vec::new();
        let mut item_index: HashMap<String, usize> = HashMap::new();
        let mut bitmap_ids: HashMap<String, u64> = HashMap::new();
        let mut pending = Vec::new();
        let mut p = VirtualPool {
            desk: &mut desk,
            items: &mut items,
            item_index: &mut item_index,
            bitmap_ids: &mut bitmap_ids,
        };
        assert!(p.register(0, &bin_snapshot(), &mut pending));
        p.remove("shell:回收站-645ff040");
        assert!(p.items.is_empty());
        assert!(p.item_index.is_empty());
        assert!(p.bitmap_ids.is_empty());
        assert!(p.desk.icons.is_empty());
        assert!(p.desk.fences[0].icon_ids.is_empty());
    }

    /// 旧口径死元数据 + 新口径并存：同步后旧键从池/items/位图表全清。
    #[test]
    fn stale_entry_is_removed_from_all_three_tables() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        put_virtual_icon(&mut desk, "shell:回收站", "回收站", None);
        desk.fences[0].icon_ids.push("shell:回收站".into());
        // 先在建池之前把旧条目塞进 items / item_index / bitmap_ids（模拟上次运行的池状态）
        let mut items: Vec<DesktopItem> = Vec::new();
        let mut item_index: HashMap<String, usize> = HashMap::new();
        let mut bitmap_ids: HashMap<String, u64> = HashMap::new();
        {
            let mut old = winbosk_shell::virtual_items::virtual_item(&bin_snapshot());
            old.id = "shell:回收站".into();
            let idx = items.len();
            items.push(old);
            item_index.insert("shell:回收站".into(), idx);
            bitmap_ids.insert("shell:回收站".into(), 1);
        }
        let mut pending = Vec::new();
        let mut p = VirtualPool {
            desk: &mut desk,
            items: &mut items,
            item_index: &mut item_index,
            bitmap_ids: &mut bitmap_ids,
        };
        assert!(p.sync(&[bin_snapshot()], &mut pending));
        assert!(!p.desk.icons.contains_key("shell:回收站"));
        assert!(!p.item_index.contains_key("shell:回收站"));
        assert!(!p.bitmap_ids.contains_key("shell:回收站"));
        assert!(!p.items.iter().any(|it| it.id == "shell:回收站"));
        assert_eq!(
            p.desk.fences[0].icon_ids,
            vec!["shell:回收站-645ff040".to_string()]
        );
    }

    /// H1 锁（心跳回归）：虚拟项在场不破坏 `mirror_converged` 的早退。
    /// 虚拟项 `path == None` 被 `mirror_existing_paths` 首句丢弃 ⇒ 不进 `owned` ⇒
    /// 但 `visible`/`all` 也派生自磁盘，两侧子集断言仍成立 ⇒ 仍收敛。
    #[test]
    fn virtual_items_do_not_break_mirror_fast_path() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        let a = r"C:\Users\me\Desktop\a.txt";
        put_virtual_icon(&mut desk, r"c:\users\me\desktop\a.txt", "a.txt", Some(a));
        put_virtual_icon(&mut desk, "shell:回收站-645ff040", "回收站", None);
        desk.fences[0].icon_ids.extend([
            r"c:\users\me\desktop\a.txt".to_string(),
            "shell:回收站-645ff040".into(),
        ]);

        let roots = roots();
        let owned = mirror_existing_paths(&desk, 0, &roots, true);
        let visible = vec![PathBuf::from(a)];
        let all: HashSet<String> = visible
            .iter()
            .map(|p| p.to_string_lossy().to_ascii_lowercase())
            .collect();
        assert!(
            mirror_converged(&owned, &all, &visible),
            "虚拟项在场仍必须收敛，否则 4s 心跳会白跑全量 exists()"
        );
    }

    /// 虚拟项与文件项同池：文件被外部删掉 → 文件项被回收，虚拟项保留。
    #[test]
    fn file_removal_keeps_virtual_items() {
        let mut desk = Desk::new(winbosk_core::config::AppSettings::default());
        desk.fences.push(desktop_fence(1));
        put_virtual_icon(&mut desk, "shell:回收站-645ff040", "回收站", None);
        put_virtual_icon(
            &mut desk,
            r"c:\users\me\desktop\gone.txt",
            "gone.txt",
            Some(r"C:\Users\me\Desktop\gone.txt"),
        );
        desk.fences[0].icon_ids.extend([
            "shell:回收站-645ff040".into(),
            r"c:\users\me\desktop\gone.txt".into(),
        ]);

        // 磁盘上 gone.txt 已不存在（不进 all_set）→ mirror_converged 不成立 → 走回收分支
        let roots = roots();
        let owned = mirror_existing_paths(&desk, 0, &roots, true);
        let all: HashSet<String> = HashSet::new();
        assert!(!mirror_converged(&owned, &all, &[]));

        // 回收分支只按 path 反查并判 exists()：虚拟项 path==None 直接被跳过
        let ids: Vec<String> = desk.icons.keys().cloned().collect();
        let mut removed = Vec::new();
        for id in ids {
            let Some(p) = desk.icons.get(&id).and_then(|ic| ic.path.clone()) else {
                continue;
            };
            if !Path::new(&p).exists() {
                removed.push(id);
            }
        }
        assert_eq!(removed, vec![r"c:\users\me\desktop\gone.txt".to_string()]);
        assert!(desk.icons.contains_key("shell:回收站-645ff040"));
    }
}
