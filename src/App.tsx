import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ClipboardItem, ItemType, Settings, Tab } from "./types";
import TopBar from "./components/TopBar";
import TabBar from "./components/TabBar";
import ItemList from "./components/ItemList";
import StatusBar from "./components/StatusBar";
import SettingsModal from "./components/SettingsModal";
import ClearConfirmModal from "./components/ClearConfirmModal";
import ImageViewer from "./components/ImageViewer";
import FilterModal from "./components/FilterModal";
import Toast from "./components/Toast";

export default function App() {
  const [items, setItems] = useState<ClipboardItem[]>([]);
  const [searchResults, setSearchResults] = useState<ClipboardItem[] | null>(null);
  const [invalidIds, setInvalidIds] = useState<Set<number>>(new Set());
  const [tab, setTab] = useState<Tab>("all");
  const [sortAsc, setSortAsc] = useState(false);
  const [search, setSearch] = useState("");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showClearConfirm, setShowClearConfirm] = useState(false);
  // 查看大图：存被打开记录的原图路径，null = 未打开
  const [viewImagePath, setViewImagePath] = useState<string | null>(null);
  const [showFilter, setShowFilter] = useState(false);
  // 类型筛选：空数组 = 显示全部；勾选 1~2 类则只显示这些类型（切换标签/搜索时保持生效）
  const [typeFilter, setTypeFilter] = useState<ItemType[]>([]);
  const [toast, setToast] = useState<string | null>(null);
  const [multiSelectMode, setMultiSelectMode] = useState(false);
  // 多选已选记录：按选择顺序保存 {id,type}，序号 = 数组下标 + 1；取消后自动重排
  const [selected, setSelected] = useState<{ id: number; type: ItemType }[]>([]);
  const toastTimer = useRef<number | null>(null);

  const showToast = useCallback((msg: string) => {
    setToast(msg);
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 1500);
  }, []);
  useEffect(
    () => () => {
      if (toastTimer.current) window.clearTimeout(toastTimer.current);
    },
    [],
  );

  // 从数据库加载（后端负责收藏过滤与排序；置顶恒最前）
  const loadItems = useCallback(async () => {
    try {
      const list = await invoke<ClipboardItem[]>("get_items", {
        favoriteOnly: tab === "favorites",
        sortAsc,
      });
      setItems(list);
    } catch (e) {
      console.error(e);
      showToast("加载记录失败");
    }
  }, [tab, sortAsc, showToast]);

  useEffect(() => {
    loadItems();
  }, [loadItems]);

  // 实时刷新：新记录入库后后端推送 clipboard-updated
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<ClipboardItem>("clipboard-updated", () => {
      loadItems();
    })
      .then((u) => {
        unlisten = u;
      })
      .catch((e) => console.error("监听事件失败", e));
    return () => {
      unlisten?.();
    };
  }, [loadItems]);

  // FTS5 全文搜索：输入 200ms 防抖后调后端 search_items；清空恢复全部记录
  useEffect(() => {
    const q = search.trim();
    if (!q) {
      setSearchResults(null);
      return;
    }
    const timer = window.setTimeout(() => {
      invoke<ClipboardItem[]>("search_items", { query: q })
        .then(setSearchResults)
        .catch((e) => {
          console.error(e);
          showToast("搜索失败");
        });
    }, 200);
    return () => window.clearTimeout(timer);
  }, [search, showToast]);

  // 展示列表：搜索时用搜索结果（收藏标签下再过滤），最后叠加类型筛选
  const visibleItems = useMemo(() => {
    const list = searchResults ?? items;
    const byTab = tab !== "favorites" ? list : list.filter((i) => i.is_favorite);
    if (typeFilter.length === 0) return byTab;
    return byTab.filter((i) => typeFilter.includes(i.item_type));
  }, [searchResults, items, tab, typeFilter]);

  // 筛选勾选：已选则取消；否则追加（上限 2 项，UI 已禁用，这里是双保险）
  const toggleTypeFilter = useCallback((t: ItemType) => {
    setTypeFilter((prev) => {
      if (prev.includes(t)) return prev.filter((x) => x !== t);
      if (prev.length >= 2) return prev;
      return [...prev, t];
    });
  }, []);

  // 失效检测：对可见的 file 记录惰性检查文件存在性（文档 6.2）
  useEffect(() => {
    const fileItems = items.filter(
      (i) => i.item_type === "file" && i.file_paths && i.file_paths.length > 0,
    );
    if (fileItems.length === 0) {
      setInvalidIds(new Set());
      return;
    }
    const ids = fileItems.map((i) => i.id);
    const pathGroups = fileItems.map((i) => i.file_paths ?? []);
    Promise.all(
      pathGroups.map((p) =>
        invoke<boolean[]>("check_files_exist", { paths: p }).catch(() => []),
      ),
    )
      .then((results) => {
        const inv = new Set<number>();
        results.forEach((exists, idx) => {
          if (exists.some((e) => !e)) inv.add(ids[idx]);
        });
        setInvalidIds(inv);
      })
      .catch((e) => console.error("检查文件存在性失败", e));
  }, [items]);

  useEffect(() => {
    invoke<Settings>("get_settings")
      .then(setSettings)
      .catch((e) => {
        console.error(e);
        showToast("读取设置失败");
      });
  }, [showToast]);

  const saveSettings = async (s: Settings) => {
    const pathChanged = settings ? s.storage_path !== settings.storage_path : false;
    try {
      if (pathChanged) {
        // 数据保存位置改变 → 触发数据迁移（迁移成功后程序重启）
        const msg = await invoke<string>("migrate_storage", { settings: s });
        showToast(msg || "数据已迁移，程序将重启");
        window.setTimeout(() => {
          invoke("relaunch_app").catch((e) => console.error("重启失败", e));
        }, 800);
      } else {
        await invoke("save_settings", { settings: s });
        setSettings(s);
        showToast("设置已保存");
        setShowSettings(false);
      }
    } catch (e) {
      console.error(e);
      showToast(e instanceof Error ? e.message : String(e));
    }
  };

  const copyItem = async (id: number) => {
    try {
      const msg = await invoke<string>("copy_item", { id });
      showToast(msg || "已复制到剪贴板");
    } catch (e) {
      console.error(e);
      showToast("复制失败");
    }
  };
  const toggleFavorite = async (id: number) => {
    try {
      await invoke("toggle_favorite", { id });
      await loadItems();
    } catch (e) {
      console.error(e);
      showToast("操作失败");
    }
  };
  const togglePin = async (id: number) => {
    try {
      await invoke("toggle_pin", { id });
      await loadItems();
    } catch (e) {
      console.error(e);
      showToast("操作失败");
    }
  };
  const deleteItem = async (id: number) => {
    try {
      await invoke("delete_item", { id });
      showToast("已删除");
      await loadItems();
    } catch (e) {
      console.error(e);
      showToast("删除失败");
    }
  };
  // 全部删除：清空全部记录（含图片文件/FTS 索引）
  const clearAll = async () => {
    setShowClearConfirm(false);
    try {
      const n = await invoke<number>("clear_all");
      showToast(`已清空 ${n} 条记录`);
      await loadItems();
    } catch (e) {
      console.error(e);
      showToast("清空失败");
    }
  };
  const revealFile = async (paths: string[]) => {
    if (!paths || paths.length === 0) return;
    try {
      await invoke("reveal_file", { paths });
    } catch (e) {
      console.error(e);
      showToast("定位失败");
    }
  };

  // ---- 多选批量复制（文档 6.3）----
  // 勾选被拒时的提示：updater 内只写 ref（保持纯，避免 StrictMode double-invoke 副作用），
  // 每次点击递增 tick 强制 effect 检查，防止"拦截时 selected 引用未变导致提示不触发"
  const pendingToastRef = useRef<string | null>(null);
  const [toggleTick, setToggleTick] = useState(0);
  useEffect(() => {
    if (pendingToastRef.current) {
      const msg = pendingToastRef.current;
      pendingToastRef.current = null;
      showToast(msg);
    }
  }, [toggleTick, showToast]);

  const toggleSelect = useCallback((item: ClipboardItem) => {
    setSelected((prev) => {
      if (prev.some((s) => s.id === item.id)) {
        return prev.filter((s) => s.id !== item.id); // 取消勾选，后续重新编号
      }
      // 组互斥：文本 与 文件/图片 不可混选（文件+图片可混选）
      const group = (t: ItemType) => (t === "text" ? "text" : "file-image");
      if (prev.length > 0 && group(prev[0].type) !== group(item.item_type)) {
        pendingToastRef.current = "文本不能与文件/图片混选";
        return prev;
      }
      // 已选图片 ≥10 时，禁止再选文件（含文件组上限 10，提前拦截）
      if (item.item_type === "file") {
        const imgCount = prev.filter((s) => s.type === "image").length;
        if (imgCount >= 10) {
          pendingToastRef.current = "选择的数量高于最大值，目前禁止选择文件";
          return prev;
        }
      }
      // 上限：含文件 → 10；纯文本/纯图片 → 50
      const willHaveFile = item.item_type === "file" || prev.some((s) => s.type === "file");
      const limit = willHaveFile ? 10 : 50;
      if (prev.length >= limit) {
        pendingToastRef.current = willHaveFile ? "最多只能选择 10 条" : "最多只能选择 50 条";
        return prev;
      }
      return [...prev, { id: item.id, type: item.item_type }];
    });
    setToggleTick((t) => t + 1); // 每次点击递增，触发 effect 检查拦截提示
  }, []);

  const completeMultiSelect = async () => {
    if (selected.length === 0) {
      showToast("未选择任何记录");
      setMultiSelectMode(false);
      setSelected([]);
      return;
    }
    try {
      const msg = await invoke<string>("batch_copy", { ids: selected.map((s) => s.id) });
      showToast(msg || `${selected.length} 项已复制`);
    } catch (e) {
      console.error(e);
      showToast(e instanceof Error ? e.message : String(e));
    } finally {
      setMultiSelectMode(false);
      setSelected([]);
    }
  };

  // 顶部"多选/完成"按钮：非多选 → 进入；多选 → 执行批量复制并退出
  const handleTopBarMulti = () => {
    if (multiSelectMode) {
      completeMultiSelect();
    } else {
      setSelected([]);
      setMultiSelectMode(true);
    }
  };

  // 选中序号映射：id → 选择顺序（1 起）；未选不在映射。跨搜索/排序/标签保持
  const selectIndex = useMemo(() => {
    const m = new Map<number, number>();
    selected.forEach((s, i) => m.set(s.id, i + 1));
    return m;
  }, [selected]);

  // ---- 固定顶部/底部布局：顶部栏/底部栏钉死在视口，列表在中间独立滚动 ----
  // 用内联 style 实现（不依赖任何全局样式表），并通过测量 body 边距保持界面位置与现状完全一致
  const headerRef = useRef<HTMLDivElement>(null);
  const statusRef = useRef<HTMLDivElement>(null);
  const [headerH, setHeaderH] = useState(110);
  const [statusH, setStatusH] = useState(30);
  // 视口四周留白（body 默认 margin），用于根容器定位补偿，保证视觉零变化
  const [inset, setInset] = useState({ top: 8, left: 8, right: 8, bottom: 8 });
  useLayoutEffect(() => {
    const update = () => {
      if (headerRef.current) setHeaderH(headerRef.current.offsetHeight);
      if (statusRef.current) setStatusH(statusRef.current.offsetHeight);
      // 读 body 的 CSS margin（四边），不受内容高度塌陷影响
      const cs = getComputedStyle(document.body);
      const px = (v: string) => parseFloat(v) || 0;
      setInset({
        top: px(cs.marginTop),
        left: px(cs.marginLeft),
        right: px(cs.marginRight),
        bottom: px(cs.marginBottom),
      });
    };
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);

  return (
    <div
      className="relative h-full bg-bg-primary"
      style={{ position: "fixed", top: inset.top, left: inset.left, right: inset.right, bottom: inset.bottom, overflow: "hidden" }}
    >
      {/* 顶部工具栏：钉死在根容器顶部，永不随列表滚动 */}
      <div
        ref={headerRef}
        className="fixed top-0 left-0 right-0 z-30 bg-bg-primary"
        style={{ position: "absolute", top: 0, left: 0, right: 0, zIndex: 30 }}
      >
        <TopBar
          search={search}
          onSearch={setSearch}
          multiSelect={multiSelectMode}
          selectedCount={selected.length}
          onToggleMultiSelect={handleTopBarMulti}
          onOpenSettings={() => setShowSettings(true)}
          onOpenFilter={() => setShowFilter(true)}
          filterActive={typeFilter.length > 0}
          onClearAll={() => setShowClearConfirm(true)}
        />
        <TabBar tab={tab} onTab={setTab} sortAsc={sortAsc} onToggleSort={() => setSortAsc((v) => !v)} />
      </div>
      {/* 列表滚动区：从顶部工具栏下方到状态栏上方，内部独立滚动 */}
      <div
        className="absolute left-0 right-0 overflow-y-auto"
        style={{ position: "absolute", left: 0, right: 0, top: headerH, bottom: statusH, overflowY: "auto", borderTop: "1px solid #000" }}
      >
        <ItemList
          items={visibleItems}
          invalidIds={invalidIds}
          multiSelect={multiSelectMode}
          selectIndex={selectIndex}
          onToggleSelect={toggleSelect}
          onCopy={copyItem}
          onFavorite={toggleFavorite}
          onPin={togglePin}
          onDelete={deleteItem}
          onReveal={revealFile}
          onOpenImage={setViewImagePath}
        />
      </div>
      {/* 底部状态栏：钉死在根容器底部 */}
      <div
        ref={statusRef}
        className="fixed bottom-0 left-0 right-0 z-30 bg-bg-primary"
        style={{ position: "absolute", bottom: 0, left: 0, right: 0, zIndex: 30 }}
      >
        <StatusBar count={visibleItems.length} />
      </div>
      {showSettings && settings && (
        <SettingsModal
          settings={settings}
          onCancel={() => setShowSettings(false)}
          onSave={saveSettings}
          onRequestMove={() => {}} // 数据迁移在点"保存"时触发
        />
      )}
      {showFilter && (
        <FilterModal
          selected={typeFilter}
          onToggle={toggleTypeFilter}
          onReset={() => setTypeFilter([])}
          onClose={() => setShowFilter(false)}
        />
      )}
      {showClearConfirm && (
        <ClearConfirmModal
          onCancel={() => setShowClearConfirm(false)}
          onConfirm={clearAll}
        />
      )}
      {/* 大图查看器：最上层覆盖，Esc / 点图片外区域 / 右上角 × 关闭 */}
      {viewImagePath && <ImageViewer path={viewImagePath} onClose={() => setViewImagePath(null)} />}
      <Toast message={toast} />
    </div>
  );
}
