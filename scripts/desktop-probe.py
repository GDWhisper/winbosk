"""只读探测：打印「真实 Windows 桌面」当前显示的图标清单（ground truth）。

为什么需要它：WinBosk 用 overlay 接管桌面后，真实桌面图标被 `IconGuard` 隐藏，肉眼与截图都
无法判断「桌面上原本应该有哪些图标」。这个脚本直接读 explorer 的桌面 `SysListView32`
（`LVM_GETITEMCOUNT` + `LVM_GETITEMTEXTW`），拿到 Windows 认为的桌面项全集——**只读**，
不发送任何修改类消息，也不触碰文件。

用法（在 Windows 上，任意 Python 3）：

    python scripts/desktop-probe.py            # 打印图标清单与总数
    python scripts/desktop-probe.py --json     # 机器可读，便于与 desk.json 对比

典型用途：校验桌面镜像栅栏的内容是否与真实桌面一致（用户桌面文件 + 公共桌面文件 + 已启用的
虚拟壳项，如「回收站」）。注意这是**跨进程**读取：需要在 explorer 进程内分配缓冲区。

已知边界：
- 桌面的「显示桌面图标」被关闭时，列表为空（此时不具参考价值）；
- `SysListView32` 不存在（被销毁/极端 shell 替换）时脚本给出提示并退出码 1；
- 用户右键「刷新」前后顺序可能变化，**不要依赖顺序**，只做集合比较。
"""

import ctypes
import json
import sys
from ctypes import wintypes

u32 = ctypes.windll.user32
k32 = ctypes.windll.kernel32

LVM_FIRST = 0x1000
LVM_GETITEMCOUNT = LVM_FIRST + 4
LVM_GETITEMTEXTW = LVM_FIRST + 115
LVIF_TEXT = 0x0001
PROCESS_VM_OPERATION = 0x0008
PROCESS_VM_READ = 0x0010
PROCESS_VM_WRITE = 0x0020
MEM_COMMIT = 0x1000
MEM_RESERVE = 0x2000
MEM_RELEASE = 0x8000
PAGE_READWRITE = 0x04


class LVITEMW(ctypes.Structure):
    _fields_ = [
        ("mask", wintypes.UINT),
        ("iItem", ctypes.c_int),
        ("iSubItem", ctypes.c_int),
        ("state", wintypes.UINT),
        ("stateMask", wintypes.UINT),
        ("pszText", ctypes.c_void_p),
        ("cchTextMax", ctypes.c_int),
        ("iImage", ctypes.c_int),
        ("lParam", ctypes.c_void_p),
        ("iIndent", ctypes.c_int),
        ("iGroupId", ctypes.c_int),
        ("cColumns", wintypes.UINT),
        ("puColumns", ctypes.c_void_p),
        ("piColFmt", ctypes.c_void_p),
        ("iGroup", ctypes.c_int),
    ]


def find_desktop_listview():
    """桌面图标视图：Progman（或动态壁纸场景下的 WorkerW）→ SHELLDLL_DefView → SysListView32。"""
    parents = []
    progman = u32.FindWindowW("Progman", None)
    if progman:
        parents.append(progman)

    WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    def enum_cb(hwnd, _):
        cls = ctypes.create_unicode_buffer(64)
        u32.GetClassNameW(hwnd, cls, 64)
        if cls.value == "WorkerW":
            parents.append(hwnd)
        return True

    u32.EnumWindows(WNDENUMPROC(enum_cb), 0)
    for p in parents:
        dv = u32.FindWindowExW(p, None, "SHELLDLL_DefView", None)
        if dv:
            lv = u32.FindWindowExW(dv, None, "SysListView32", None)
            if lv:
                return lv
    return None


def read_titles(lv):
    pid = wintypes.DWORD()
    u32.GetWindowThreadProcessId(lv, ctypes.byref(pid))
    h = k32.OpenProcess(
        PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE, False, pid.value
    )
    if not h:
        raise OSError(f"OpenProcess(explorer pid={pid.value}) 失败: {k32.GetLastError()}")
    try:
        n = u32.SendMessageW(lv, LVM_GETITEMCOUNT, 0, 0)
        remote = k32.VirtualAllocEx(h, None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
        if not remote:
            raise OSError("VirtualAllocEx 失败")
        try:
            remote_text = remote + 512
            names = []
            for i in range(n):
                item = LVITEMW()
                item.mask = LVIF_TEXT
                item.iItem = i
                item.iSubItem = 0
                item.pszText = ctypes.c_void_p(remote_text)
                item.cchTextMax = 260
                k32.WriteProcessMemory(
                    h, remote, ctypes.byref(item), ctypes.sizeof(item), None
                )
                u32.SendMessageW(lv, LVM_GETITEMTEXTW, i, remote)
                buf = ctypes.create_unicode_buffer(260)
                read = ctypes.c_size_t()
                k32.ReadProcessMemory(h, remote_text, buf, ctypes.sizeof(buf), ctypes.byref(read))
                names.append(buf.value)
            return names
        finally:
            k32.VirtualFreeEx(h, remote, 0, MEM_RELEASE)
    finally:
        k32.CloseHandle(h)


def main():
    lv = find_desktop_listview()
    if not lv:
        print("未找到桌面 SysListView32（桌面视图不可用）", file=sys.stderr)
        return 1
    names = read_titles(lv)
    if "--json" in sys.argv:
        print(json.dumps({"count": len(names), "names": names}, ensure_ascii=False, indent=2))
    else:
        print(f"真实桌面图标数 = {len(names)}")
        for i, n in enumerate(names):
            print(f"  [{i:3}] {n}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
