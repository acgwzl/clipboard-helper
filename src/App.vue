<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick, watch } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import QRCode from "qrcode";

interface ClipItem {
  id: number;
  content_type: "text" | "image";
  text: string | null;
  image_path: string | null;
  pinned: boolean;
  created_at: number;
  copy_count: number;
  tags: string[];
  image_width: number | null;
  image_height: number | null;
  image_bytes: number | null;
  sort_order: number | null;
  thumb_path: string | null;
}

interface Stats {
  total: number;
  text_count: number;
  image_count: number;
  pinned_count: number;
  total_copies: number;
  top_items: ClipItem[];
}

type Filter = "all" | "text" | "image" | "pinned";
type Theme = "archive" | "archive-dark";
type Group = "今天" | "昨天" | "本周" | "本月" | "更早";
type TimeFilter = "any" | "today" | "yesterday" | "week" | "month" | "older";

interface PrivacySettings {
  enabled: boolean;
  ignore_incognito: boolean;
  sensitive_title_keywords: string[];
  ignored_apps: string[];
  paused_until: number | null;
}

interface PrivacyStatus {
  settings: PrivacySettings;
  paused_remaining_ms: number;
  active_app: string | null;
  active_title: string | null;
  blocking_reason: string | null;
}

interface OcrCrop {
  x: number;
  y: number;
  width: number;
  height: number;
}

const THEMES: { id: Theme; name: string; color: string; kind: "light" | "dark" }[] = [
  { id: "archive",      name: "档案",   color: "#c14a2e", kind: "light" },
  { id: "archive-dark", name: "档案·夜", color: "#e06a4a", kind: "dark" },
];

// ----- state -----
const items = ref<ClipItem[]>([]);
const search = ref("");
const selectedIndex = ref(0);
const filter = ref<Filter>("all");
const timeFilter = ref<TimeFilter>("any");
const minCopies = ref<number>(0);
const regexMode = ref(false);
const caseSensitive = ref(false);
const selectedTag = ref("");
const autoTagFilter = ref("");
const qrItem = ref<ClipItem | null>(null);
const qrDataUrl = ref("");
const ocrItem = ref<ClipItem | null>(null);
const ocrText = ref("");
const ocrBusy = ref(false);
const ocrMessage = ref("");
const ocrTool = ref<"select" | "pan">("select");
const ocrSelection = ref<OcrCrop | null>(null);
const ocrDragStart = ref<{ x: number; y: number } | null>(null);
const ocrPanStart = ref<{ x: number; y: number; scrollLeft: number; scrollTop: number } | null>(null);
const ocrImageEl = ref<HTMLImageElement | null>(null);
const ocrViewport = ref<HTMLElement | null>(null);
const ocrZoom = ref(1);
const ocrBaseScale = ref(1);
const ocrNaturalSize = ref({ width: 0, height: 0 });
const advancedOpen = ref(localStorage.getItem("advancedOpen") === "true");
const searchError = ref("");
const VALID_THEMES: Theme[] = ["archive", "archive-dark"];
const storedTheme = localStorage.getItem("theme") as Theme | null;
const theme = ref<Theme>(storedTheme && VALID_THEMES.includes(storedTheme) ? storedTheme : "archive");
const pinned = ref<boolean>(localStorage.getItem("pinned") === "true");
const themePickerOpen = ref(false);
const menuOpen = ref(false);
const draggedItemId = ref<number | null>(null);
const dropTargetId = ref<number | null>(null);
const lightboxItem = ref<ClipItem | null>(null);
const searchInput = ref<HTMLInputElement | null>(null);
let unlistenFns: UnlistenFn[] = [];
const now = ref(Date.now());

const selectMode = ref(false);
const selectedIds = ref<Set<number>>(new Set());

const groupBy = ref<boolean>(localStorage.getItem("groupBy") === "true");
const miniMode = ref<boolean>(localStorage.getItem("miniMode") === "true");

const dragOver = ref(false);

const settingsOpen = ref(false);
const statsOpen = ref(false);
const helpOpen = ref(false);
const tagEditorItem = ref<ClipItem | null>(null);
const tagEditorText = ref("");

// 新建片段 modal
const snippetEditorOpen = ref(false);
const snippetText = ref("");
const snippetTags = ref("");
const privacyStatus = ref<PrivacyStatus | null>(null);
const ignoredAppsText = ref("");
const sensitiveKeywordsText = ref("");

// 拖拽调试（短暂显示在底部，帮助排查）
const dragDebug = ref("");

const hotkeyValue = ref("");
const hotkeyEditing = ref(false);
const hotkeyError = ref("");

const stats = ref<Stats | null>(null);

const ocrSelectionStyle = computed(() => {
  const s = ocrSelection.value;
  if (!s) return {};
  return {
    left: `${s.x}px`,
    top: `${s.y}px`,
    width: `${s.width}px`,
    height: `${s.height}px`,
  };
});
const ocrStageStyle = computed(() => {
  const natural = ocrNaturalSize.value;
  if (!natural.width || !natural.height) return {};
  const scale = ocrBaseScale.value * ocrZoom.value;
  return {
    width: `${Math.max(1, Math.round(natural.width * scale))}px`,
    height: `${Math.max(1, Math.round(natural.height * scale))}px`,
  };
});
const ocrZoomLabel = computed(() => `${Math.round(ocrZoom.value * 100)}%`);
const hasOcrText = computed(() => ocrText.value.trim().length > 0);

watch(theme, (v) => {
  document.documentElement.dataset.theme = v;
  localStorage.setItem("theme", v);
});
watch(pinned, async (v) => {
  localStorage.setItem("pinned", String(v));
  await invoke("set_window_pin", { pin: v });
});
watch(groupBy, (v) => localStorage.setItem("groupBy", String(v)));
watch(advancedOpen, (v) => localStorage.setItem("advancedOpen", String(v)));

function pickTheme(t: Theme) {
  theme.value = t;
  themePickerOpen.value = false;
}

async function togglePinned() {
  pinned.value = !pinned.value;
}

// 收藏视图的显示顺序:手动拖拽过的(有 sort_order)按值升序在前,其余按时间倒序
function sortPinned(list: ClipItem[]): ClipItem[] {
  return [...list].sort((a, b) => {
    const ao = a.sort_order, bo = b.sort_order;
    if (ao != null && bo != null && ao !== bo) return ao - bo;
    if (ao != null && bo == null) return -1;
    if (ao == null && bo != null) return 1;
    return b.created_at - a.created_at;
  });
}

const filtered = computed(() => {
  let list = items.value;
  if (filter.value === "pinned") {
    list = sortPinned(list.filter((i) => i.pinned));
  } else {
    // 收藏项在普通视图也显示
    if (filter.value === "text") list = list.filter((i) => i.content_type === "text");
    else if (filter.value === "image") list = list.filter((i) => i.content_type === "image");
  }
  if (timeFilter.value !== "any") list = list.filter((i) => timeMatches(i.created_at, timeFilter.value));
  if (minCopies.value > 0) list = list.filter((i) => (i.copy_count || 0) >= minCopies.value);
  if (selectedTag.value) list = list.filter((i) => (i.tags || []).some((t) => t.toLowerCase() === selectedTag.value.toLowerCase()));
  if (autoTagFilter.value) list = list.filter((i) => autoTags(i).includes(autoTagFilter.value));

  const q = search.value.trim();
  searchError.value = "";
  if (q) {
    if (regexMode.value) {
      try {
        const re = new RegExp(q, caseSensitive.value ? "" : "i");
        list = list.filter((i) => i.content_type === "text" && !!i.text && re.test(i.text));
      } catch (e: any) {
        searchError.value = e?.message || "正则无效";
        list = [];
      }
    } else {
      const needle = caseSensitive.value ? q : q.toLowerCase();
      list = list.filter((i) => {
        const hay = [i.text || "", ...(i.tags || [])].join(" ");
        const target = caseSensitive.value ? hay : hay.toLowerCase();
        return target.includes(needle);
      });
    }
  }
  return list;
});

const counts = computed(() => ({
  all: items.value.length,
  text: items.value.filter((i) => i.content_type === "text").length,
  image: items.value.filter((i) => i.content_type === "image").length,
  pinned: items.value.filter((i) => i.pinned).length,
}));

const allTags = computed(() => {
  const map = new Map<string, number>();
  for (const item of items.value) {
    for (const tag of item.tags || []) {
      const clean = tag.trim();
      if (!clean) continue;
      map.set(clean, (map.get(clean) || 0) + 1);
    }
  }
  return [...map.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0], "zh-CN"))
    .map(([name, count]) => ({ name, count }));
});

const miniTagFilters = computed(() => allTags.value.slice(0, 8));

const hasAdvancedFilters = computed(() =>
  timeFilter.value !== "any" ||
  minCopies.value > 0 ||
  regexMode.value ||
  caseSensitive.value ||
  !!selectedTag.value ||
  !!autoTagFilter.value
);

function resetAdvancedFilters() {
  timeFilter.value = "any";
  minCopies.value = 0;
  regexMode.value = false;
  caseSensitive.value = false;
  selectedTag.value = "";
  autoTagFilter.value = "";
}

const metrics = computed(() => {
  const d0 = new Date(); d0.setHours(0, 0, 0, 0);
  const t0 = d0.getTime();
  let today = 0, copies = 0, pinned = 0, image = 0;
  for (const it of items.value) {
    if (it.created_at >= t0) today++;
    copies += it.copy_count || 0;
    if (it.pinned) pinned++;
    if (it.content_type === "image") image++;
  }
  return { today, pinned, image, copies };
});

function groupOf(ts: number): Group {
  const today0 = new Date(); today0.setHours(0, 0, 0, 0);
  const t0 = today0.getTime();
  const yesterday0 = t0 - 24 * 3600 * 1000;
  const week0 = t0 - 6 * 24 * 3600 * 1000;
  const month0 = new Date(); month0.setDate(month0.getDate() - 30); month0.setHours(0,0,0,0);
  if (ts >= t0)               return "今天";
  if (ts >= yesterday0)       return "昨天";
  if (ts >= week0)            return "本周";
  if (ts >= month0.getTime()) return "本月";
  return "更早";
}

const grouped = computed(() => {
  if (!groupBy.value) return [] as { name: string; items: ClipItem[] }[];
  const map = new Map<Group, ClipItem[]>();
  const order: Group[] = ["今天", "昨天", "本周", "本月", "更早"];
  for (const g of order) map.set(g, []);
  for (const it of filtered.value) {
    map.get(groupOf(it.created_at))?.push(it);
  }
  const out: { name: string; items: ClipItem[] }[] = [];
  for (const g of order) {
    const arr = map.get(g)!;
    if (arr.length > 0) out.push({ name: g, items: arr });
  }
  return out;
});

const groupedByTag = computed(() => {
  if (!selectedTag.value || !groupBy.value) return [] as { name: string; items: ClipItem[] }[];
  return [{ name: `#${selectedTag.value}`, items: filtered.value }];
});

const flatIndexed = computed(() => {
  if (groupBy.value && selectedTag.value) return groupedByTag.value.flatMap((g) => g.items);
  if (groupBy.value) return grouped.value.flatMap((g) => g.items);
  return filtered.value;
});

const miniItems = computed(() => flatIndexed.value.slice(0, 5));

function isSelected(item: ClipItem): boolean {
  return flatIndexed.value[selectedIndex.value]?.id === item.id;
}

// 图片经 Tauri asset 协议直出(WebView 按需加载并自带缓存),列表优先用缩略图
function imgSrc(item: ClipItem): string {
  return convertFileSrc(item.thumb_path || item.image_path || "");
}
function imgFullSrc(item: ClipItem): string {
  return convertFileSrc(item.image_path || "");
}

async function refresh() {
  items.value = await invoke<ClipItem[]>("get_items");
  if (selectedIndex.value >= flatIndexed.value.length) {
    selectedIndex.value = Math.max(0, flatIndexed.value.length - 1);
  }
}

function timeMatches(ts: number, value: TimeFilter): boolean {
  const today0 = new Date(); today0.setHours(0, 0, 0, 0);
  const t0 = today0.getTime();
  const yesterday0 = t0 - 24 * 3600 * 1000;
  const week0 = t0 - 6 * 24 * 3600 * 1000;
  const month0 = new Date(); month0.setDate(month0.getDate() - 30); month0.setHours(0,0,0,0);
  if (value === "today") return ts >= t0;
  if (value === "yesterday") return ts >= yesterday0 && ts < t0;
  if (value === "week") return ts >= week0;
  if (value === "month") return ts >= month0.getTime();
  if (value === "older") return ts < month0.getTime();
  return true;
}

function searchRegex(): RegExp | null {
  const q = search.value.trim();
  if (!q) return null;
  try {
    if (regexMode.value) return new RegExp(q, caseSensitive.value ? "g" : "gi");
    return new RegExp(escapeRegExp(q), caseSensitive.value ? "g" : "gi");
  } catch {
    return null;
  }
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function highlightedParts(text: string | null | undefined, limit = 180): { text: string; hit: boolean }[] {
  const source = preview(text, limit);
  const re = searchRegex();
  if (!re || !source) return [{ text: source, hit: false }];
  const parts: { text: string; hit: boolean }[] = [];
  let last = 0;
  for (const match of source.matchAll(re)) {
    const start = match.index ?? 0;
    const hit = match[0];
    if (!hit) continue;
    if (start > last) parts.push({ text: source.slice(last, start), hit: false });
    parts.push({ text: hit, hit: true });
    last = start + hit.length;
  }
  if (last < source.length) parts.push({ text: source.slice(last), hit: false });
  return parts.length ? parts : [{ text: source, hit: false }];
}

function openTagEditor(item: ClipItem) {
  tagEditorItem.value = item;
  tagEditorText.value = (item.tags || []).join(", ");
}

async function saveTags() {
  if (!tagEditorItem.value) return;
  const tags = tagEditorText.value
    .split(/[，,\n]/)
    .map((t) => normalizeTag(t))
    .filter(Boolean);
  await invoke("set_tags", { id: tagEditorItem.value.id, tags });
  tagEditorItem.value = null;
  await refresh();
}

function openSnippetEditor() {
  snippetText.value = "";
  snippetTags.value = "";
  snippetEditorOpen.value = true;
}

async function saveSnippet() {
  if (!snippetText.value.trim()) {
    showToast("片段内容不能为空");
    return;
  }
  const tags = snippetTags.value
    .split(/[，,\n]/)
    .map((t) => normalizeTag(t))
    .filter(Boolean);
  await invoke("create_snippet", { text: snippetText.value, tags });
  snippetEditorOpen.value = false;
  await refresh();
  showToast("片段已创建");
}

// 拖拽排序(仅收藏视图)
function onDragStart(item: ClipItem, e: DragEvent) {
  if (!item.pinned) return; // 只允许收藏条目拖拽
  draggedItemId.value = item.id;
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = "move";
  }
}

function onDragOver(item: ClipItem, e: DragEvent) {
  if (!draggedItemId.value || !item.pinned) return;
  e.preventDefault();
  dropTargetId.value = item.id;
  if (e.dataTransfer) {
    e.dataTransfer.dropEffect = "move";
  }
}

function onDragLeave() {
  dropTargetId.value = null;
}

async function onDrop(targetItem: ClipItem, e: DragEvent) {
  e.preventDefault();
  if (!draggedItemId.value || draggedItemId.value === targetItem.id) {
    draggedItemId.value = null;
    dropTargetId.value = null;
    return;
  }
  // 重新排序:基于收藏视图的「显示顺序」计算位置(items 本身按时间排)
  const pinnedItems = sortPinned(items.value.filter(it => it.pinned));
  const dragIdx = pinnedItems.findIndex(it => it.id === draggedItemId.value);
  const dropIdx = pinnedItems.findIndex(it => it.id === targetItem.id);
  if (dragIdx === -1 || dropIdx === -1) return;

  const [dragged] = pinnedItems.splice(dragIdx, 1);
  pinnedItems.splice(dropIdx, 0, dragged);

  // 生成新的 sort_order 映射(从0开始)
  const orders: Array<[number, number]> = pinnedItems.map((it, idx) => [it.id, idx]);
  await invoke("update_sort_order", { orders });
  await refresh();
  draggedItemId.value = null;
  dropTargetId.value = null;
}

function onDragEnd() {
  draggedItemId.value = null;
  dropTargetId.value = null;
}

function normalizeTag(tag: string): string {
  return tag.trim().replace(/^#/, "").slice(0, 24);
}

function visibleTags(item: ClipItem, limit = 2): string[] {
  return (item.tags || []).slice(0, limit);
}

async function pickItem(item: ClipItem, autoPaste = true) {
  if (selectMode.value) {
    toggleSelect(item.id);
    return;
  }
  // 片段占位符替换:文本条目且含占位符 → 替换后用 paste_text
  if (item.content_type === "text" && item.text) {
    const placeholders = hasPlaceholders(item.text);
    if (placeholders.length > 0) {
      const replaced = await replacePlaceholders(item.text);
      await invoke("paste_text", { text: replaced, autoPaste });
      await invoke("increment_copy_count", { id: item.id });
      await refresh();
      return;
    }
  }
  await invoke("pick_item", { id: item.id, autoPaste });
}

async function togglePinClip(item: ClipItem) {
  await invoke("toggle_pin", { id: item.id });
  await refresh();
}

async function deleteItem(item: ClipItem) {
  await invoke("delete_item", { id: item.id });
  if (lightboxItem.value?.id === item.id) lightboxItem.value = null;
  await refresh();
}

async function clearAll() {
  if (!confirm("清空所有未收藏记录？收藏项会保留。")) return;
  await invoke("clear_history");
  await refresh();
}

function openLightbox(item: ClipItem) {
  if (item.content_type === "image") lightboxItem.value = item;
}

function scrollIntoView() {
  nextTick(() => {
    document.querySelector<HTMLElement>(".item.selected")
      ?.scrollIntoView({ block: "nearest" });
  });
}

function toggleSelectMode() {
  selectMode.value = !selectMode.value;
  if (!selectMode.value) selectedIds.value = new Set();
}
function toggleSelect(id: number) {
  const s = new Set(selectedIds.value);
  if (s.has(id)) s.delete(id); else s.add(id);
  selectedIds.value = s;
}
function selectAllVisible() {
  selectedIds.value = new Set(flatIndexed.value.map((i) => i.id));
}
function selectNone() { selectedIds.value = new Set(); }

async function batchDelete() {
  const ids = [...selectedIds.value];
  if (ids.length === 0) return;
  if (!confirm(`删除选中的 ${ids.length} 条？`)) return;
  await invoke("batch_delete", { ids });
  selectedIds.value = new Set();
  await refresh();
}
async function batchPin(p: boolean) {
  const ids = [...selectedIds.value];
  if (ids.length === 0) return;
  await invoke("batch_pin", { ids, pinned: p });
  selectedIds.value = new Set();
  await refresh();
}

// 选中项按可视（flatIndexed）顺序排列的 id 列表
function orderedSelectedIds(): number[] {
  return flatIndexed.value.filter((i) => selectedIds.value.has(i.id)).map((i) => i.id);
}

// 前 9 条的 1-based 序号，用于 Alt+数字 快速粘贴的角标
function quickIndex(item: ClipItem): number | null {
  const i = flatIndexed.value.findIndex((x) => x.id === item.id);
  return i >= 0 && i < 9 ? i + 1 : null;
}

// 占位符检测与替换
function hasPlaceholders(text: string): string[] {
  const found: string[] = [];
  if (/\{\{date\}\}/i.test(text)) found.push("date");
  if (/\{\{time\}\}/i.test(text)) found.push("time");
  if (/\{\{clipboard\}\}/i.test(text)) found.push("clipboard");
  return found;
}

async function replacePlaceholders(text: string): Promise<string> {
  let out = text;
  const now = new Date();
  out = out.replace(/\{\{date\}\}/gi, now.toISOString().slice(0, 10)); // YYYY-MM-DD
  out = out.replace(/\{\{time\}\}/gi, now.toTimeString().slice(0, 8)); // HH:MM:SS
  // {{clipboard}} 从系统剪贴板读
  if (/\{\{clipboard\}\}/i.test(out)) {
    try {
      const clip = await navigator.clipboard.readText();
      out = out.replace(/\{\{clipboard\}\}/gi, clip);
    } catch {
      out = out.replace(/\{\{clipboard\}\}/gi, "");
    }
  }
  return out;
}

// 合并选中项文本为一条并粘贴
async function mergeSelected() {
  const ids = orderedSelectedIds();
  if (ids.length === 0) return;
  const texts = ids
    .map((id) => items.value.find((i) => i.id === id))
    .filter((i): i is ClipItem => !!i && i.content_type === "text" && !!i.text)
    .map((i) => i.text as string);
  if (texts.length === 0) { showToast("选中项没有可合并的文本"); return; }
  await invoke("paste_text", { text: texts.join("\n"), autoPaste: true });
  selectMode.value = false;
  selectedIds.value = new Set();
  await refresh();
}

// 按可视顺序依次粘贴选中项到当前应用（填表）
async function pasteSequence() {
  const ids = orderedSelectedIds();
  if (ids.length === 0) return;
  await invoke("paste_sequence", { ids, delayMs: 250 });
  selectMode.value = false;
  selectedIds.value = new Set();
  await refresh();
}

async function exportAll() {
  try {
    const path = await saveDialog({
      defaultPath: `clipboard-export-${new Date().toISOString().slice(0,10)}.json`,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const n = await invoke<number>("export_history", { path });
    alert(`已导出 ${n} 条到\n${path}`);
  } catch (e: any) {
    alert("导出失败: " + e);
  }
}
async function importAll() {
  try {
    const path = await openDialog({
      multiple: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path || typeof path !== "string") return;
    const n = await invoke<number>("import_history", { path });
    alert(`已导入 ${n} 条`);
    await refresh();
  } catch (e: any) {
    alert("导入失败: " + e);
  }
}

function captureHotkey(e: KeyboardEvent) {
  e.preventDefault();
  e.stopPropagation();
  if (["Control","Shift","Alt","Meta"].includes(e.key)) return;
  let main = e.key;
  if (main.length === 1) main = main.toUpperCase();
  if (main === " ") main = "Space";
  // 无强修饰键的裸键会吞掉全系统同名按键,禁止录入(功能键 F1-F24 例外)
  const isFnKey = /^F([1-9]|1\d|2[0-4])$/.test(main);
  if (!(e.ctrlKey || e.altKey || e.metaKey) && !isFnKey) {
    hotkeyError.value = "需包含 Ctrl / Alt / Win 修饰键(F1-F24 可单独使用)";
    return;
  }
  hotkeyError.value = "";
  const parts: string[] = [];
  if (e.ctrlKey)  parts.push("Ctrl");
  if (e.shiftKey) parts.push("Shift");
  if (e.altKey)   parts.push("Alt");
  if (e.metaKey)  parts.push("Meta");
  parts.push(main);
  hotkeyValue.value = parts.join("+");
}
async function saveHotkey() {
  hotkeyError.value = "";
  try {
    await invoke("set_hotkey", { combo: hotkeyValue.value });
    hotkeyEditing.value = false;
  } catch (e: any) {
    hotkeyError.value = "无效组合: " + e;
  }
}

async function loadPrivacyStatus() {
  try {
    privacyStatus.value = await invoke<PrivacyStatus>("get_privacy_status");
    ignoredAppsText.value = (privacyStatus.value.settings.ignored_apps || []).join(", ");
    sensitiveKeywordsText.value = (privacyStatus.value.settings.sensitive_title_keywords || []).join(", ");
  } catch {
    privacyStatus.value = null;
  }
}

async function savePrivacy() {
  if (!privacyStatus.value) return;
  const settings: PrivacySettings = {
    ...privacyStatus.value.settings,
    ignored_apps: splitList(ignoredAppsText.value),
    sensitive_title_keywords: splitList(sensitiveKeywordsText.value),
  };
  privacyStatus.value = await invoke<PrivacyStatus>("set_privacy_settings", { settings });
  ignoredAppsText.value = privacyStatus.value.settings.ignored_apps.join(", ");
  sensitiveKeywordsText.value = privacyStatus.value.settings.sensitive_title_keywords.join(", ");
  showToast("隐私规则已保存");
}

async function pauseClipboard(minutes: number) {
  privacyStatus.value = await invoke<PrivacyStatus>("pause_clipboard", { minutes });
  if (privacyStatus.value) {
    ignoredAppsText.value = privacyStatus.value.settings.ignored_apps.join(", ");
    sensitiveKeywordsText.value = privacyStatus.value.settings.sensitive_title_keywords.join(", ");
  }
  showToast(minutes > 0 ? `已暂停 ${minutes} 分钟` : "已恢复监听");
}

function splitList(text: string): string[] {
  return text
    .split(/[，,\n]/)
    .map((x) => x.trim())
    .filter(Boolean);
}

function pauseLabel(ms: number): string {
  if (ms <= 0) return "未暂停";
  const min = Math.ceil(ms / 60_000);
  return `剩余 ${min} 分钟`;
}

async function openStats() {
  stats.value = await invoke<Stats>("get_stats");
  statsOpen.value = true;
  menuOpen.value = false;
}

// ----- 三栏布局辅助 -----
// ---- 档案主题结构元素 ----
const brandSub = computed(() => `ARCHIVE NO. ${String(items.value[0]?.id ?? 0).padStart(4, "0")}`);

function serialOf(item: ClipItem): string {
  return String(item.id % 10000).padStart(4, "0");
}

// 条目"类型章":档案语言的汉字铅字(收藏=藏)
function themedGlyph(item: ClipItem): string {
  if (item.pinned) return "藏";
  if (item.content_type === "image") return "图";
  if (detectUrl(item.text)) return "链";
  if (detectEmail(item.text)) return "邮";
  if (detectColor(item.text)) return "色";
  if (looksJson(item.text)) return "码";
  if (detectPath(item.text)) return "档";
  return "文";
}

function detectUrl(text: string | null | undefined): string | null {
  if (!text) return null;
  const match = text.trim().match(/https?:\/\/[^\s<>"')]+/i);
  return match?.[0] || null;
}

function urlInfo(text: string | null | undefined): { url: string; host: string; path: string } | null {
  const raw = detectUrl(text);
  if (!raw) return null;
  try {
    const u = new URL(raw);
    const path = `${u.pathname}${u.search}`.replace(/\/$/, "");
    return { url: raw, host: u.hostname.replace(/^www\./, ""), path: path || "/" };
  } catch {
    return null;
  }
}

function looksJson(text: string | null | undefined): boolean {
  const t = (text || "").trim();
  if (!/^[\[{]/.test(t)) return false;
  try { JSON.parse(t); return true; } catch { return false; }
}

function jsonSummary(text: string | null | undefined): string {
  try {
    const parsed = JSON.parse((text || "").trim());
    if (Array.isArray(parsed)) return `Array(${parsed.length})`;
    if (parsed && typeof parsed === "object") return `Object(${Object.keys(parsed).length})`;
    return typeof parsed;
  } catch {
    return "JSON";
  }
}

function detectCodeLanguage(text: string | null | undefined): string | null {
  const t = (text || "").trim();
  if (!t) return null;
  if (/^(npm|yarn|pnpm|git|cargo|cd|sudo|docker|node|python|pip|curl)\b/m.test(t)) return "Shell";
  if (/^(import|export)\s|\bconst\s+\w+\s*=|=>|console\.log/.test(t)) return "JavaScript";
  if (/<template>|<script setup|<\/[a-z][\s\S]*>/i.test(t)) return "Vue / HTML";
  if (/^\s*(fn|use|impl|pub struct)\b/m.test(t)) return "Rust";
  if (/^\s*(def|class|from|import)\b/m.test(t) && /:\s*$/m.test(t)) return "Python";
  if (/^\s*[{[]/.test(t)) return null;
  return null;
}

function typeLabel(item: ClipItem): string {
  if (item.content_type === "image") return "图片";
  const t = (item.text || "").trim();
  if (/^[\[{]/.test(t)) return "JSON / 结构";
  if (/^https?:\/\//i.test(t)) return "链接";
  if (/^(npm|yarn|pnpm|git|cargo|cd|sudo|docker|node|python|pip|curl)\b/.test(t)) return "命令";
  if (/^([a-zA-Z]:\\|\/|\.\/|~\/)/.test(t)) return "路径";
  return "文本";
}

function detailLabel(item: ClipItem): string {
  if (item.content_type === "image") {
    const dims = item.image_width && item.image_height ? `${item.image_width}×${item.image_height}` : "图片";
    const size = item.image_bytes ? formatBytes(item.image_bytes) : "";
    return [dims, size].filter(Boolean).join(" · ");
  }
  const url = urlInfo(item.text);
  if (url) return `${url.host}${url.path !== "/" ? " · " + url.path.slice(0, 46) : ""}`;
  if (looksJson(item.text)) return jsonSummary(item.text);
  const lang = detectCodeLanguage(item.text);
  if (lang) return lang;
  return "";
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

async function copyPlain(item: ClipItem) {
  const text = item.content_type === "text" ? (item.text || "") : (item.image_path || "");
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    showToast("已复制纯文本");
  } catch {
    await invoke("add_text", { text });
    await refresh();
  }
}

async function openItemUrl(item: ClipItem) {
  const url = detectUrl(item.text);
  if (!url) return;
  await openUrl(url);
}

async function prettyJsonAction(item: ClipItem) {
  if (!item.text) return;
  try {
    const out = await invoke<string>("transform_text", { op: "json_pretty", text: item.text });
    await navigator.clipboard.writeText(out);
    showToast("已复制格式化 JSON");
  } catch (e: any) {
    showToast("JSON 无效: " + e);
  }
}

async function copyDomain(item: ClipItem) {
  const info = urlInfo(item.text);
  if (!info) return;
  await navigator.clipboard.writeText(info.host);
  showToast("已复制域名");
}

function showToast(message: string) {
  dragDebug.value = message;
  setTimeout(() => { if (dragDebug.value === message) dragDebug.value = ""; }, 1800);
}

/* ===================== Track 3 内容智能 ===================== */
function detectEmail(text: string | null | undefined): string | null {
  if (!text) return null;
  const m = text.trim().match(/\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/);
  return m?.[0] || null;
}
function detectColor(text: string | null | undefined): string | null {
  if (!text) return null;
  const t = text.trim();
  return /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$|^rgba?\([^)]*\)$|^hsla?\([^)]*\)$/.test(t) ? t : null;
}
function detectPath(text: string | null | undefined): string | null {
  const t = (text || "").trim();
  if (!t || /[\r\n]/.test(t)) return null;
  if (/^[a-zA-Z]:\\/.test(t) || /^\\\\/.test(t)) return t;
  return null;
}
function looksNumber(text: string | null | undefined): boolean {
  const t = (text || "").trim();
  if (!t || t.length > 40) return false;
  return /^[+\-(]?\d[\d\s().,\-+]*$/.test(t) && (t.match(/\d/g)?.length || 0) >= 3;
}

// 虚拟自动标签：由检测器派生，不落库
function autoTags(item: ClipItem): string[] {
  if (item.content_type !== "text") return [];
  const t = item.text || "";
  const out: string[] = [];
  if (detectUrl(t)) out.push("链接");
  if (looksJson(t)) out.push("JSON");
  if (detectCodeLanguage(t)) out.push("代码");
  if (detectEmail(t)) out.push("邮箱");
  if (detectColor(t)) out.push("颜色");
  if (detectPath(t)) out.push("路径");
  if (looksNumber(t)) out.push("数字");
  return out;
}
function toggleAutoTag(name: string) {
  autoTagFilter.value = autoTagFilter.value === name ? "" : name;
}

async function mailtoItem(item: ClipItem) {
  const email = detectEmail(item.text);
  if (!email) return;
  try { await openUrl(`mailto:${email}`); } catch (e: any) { showToast("打开失败：" + e); }
}
async function revealItem(item: ClipItem) {
  const p = detectPath(item.text);
  if (!p) return;
  try { await invoke("reveal_path", { path: p }); }
  catch (e: any) { showToast("定位失败：" + e); }
}

// 二维码
async function openQr(item: ClipItem) {
  const text = item.content_type === "text" ? (item.text || "") : (item.image_path || "");
  if (!text) { showToast("没有可生成的内容"); return; }
  try {
    qrDataUrl.value = await QRCode.toDataURL(text, { margin: 1, width: 260, errorCorrectionLevel: "M" });
    qrItem.value = item;
  } catch (e: any) {
    showToast("内容过长，无法生成二维码");
  }
}

// OCR 文字识别：先弹窗确认，用户确认后再加入列表
function extractText(item: ClipItem) {
  if (item.content_type !== "image" || !item.image_path) {
    showToast("仅支持图片条目");
    return;
  }
  ocrItem.value = item;
  ocrText.value = "";
  ocrTool.value = "select";
  ocrSelection.value = null;
  ocrDragStart.value = null;
  ocrPanStart.value = null;
  ocrZoom.value = 1;
  ocrBaseScale.value = 1;
  ocrNaturalSize.value = { width: 0, height: 0 };
  ocrMessage.value = "可拖动图片框选区域，或直接识别整图。";
}

function closeOcrDialog() {
  if (ocrBusy.value) return;
  ocrItem.value = null;
  ocrText.value = "";
  ocrTool.value = "select";
  ocrSelection.value = null;
  ocrDragStart.value = null;
  ocrPanStart.value = null;
  ocrZoom.value = 1;
  ocrBaseScale.value = 1;
  ocrNaturalSize.value = { width: 0, height: 0 };
  ocrMessage.value = "";
}

function onOcrImageLoad(e: Event) {
  const img = e.currentTarget as HTMLImageElement;
  ocrNaturalSize.value = {
    width: img.naturalWidth,
    height: img.naturalHeight,
  };
  nextTick(() => fitOcrImage(false));
}

function fitOcrImage(keepSelection = true) {
  const natural = ocrNaturalSize.value;
  const viewport = ocrViewport.value;
  if (!natural.width || !natural.height || !viewport) return;
  const crop = keepSelection ? currentOcrCrop() : null;
  const maxW = Math.max(220, viewport.clientWidth - 6);
  const maxH = Math.max(220, viewport.clientHeight - 6);
  ocrBaseScale.value = Math.min(1, maxW / natural.width, maxH / natural.height);
  ocrZoom.value = 1;
  restoreOcrCrop(crop);
}

function restoreOcrCrop(crop: OcrCrop | null) {
  if (!crop) return;
  nextTick(() => {
    const img = ocrImageEl.value;
    if (!img?.naturalWidth || !img.clientWidth) return;
    const scale = img.clientWidth / img.naturalWidth;
    ocrSelection.value = {
      x: crop.x * scale,
      y: crop.y * scale,
      width: crop.width * scale,
      height: crop.height * scale,
    };
  });
}

function setOcrZoom(next: number) {
  const crop = currentOcrCrop();
  ocrZoom.value = Math.max(0.5, Math.min(6, next));
  restoreOcrCrop(crop);
}

function zoomOcr(delta: number) {
  setOcrZoom(ocrZoom.value + delta);
}

function onOcrWheel(e: WheelEvent) {
  if (!e.ctrlKey) return;
  e.preventDefault();
  zoomOcr(e.deltaY < 0 ? 0.25 : -0.25);
}

function setOcrTool(tool: "select" | "pan") {
  if (ocrBusy.value) return;
  ocrTool.value = tool;
  ocrDragStart.value = null;
  ocrPanStart.value = null;
  ocrMessage.value = tool === "pan"
    ? "拖动模式：按住图片平移，定位后可切回框选。"
    : "框选模式：拖动图片区域画出要识别的范围。";
}

function ocrPoint(e: PointerEvent): { x: number; y: number } {
  const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
  const x = Math.max(0, Math.min(rect.width, e.clientX - rect.left));
  const y = Math.max(0, Math.min(rect.height, e.clientY - rect.top));
  return { x, y };
}

function startOcrPan(e: PointerEvent) {
  const viewport = ocrViewport.value;
  if (!viewport) return;
  e.preventDefault();
  ocrPanStart.value = {
    x: e.clientX,
    y: e.clientY,
    scrollLeft: viewport.scrollLeft,
    scrollTop: viewport.scrollTop,
  };
  (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
}

function moveOcrPan(e: PointerEvent) {
  const start = ocrPanStart.value;
  const viewport = ocrViewport.value;
  if (!start || !viewport) return;
  viewport.scrollLeft = start.scrollLeft - (e.clientX - start.x);
  viewport.scrollTop = start.scrollTop - (e.clientY - start.y);
}

function endOcrPan() {
  if (ocrPanStart.value) {
    ocrPanStart.value = null;
  }
}

function onOcrSelectStart(e: PointerEvent) {
  if (ocrBusy.value) return;
  if (ocrTool.value === "pan" || e.button === 1) {
    startOcrPan(e);
    return;
  }
  if (e.button !== 0) return;
  e.preventDefault();
  const p = ocrPoint(e);
  ocrDragStart.value = p;
  ocrSelection.value = { x: p.x, y: p.y, width: 0, height: 0 };
  (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
}

function onOcrSelectMove(e: PointerEvent) {
  if (ocrPanStart.value) {
    moveOcrPan(e);
    return;
  }
  if (!ocrDragStart.value || ocrBusy.value) return;
  const p = ocrPoint(e);
  const start = ocrDragStart.value;
  ocrSelection.value = {
    x: Math.min(start.x, p.x),
    y: Math.min(start.y, p.y),
    width: Math.abs(p.x - start.x),
    height: Math.abs(p.y - start.y),
  };
}

function onOcrSelectEnd(e: PointerEvent) {
  if (ocrPanStart.value) {
    endOcrPan();
    return;
  }
  if (!ocrDragStart.value) return;
  onOcrSelectMove(e);
  ocrDragStart.value = null;
  const s = ocrSelection.value;
  if (!s || s.width < 8 || s.height < 8) {
    ocrSelection.value = null;
    ocrMessage.value = "选区太小，已取消。可重新拖动框选。";
  } else {
    ocrMessage.value = "选区已就绪，可以识别选区。";
  }
}

function clearOcrSelection() {
  ocrSelection.value = null;
  ocrDragStart.value = null;
  ocrMessage.value = "已清除选区，可识别整图或重新框选。";
}

function currentOcrCrop(): OcrCrop | null {
  const s = ocrSelection.value;
  const img = ocrImageEl.value;
  if (!s || !img || !img.naturalWidth || !img.naturalHeight || !img.clientWidth || !img.clientHeight) {
    return null;
  }
  const scaleX = img.naturalWidth / img.clientWidth;
  const scaleY = img.naturalHeight / img.clientHeight;
  return {
    x: Math.max(0, Math.round(s.x * scaleX)),
    y: Math.max(0, Math.round(s.y * scaleY)),
    width: Math.max(1, Math.round(s.width * scaleX)),
    height: Math.max(1, Math.round(s.height * scaleY)),
  };
}

async function recognizeOcr(scope: "all" | "selection") {
  const item = ocrItem.value;
  if (!item?.image_path || ocrBusy.value) return;
  const crop = scope === "selection" ? currentOcrCrop() : null;
  if (scope === "selection" && !crop) {
    ocrMessage.value = "请先在图片上拖动框选要识别的区域。";
    return;
  }
  ocrBusy.value = true;
  ocrMessage.value = scope === "selection" ? "正在识别选区..." : "正在识别整图...";
  try {
    ocrText.value = await invoke<string>("extract_text_from_image", {
      imagePath: item.image_path,
      crop,
    });
    ocrMessage.value = "请确认或编辑识别结果，然后加入列表。";
  } catch (e: any) {
    ocrMessage.value = e || "识别失败";
  } finally {
    ocrBusy.value = false;
  }
}

async function addOcrText() {
  const text = ocrText.value.trim();
  if (!text || ocrBusy.value) return;
  await invoke("add_text", { text });
  closeOcrDialog();
  await refresh();
  showToast("识别文字已加入列表");
}

async function copyOcrText() {
  const text = ocrText.value.trim();
  if (!text) return;
  await navigator.clipboard.writeText(text);
  showToast("识别文字已复制");
}

async function winMin() { try { await getCurrentWindow().minimize(); } catch {} }
async function winToggleMax() { try { await getCurrentWindow().toggleMaximize(); } catch {} }

async function setMiniMode(next: boolean) {
  miniMode.value = next;
  localStorage.setItem("miniMode", String(next));
  selectMode.value = false;
  selectedIds.value = new Set();
  groupBy.value = next ? false : groupBy.value;
  await nextTick();
  searchInput.value?.focus();
  try {
    await invoke("set_window_mode", { mini: next });
  } catch (e: any) {
    showToast("窗口尺寸切换失败: " + e);
  }
}

async function toggleMiniMode() {
  await setMiniMode(!miniMode.value);
}

async function setupDragDrop() {
  const handleDrop = async (paths: string[], source: string) => {
    dragOver.value = false;
    dragDebug.value = `处理中(${source}): ${paths.length} 个`;
    let ok = 0, fail = 0;
    for (const path of paths) {
      try {
        await invoke("add_file_path", { path });
        ok++;
      } catch (err) {
        console.error("drop add_file_path failed:", path, err);
        fail++;
      }
    }
    dragDebug.value = `已加入 ${ok} 条${fail > 0 ? `，失败 ${fail}` : ""}`;
    setTimeout(() => { dragDebug.value = ""; }, 2500);
    await refresh();
  };

  // 原生 Tauri 事件
  unlistenFns.push(await listen<any>("tauri://drag-enter", (e) => {
    dragOver.value = true;
    dragDebug.value = `拖入(t): ${(e.payload?.paths || []).length} 个`;
  }));
  unlistenFns.push(await listen<any>("tauri://drag-over", () => { dragOver.value = true; }));
  unlistenFns.push(await listen<any>("tauri://drag-leave", () => {
    dragOver.value = false;
    dragDebug.value = "已移出";
    setTimeout(() => { if (dragDebug.value === "已移出") dragDebug.value = ""; }, 1500);
  }));
  unlistenFns.push(await listen<any>("tauri://drag-drop", async (e) => {
    const paths: string[] = e.payload?.paths || [];
    await handleDrop(paths, "tauri");
  }));

  // Rust 端兜底事件
  unlistenFns.push(await listen<string[]>("app-drag-enter", (e) => {
    dragOver.value = true;
    dragDebug.value = `拖入(r): ${e.payload?.length || 0} 个`;
  }));
  unlistenFns.push(await listen("app-drag-over", () => { dragOver.value = true; }));
  unlistenFns.push(await listen("app-drag-leave", () => {
    dragOver.value = false;
    dragDebug.value = "已移出";
    setTimeout(() => { if (dragDebug.value === "已移出") dragDebug.value = ""; }, 1500);
  }));
  unlistenFns.push(await listen<string[]>("app-drag-drop", async (e) => {
    const paths: string[] = e.payload || [];
    await handleDrop(paths, "rust");
  }));
}

function onKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && (e.key === "m" || e.key === "M")) {
    e.preventDefault();
    toggleMiniMode();
    return;
  }
  if ((e.ctrlKey || e.metaKey) && (e.key === "k" || e.key === "K")) {
    e.preventDefault();
    searchInput.value?.focus();
    searchInput.value?.select();
    return;
  }
  if (helpOpen.value) {
    if (e.key === "Escape") helpOpen.value = false;
    return;
  }
  if (qrItem.value) {
    if (e.key === "Escape") qrItem.value = null;
    return;
  }
  if (ocrItem.value) {
    if (e.key === "Escape") closeOcrDialog();
    return;
  }
  if (tagEditorItem.value) {
    if (e.key === "Escape") tagEditorItem.value = null;
    return;
  }
  if (settingsOpen.value || statsOpen.value) {
    if (e.key === "Escape") {
      if (hotkeyEditing.value) { hotkeyEditing.value = false; return; }
      settingsOpen.value = false;
      statsOpen.value = false;
    }
    return;
  }
  if (lightboxItem.value) {
    if (e.key === "Escape") {
      lightboxItem.value = null;
    } else if (e.key === "Enter") {
      e.preventDefault();
      const t = lightboxItem.value;
      lightboxItem.value = null;
      if (t) pickItem(t, !e.ctrlKey);
    }
    return;
  }
  if (themePickerOpen.value && e.key === "Escape") {
    themePickerOpen.value = false; return;
  }
  if (menuOpen.value && e.key === "Escape") {
    menuOpen.value = false; return;
  }

  // 焦点在工具区输入框/文本域时（命令条除外），交还原生输入，避免触发列表导航
  const el = e.target as HTMLElement | null;
  const inSearch = el === searchInput.value;
  const inField = !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
  if (inField && !inSearch) {
    if (e.key === "Escape" && el) el.blur();
    return;
  }

  // Alt+1–9：快速粘贴对应序号的条目（带 Alt 修饰，焦点在搜索框也不冲突）
  if (e.altKey && !e.ctrlKey && !e.metaKey && /^[1-9]$/.test(e.key)) {
    const item = flatIndexed.value[parseInt(e.key, 10) - 1];
    if (item) {
      e.preventDefault();
      pickItem(item, true);
    }
    return;
  }

  // “/” 聚焦搜索框
  if (e.key === "/" && !inSearch) {
    e.preventDefault();
    searchInput.value?.focus();
    searchInput.value?.select();
    return;
  }

  if (e.key === "Escape") {
    if (selectMode.value) { selectMode.value = false; selectedIds.value = new Set(); return; }
    invoke("hide_window");
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    selectedIndex.value = Math.min(flatIndexed.value.length - 1, selectedIndex.value + 1);
    scrollIntoView();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    selectedIndex.value = Math.max(0, selectedIndex.value - 1);
    scrollIntoView();
  } else if (e.key === "Enter") {
    e.preventDefault();
    const item = flatIndexed.value[selectedIndex.value];
    if (item) pickItem(item, !e.ctrlKey);
  } else if (e.key === " ") {
    if (inSearch) return;
    e.preventDefault();
    const item = flatIndexed.value[selectedIndex.value];
    if (item) openLightbox(item);
  } else if (e.key === "Tab") {
    e.preventDefault();
    const order: Filter[] = ["all", "text", "image", "pinned"];
    filter.value = order[(order.indexOf(filter.value) + 1) % order.length];
    selectedIndex.value = 0;
  }
}

function onGlobalClick(e: MouseEvent) {
  const target = e.target as HTMLElement;
  if (themePickerOpen.value && !target.closest(".theme-switcher")) themePickerOpen.value = false;
  if (menuOpen.value && !target.closest(".menu-wrap")) menuOpen.value = false;
}

function preview(text: string | null | undefined, limit = 140): string {
  if (!text) return "";
  const single = text.replace(/\s+/g, " ").trim();
  return single.length > limit ? single.slice(0, limit) + "…" : single;
}

function relativeTime(ts: number): string {
  const diff = Math.max(0, now.value - ts);
  const s = Math.floor(diff / 1000);
  if (s < 5) return "刚刚";
  if (s < 60) return `${s} 秒前`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分钟前`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} 小时前`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d} 日前`;
  return new Date(ts).toLocaleDateString("zh-CN");
}

watch(filter, () => (selectedIndex.value = 0));
watch(search, () => (selectedIndex.value = 0));
watch([timeFilter, minCopies, regexMode, caseSensitive, selectedTag, autoTagFilter], () => (selectedIndex.value = 0));

onMounted(async () => {
  document.documentElement.dataset.theme = theme.value;
  await invoke("set_window_pin", { pin: pinned.value }).catch(() => {});
  if (miniMode.value) await setMiniMode(true);
  await refresh();
  await loadPrivacyStatus();
  try { hotkeyValue.value = await invoke<string>("get_hotkey"); } catch { hotkeyValue.value = "Ctrl+Shift+V"; }
  unlistenFns.push(await listen("clips-changed", () => refresh()));
  await setupDragDrop();
  window.addEventListener("keydown", onKeydown);
  window.addEventListener("click", onGlobalClick);
  searchInput.value?.focus();
  const timer = setInterval(() => {
    now.value = Date.now();
    loadPrivacyStatus();
  }, 30_000);
  onUnmounted(() => clearInterval(timer));
});

onUnmounted(() => {
  unlistenFns.forEach((u) => u());
  window.removeEventListener("keydown", onKeydown);
  window.removeEventListener("click", onGlobalClick);
});

listen("tauri://focus", () => {
  searchInput.value?.focus();
  search.value = "";
  selectedIndex.value = 0;
});
</script>

<template>
  <div class="app" :class="{ 'drag-over': dragOver, 'mini-mode': miniMode }">
    <!-- icon sprite -->
    <svg width="0" height="0" style="position:absolute" aria-hidden="true">
      <symbol id="i-term" viewBox="0 0 24 24"><path d="M5 7l5 5-5 5"/><path d="M13 17h6"/></symbol>
      <symbol id="i-search" viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></symbol>
      <symbol id="i-star" viewBox="0 0 24 24"><path d="M12 3.6l2.6 5.3 5.8.8-4.2 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8-4.2-4.1 5.8-.8z"/></symbol>
      <symbol id="i-trash" viewBox="0 0 24 24"><path d="M4 7h16"/><path d="M9 7V5a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v2"/><path d="M6 7l1 13a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-13"/><path d="M10 11v6M14 11v6"/></symbol>
      <symbol id="i-gear" viewBox="0 0 24 24"><circle cx="12" cy="12" r="3.2"/><path d="M19.4 13.5a7.5 7.5 0 0 0 0-3l2-1.5-2-3.4-2.3 1a7.5 7.5 0 0 0-2.6-1.5L14 1.5h-4l-.5 2.6a7.5 7.5 0 0 0-2.6 1.5l-2.3-1-2 3.4 2 1.5a7.5 7.5 0 0 0 0 3l-2 1.5 2 3.4 2.3-1a7.5 7.5 0 0 0 2.6 1.5L10 22.5h4l.5-2.6a7.5 7.5 0 0 0 2.6-1.5l2.3 1 2-3.4z"/></symbol>
      <symbol id="i-down" viewBox="0 0 24 24"><path d="M12 4v10M8 11l4 4 4-4"/><path d="M5 19h14"/></symbol>
      <symbol id="i-up" viewBox="0 0 24 24"><path d="M12 20V10M8 13l4-4 4 4"/><path d="M5 5h14"/></symbol>
      <symbol id="i-return" viewBox="0 0 24 24"><path d="M9 10l-4 4 4 4"/><path d="M5 14h10a4 4 0 0 0 4-4V6"/></symbol>
      <symbol id="i-min" viewBox="0 0 24 24"><path d="M5 12h14"/></symbol>
      <symbol id="i-max" viewBox="0 0 24 24"><rect x="5" y="5" width="14" height="14" rx="2"/></symbol>
      <symbol id="i-close" viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></symbol>
      <symbol id="i-dots" viewBox="0 0 24 24"><circle cx="5" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="19" cy="12" r="1.6"/></symbol>
      <symbol id="i-help" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M9.2 9.3a2.8 2.8 0 0 1 5.4 1c0 1.9-2.6 2.3-2.6 4"/><path d="M12 17.2v.01"/></symbol>
      <symbol id="i-grid" viewBox="0 0 24 24"><rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/></symbol>
      <symbol id="i-check" viewBox="0 0 24 24"><path d="M5 12.5l4.2 4.2L19 7"/></symbol>
      <symbol id="i-circle" viewBox="0 0 24 24"><circle cx="12" cy="12" r="7"/></symbol>
      <symbol id="i-qr" viewBox="0 0 24 24"><rect x="4" y="4" width="6" height="6" rx="1"/><rect x="14" y="4" width="6" height="6" rx="1"/><rect x="4" y="14" width="6" height="6" rx="1"/><path d="M14 14h2v2h-2zM18 14h2v6h-6v-2h4zM14 18h2"/></symbol>
      <symbol id="i-ocr" viewBox="0 0 24 24"><path d="M5 8V5h3M16 5h3v3M19 16v3h-3M8 19H5v-3"/><path d="M8 10h8M8 14h5"/></symbol>
    </svg>

    <!-- ===== 顶栏 ===== -->
    <header class="topbar">
      <div class="brand">
        <span class="mark" aria-hidden="true"><svg class="ic"><use href="#i-term"/></svg></span>
        <div class="brand-text">
          <h1>剪贴板助手</h1>
          <span class="sub">{{ brandSub }}</span>
        </div>
      </div>

      <div class="command">
        <span class="lead" aria-hidden="true"><svg class="ic"><use href="#i-search"/></svg></span>
        <input ref="searchInput" v-model="search" class="cmd-input" placeholder="搜索、转换或粘贴…" />
        <span v-if="search" class="cmd-clear" @click="search = ''">×</span>
        <span v-else class="kbd" aria-hidden="true">/</span>
      </div>

      <div class="top-actions">
        <div class="theme-switcher">
          <button class="topbtn icon" @click.stop="themePickerOpen = !themePickerOpen" title="主题">
            <span class="theme-dot" :style="{ background: THEMES.find(t => t.id === theme)?.color }"></span>
          </button>
          <div v-if="themePickerOpen" class="theme-popover">
            <div class="theme-section-label">浅</div>
            <button
              v-for="t in THEMES.filter(x => x.kind === 'light')"
              :key="t.id"
              :class="['theme-row', { active: theme === t.id }]"
              @click.stop="pickTheme(t.id)"
            >
              <span class="theme-row-dot" :style="{ background: t.color }"></span>
              <span class="theme-row-name">{{ t.name }}</span>
              <span class="theme-row-id">{{ t.id }}</span>
            </button>
            <div class="theme-section-label">深</div>
            <button
              v-for="t in THEMES.filter(x => x.kind === 'dark')"
              :key="t.id"
              :class="['theme-row', { active: theme === t.id }]"
              @click.stop="pickTheme(t.id)"
            >
              <span class="theme-row-dot" :style="{ background: t.color }"></span>
              <span class="theme-row-name">{{ t.name }}</span>
              <span class="theme-row-id">{{ t.id }}</span>
            </button>
          </div>
        </div>

        <button class="topbtn" @click="openSnippetEditor()" title="创建可复用片段"><svg class="ic"><use href="#i-star"/></svg>片段</button>
        <button class="topbtn icon" @click="helpOpen = true" title="帮助"><svg class="ic"><use href="#i-help"/></svg></button>
        <button class="topbtn icon" @click="loadPrivacyStatus(); settingsOpen = true" title="设置"><svg class="ic"><use href="#i-gear"/></svg></button>
        <button class="topbtn" @click="importAll()"><svg class="ic"><use href="#i-down"/></svg>导入</button>
        <button class="topbtn" @click="exportAll()"><svg class="ic"><use href="#i-up"/></svg>导出</button>
        <button class="topbtn mini-toggle-btn" :class="{ active: miniMode }" @click="toggleMiniMode">{{ miniMode ? '完整' : '小窗' }}</button>

        <div class="menu-wrap" v-if="!miniMode">
          <button class="topbtn icon" @click.stop="menuOpen = !menuOpen" title="更多"><svg class="ic"><use href="#i-dots"/></svg></button>
          <div v-if="menuOpen" class="menu-popover">
            <button class="menu-row" @click.stop="groupBy = !groupBy; menuOpen = false">
              <span class="menu-glyph">日</span><span>{{ groupBy ? '取消日期分组' : '按日期分组' }}</span>
            </button>
            <button class="menu-row" @click.stop="openStats">
              <span class="menu-glyph">▤</span><span>详细统计</span>
            </button>
            <div class="menu-sep"></div>
            <button class="menu-row danger" @click.stop="clearAll(); menuOpen = false">
              <span class="menu-glyph">⌫</span><span>清空未收藏</span>
            </button>
          </div>
        </div>

        <div class="winctrls">
          <button class="winbtn" @click="winMin()" title="最小化"><svg class="ic"><use href="#i-min"/></svg></button>
          <button class="winbtn" @click="winToggleMax()" title="最大化 / 还原"><svg class="ic"><use href="#i-max"/></svg></button>
          <button class="winbtn close" @click="invoke('hide_window')" title="关闭"><svg class="ic"><use href="#i-close"/></svg></button>
        </div>
      </div>
    </header>

    <!-- ===== 工作区 ===== -->
    <div class="workspace">
      <!-- 左：视图 + 指标 -->
      <aside v-if="!miniMode" class="side">
        <section class="section">
          <h2 class="eyebrow">视图</h2>
          <div class="nav">
            <button
              v-for="f in (['all','text','image','pinned'] as Filter[])"
              :key="f"
              :class="['chip', { active: filter === f }]"
              @click="filter = f"
            >
              <span class="chip-l">{{ { all: '全部', text: '文本', image: '图片', pinned: '收藏' }[f] }}</span>
              <b>{{ counts[f] }}</b>
            </button>
          </div>
        </section>

        <section class="section">
          <h2 class="eyebrow">
            筛选
            <button class="eyebrow-link" @click="advancedOpen = !advancedOpen">{{ advancedOpen ? '收起' : '高级' }}</button>
          </h2>
          <div v-if="searchError" class="filter-error">{{ searchError }}</div>
          <div v-if="advancedOpen" class="filter-panel">
            <label class="filter-row">
              <span>时间</span>
              <select v-model="timeFilter" class="field-mini">
                <option value="any">不限</option>
                <option value="today">今天</option>
                <option value="yesterday">昨天</option>
                <option value="week">本周</option>
                <option value="month">本月</option>
                <option value="older">更早</option>
              </select>
            </label>
            <label class="filter-row">
              <span>复制 ≥</span>
              <input v-model.number="minCopies" class="field-mini" type="number" min="0" max="999" />
            </label>
            <div class="filter-toggles">
              <button :class="['mini-toggle', { active: regexMode }]" @click="regexMode = !regexMode">正则</button>
              <button :class="['mini-toggle', { active: caseSensitive }]" @click="caseSensitive = !caseSensitive">区分大小写</button>
            </div>
            <button v-if="hasAdvancedFilters" class="linkbtn clear-filters" @click="resetAdvancedFilters">清除高级筛选</button>
          </div>
        </section>

        <section class="section" v-if="allTags.length">
          <h2 class="eyebrow">标签<button v-if="selectedTag" class="eyebrow-link" @click="selectedTag = ''">全部</button></h2>
          <div class="tag-cloud">
            <button
              v-for="tag in allTags"
              :key="tag.name"
              :class="['tag-filter', { active: selectedTag === tag.name }]"
              @click="selectedTag = selectedTag === tag.name ? '' : tag.name"
            >
              #{{ tag.name }} <b>{{ tag.count }}</b>
            </button>
          </div>
        </section>

        <section class="section">
          <h2 class="eyebrow">指标<button class="eyebrow-link" @click="openStats">详情</button></h2>
          <div class="stats">
            <div class="stat"><span>今日</span><b>{{ metrics.today }}</b></div>
            <div class="stat"><span>收藏</span><b>{{ metrics.pinned }}</b></div>
            <div class="stat"><span>图片</span><b>{{ metrics.image }}</b></div>
            <div class="stat"><span>复制</span><b>{{ metrics.copies }}</b></div>
          </div>
        </section>

        <div class="side-bottom">
          <button class="topbtn block" :class="{ active: selectMode }" @click="toggleSelectMode">
            <svg class="ic"><use href="#i-grid"/></svg>{{ selectMode ? '退出多选' : '批量选择' }}
          </button>
          <button class="toggle" @click="togglePinned">
            <span>窗口置顶</span>
            <span class="switch" :class="{ on: pinned }" aria-hidden="true"></span>
          </button>
        </div>
      </aside>

      <!-- 中：历史 -->
      <main class="history">
        <div class="history-head" v-if="!miniMode">
          <h2>{{ miniMode ? '快速粘贴' : '最近项目' }}</h2>
          <div class="history-head-actions">
            <button class="ghostbtn" :class="{ active: groupBy }" @click="groupBy = !groupBy" title="按日期分组">日</button>
            <span class="head-count">{{ flatIndexed.length }} 条</span>
          </div>
        </div>

        <div v-if="miniMode" class="mini-strip">
          <button
            v-for="tag in miniTagFilters"
            :key="tag.name"
            :class="['mini-tag', { active: selectedTag === tag.name }]"
            @click="selectedTag = selectedTag === tag.name ? '' : tag.name"
          >#{{ tag.name }}</button>
          <button v-if="selectedTag" class="mini-tag clear" @click="selectedTag = ''">全部</button>
          <span class="mini-count">{{ flatIndexed.length }} 条</span>
        </div>

    <div v-if="selectMode && !miniMode" class="multi-bar">
      <span class="multi-count">已选 {{ selectedIds.size }}</span>
      <button class="multi-btn" @click="selectAllVisible">全选</button>
      <button class="multi-btn" @click="selectNone">清选</button>
      <span class="multi-spacer"></span>
      <button class="multi-btn" @click="mergeSelected" :disabled="selectedIds.size === 0" title="把选中项合并成一条并粘贴">合并粘贴</button>
      <button class="multi-btn" @click="pasteSequence" :disabled="selectedIds.size === 0" title="按顺序逐条粘贴到当前应用">依次粘贴</button>
      <button class="multi-btn" @click="batchPin(true)" :disabled="selectedIds.size === 0">收藏</button>
      <button class="multi-btn" @click="batchPin(false)" :disabled="selectedIds.size === 0">取消收藏</button>
      <button class="multi-btn danger" @click="batchDelete" :disabled="selectedIds.size === 0">删除</button>
    </div>

        <div class="queue">
      <div v-if="flatIndexed.length === 0" class="empty">
        <div class="empty-text">{{ search || selectedTag ? '无匹配结果' : '尚无记录' }}</div>
        <div class="empty-hint">复制或拖拽内容即可录入</div>
      </div>

      <!-- 平铺 -->
      <template v-if="miniMode || !groupBy">
        <div
          v-for="item in (miniMode ? miniItems : filtered)"
          :key="item.id"
          :class="['item', { selected: isSelected(item), pinned: item.pinned, 'multi-on': selectedIds.has(item.id), 'dragging': draggedItemId === item.id, 'drop-target': dropTargetId === item.id }]"
          :draggable="filter === 'pinned' && item.pinned && !selectMode"
          @click="pickItem(item)"
          @dragstart="onDragStart(item, $event)"
          @dragover="onDragOver(item, $event)"
          @dragleave="onDragLeave"
          @drop="onDrop(item, $event)"
          @dragend="onDragEnd"
        >
          <span v-if="!miniMode && !selectMode && quickIndex(item)" class="quick-no" :title="`Alt+${quickIndex(item)} 快速粘贴`">{{ quickIndex(item) }}</span>
          <span class="serial" aria-hidden="true">{{ serialOf(item) }}</span><span class="glyph" aria-hidden="true">{{ themedGlyph(item) }}</span>
          <div class="item-main">
            <div class="item-title">
              <span>{{ typeLabel(item) }}</span>
              <span v-if="detailLabel(item)" class="detail-label">{{ detailLabel(item) }}</span>
            </div>
            <div v-if="item.content_type === 'text'" class="item-preview">
              <span
                v-for="(part, idx) in highlightedParts(item.text)"
                :key="idx"
                :class="{ hit: part.hit }"
              >{{ part.text }}</span>
            </div>
            <div v-else class="image-line">
              <img
                v-if="item.image_path"
                :src="imgSrc(item)"
                class="thumb" alt=""
                loading="lazy"
                decoding="async"
                @click.stop="openLightbox(item)"
                title="点击查看大图"
              />
              <span v-else class="thumb thumb-loading">…</span>
            </div>
            <div class="meta">
              <span>{{ relativeTime(item.created_at) }}</span>
              <span v-if="item.copy_count > 0" class="copy-mark">复制 {{ item.copy_count }} 次</span>
              <span v-if="item.pinned" class="pin-mark">已收藏</span>
            </div>
            <div class="item-tags">
              <template v-if="!miniMode">
                <span v-if="detectColor(item.text)" class="color-swatch" :style="{ background: detectColor(item.text) || undefined }" :title="detectColor(item.text) || ''"></span>
                <button
                  v-for="at in autoTags(item)"
                  :key="'auto-' + at"
                  :class="['auto-tag', { active: autoTagFilter === at }]"
                  @click.stop="toggleAutoTag(at)"
                >{{ at }}</button>
              </template>
              <button
                v-for="tag in (miniMode ? visibleTags(item, 2) : item.tags)"
                :key="tag"
                :class="['tag-pill', { active: selectedTag === tag }]"
                @click.stop="selectedTag = selectedTag === tag ? '' : tag"
              >#{{ tag }}</button>
              <button v-if="!miniMode" class="tag-add" @click.stop="openTagEditor(item)">{{ item.tags?.length ? '编辑标签' : '+ 标签' }}</button>
            </div>
          </div>
          <div class="row-actions">
            <span v-if="isSelected(item) && !selectMode" class="enterhint" aria-hidden="true"><svg class="ic"><use href="#i-return"/></svg>复制</span>
            <button
              v-if="selectMode"
              :class="['iconbtn', 'check', { on: selectedIds.has(item.id) }]"
              @click.stop="toggleSelect(item.id)"
              title="勾选"
            ><svg class="ic"><use :href="selectedIds.has(item.id) ? '#i-check' : '#i-circle'"/></svg></button>
            <button
              v-if="item.content_type === 'text' && !miniMode"
              class="iconbtn"
              @click.stop="copyPlain(item)"
              title="复制纯文本"
            >文</button>
            <button
              v-if="detectUrl(item.text) && !miniMode"
              class="iconbtn"
              @click.stop="openItemUrl(item)"
              title="打开链接"
            >↗</button>
            <button
              v-if="looksJson(item.text) && !miniMode"
              class="iconbtn"
              @click.stop="prettyJsonAction(item)"
              title="格式化 JSON 并复制"
            >{}</button>
            <button
              v-if="detectUrl(item.text) && !miniMode"
              class="iconbtn"
              @click.stop="copyDomain(item)"
              title="复制域名"
            >域</button>
            <button
              v-if="detectEmail(item.text) && !miniMode"
              class="iconbtn"
              @click.stop="mailtoItem(item)"
              title="发送邮件"
            >邮</button>
            <button
              v-if="detectPath(item.text) && !miniMode"
              class="iconbtn"
              @click.stop="revealItem(item)"
              title="在文件夹中显示"
            >夹</button>
            <button
              v-if="item.content_type === 'text' && item.text && !miniMode"
              class="iconbtn qr-btn"
              @click.stop="openQr(item)"
              title="生成二维码"
            ><svg class="ic"><use href="#i-qr"/></svg></button>
            <button
              v-if="item.content_type === 'image' && !miniMode"
              class="iconbtn ocr-btn"
              @click.stop="extractText(item)"
              title="识别图片文字 (OCR)"
            ><svg class="ic"><use href="#i-ocr"/></svg></button>
            <button
              :class="['iconbtn', { on: item.pinned }]"
              @click.stop="togglePinClip(item)"
              :title="item.pinned ? '取消收藏' : '收藏'"
            ><svg class="ic" :class="{ fill: item.pinned }"><use href="#i-star"/></svg></button>
            <button class="iconbtn danger" @click.stop="deleteItem(item)" title="删除"><svg class="ic"><use href="#i-trash"/></svg></button>
          </div>
        </div>
      </template>

      <!-- 分组 -->
      <template v-else>
        <div v-for="g in (selectedTag ? groupedByTag : grouped)" :key="g.name" class="group">
          <div class="group-head">
            <span class="group-name">{{ g.name }}</span>
            <span class="group-count">{{ g.items.length }}</span>
          </div>
          <div
            v-for="item in g.items"
            :key="item.id"
            :class="['item', { selected: isSelected(item), pinned: item.pinned, 'multi-on': selectedIds.has(item.id) }]"
            @click="pickItem(item)"
          >
            <span v-if="!selectMode && quickIndex(item)" class="quick-no" :title="`Alt+${quickIndex(item)} 快速粘贴`">{{ quickIndex(item) }}</span>
            <span class="serial" aria-hidden="true">{{ serialOf(item) }}</span><span class="glyph" aria-hidden="true">{{ themedGlyph(item) }}</span>
            <div class="item-main">
              <div class="item-title">
                <span>{{ typeLabel(item) }}</span>
                <span v-if="detailLabel(item)" class="detail-label">{{ detailLabel(item) }}</span>
              </div>
              <div v-if="item.content_type === 'text'" class="item-preview">
                <span
                  v-for="(part, idx) in highlightedParts(item.text)"
                  :key="idx"
                  :class="{ hit: part.hit }"
                >{{ part.text }}</span>
              </div>
              <div v-else class="image-line">
                <img
                  v-if="item.image_path"
                  :src="imgSrc(item)"
                  class="thumb" alt=""
                  loading="lazy"
                  decoding="async"
                  @click.stop="openLightbox(item)"
                  title="点击查看大图"
                />
                <span v-else class="thumb thumb-loading">…</span>
              </div>
              <div class="meta">
                <span>{{ relativeTime(item.created_at) }}</span>
                <span v-if="item.copy_count > 0" class="copy-mark">复制 {{ item.copy_count }} 次</span>
                <span v-if="item.pinned" class="pin-mark">已收藏</span>
              </div>
              <div class="item-tags">
                <span v-if="detectColor(item.text)" class="color-swatch" :style="{ background: detectColor(item.text) || undefined }" :title="detectColor(item.text) || ''"></span>
                <button
                  v-for="at in autoTags(item)"
                  :key="'auto-' + at"
                  :class="['auto-tag', { active: autoTagFilter === at }]"
                  @click.stop="toggleAutoTag(at)"
                >{{ at }}</button>
                <button
                  v-for="tag in item.tags"
                  :key="tag"
                  :class="['tag-pill', { active: selectedTag === tag }]"
                  @click.stop="selectedTag = selectedTag === tag ? '' : tag"
                >#{{ tag }}</button>
                <button class="tag-add" @click.stop="openTagEditor(item)">{{ item.tags?.length ? '编辑标签' : '+ 标签' }}</button>
              </div>
            </div>
            <div class="row-actions">
              <span v-if="isSelected(item) && !selectMode" class="enterhint" aria-hidden="true"><svg class="ic"><use href="#i-return"/></svg>复制</span>
              <button
                v-if="selectMode"
                :class="['iconbtn', 'check', { on: selectedIds.has(item.id) }]"
                @click.stop="toggleSelect(item.id)"
                title="勾选"
              ><svg class="ic"><use :href="selectedIds.has(item.id) ? '#i-check' : '#i-circle'"/></svg></button>
              <button
                v-if="item.content_type === 'text'"
                class="iconbtn"
                @click.stop="copyPlain(item)"
                title="复制纯文本"
              >文</button>
              <button
                v-if="detectUrl(item.text)"
                class="iconbtn"
                @click.stop="openItemUrl(item)"
                title="打开链接"
              >↗</button>
              <button
                v-if="looksJson(item.text)"
                class="iconbtn"
                @click.stop="prettyJsonAction(item)"
                title="格式化 JSON 并复制"
              >{}</button>
              <button
                v-if="detectUrl(item.text)"
                class="iconbtn"
                @click.stop="copyDomain(item)"
                title="复制域名"
              >域</button>
              <button
                v-if="detectEmail(item.text)"
                class="iconbtn"
                @click.stop="mailtoItem(item)"
                title="发送邮件"
              >邮</button>
              <button
                v-if="detectPath(item.text)"
                class="iconbtn"
                @click.stop="revealItem(item)"
                title="在文件夹中显示"
              >夹</button>
              <button
                v-if="item.content_type === 'text' && item.text"
                class="iconbtn qr-btn"
                @click.stop="openQr(item)"
                title="生成二维码"
              ><svg class="ic"><use href="#i-qr"/></svg></button>
              <button
                v-if="item.content_type === 'image'"
                class="iconbtn ocr-btn"
                @click.stop="extractText(item)"
                title="识别图片文字 (OCR)"
              ><svg class="ic"><use href="#i-ocr"/></svg></button>
              <button
                :class="['iconbtn', { on: item.pinned }]"
                @click.stop="togglePinClip(item)"
                :title="item.pinned ? '取消收藏' : '收藏'"
              ><svg class="ic" :class="{ fill: item.pinned }"><use href="#i-star"/></svg></button>
              <button class="iconbtn danger" @click.stop="deleteItem(item)" title="删除"><svg class="ic"><use href="#i-trash"/></svg></button>
            </div>
          </div>
        </div>
      </template>
        </div>
      </main>

    </div>

    <footer v-if="!miniMode" class="footer">
      <span class="foot-ok"><span class="foot-dot"></span> 本地数据库</span>
      <span class="foot-status">{{ selectMode ? `已选 ${selectedIds.size} 条` : '就绪' }}</span>
      <span v-if="dragDebug" class="drag-debug">{{ dragDebug }}</span>
      <span class="foot-hints"><kbd>↑↓</kbd> 选择 · <kbd>↵</kbd> 复制 · <kbd>⌥1-9</kbd> 速粘 · <kbd>⌘K</kbd>/<kbd>/</kbd> 搜索</span>
      <span class="count">{{ flatIndexed.length }} / {{ items.length }}</span>
    </footer>

    <!-- Lightbox -->
    <div v-if="lightboxItem" class="lightbox" @click="lightboxItem = null">
      <img
        v-if="lightboxItem.image_path"
        :src="imgFullSrc(lightboxItem)"
        class="lightbox-img"
        @click.stop
      />
      <div class="lightbox-bar" @click.stop>
        <span class="lightbox-info">{{ relativeTime(lightboxItem.created_at) }}</span>
        <div class="lightbox-actions">
          <button class="lb-btn primary" @click="pickItem(lightboxItem!); lightboxItem = null">粘贴</button>
          <button class="lb-btn" @click="lightboxItem = null">关闭</button>
        </div>
      </div>
      <div class="lightbox-hint">Esc 关闭 ‧ ↵ 粘贴</div>
    </div>

    <!-- OCR 识别确认 -->
    <div v-if="ocrItem" class="modal-bg" @click="closeOcrDialog">
      <div class="modal ocr-modal" @click.stop>
        <div class="modal-head">
          <span>识别图片文字</span>
          <button class="icon-btn close" @click="closeOcrDialog">×</button>
        </div>
        <div class="modal-body ocr-body">
          <div class="ocr-preview">
            <div class="ocr-toolbar">
              <button :class="['setting-btn', { active: ocrTool === 'select' }]" :disabled="ocrBusy" @click="setOcrTool('select')">框选</button>
              <button :class="['setting-btn', { active: ocrTool === 'pan' }]" :disabled="ocrBusy" @click="setOcrTool('pan')">拖动图片</button>
              <button class="setting-btn" :disabled="ocrBusy" @click="fitOcrImage()">适合窗口</button>
              <button class="setting-btn" :disabled="ocrBusy" @click="zoomOcr(-0.25)">缩小</button>
              <span class="ocr-zoom-label">{{ ocrZoomLabel }}</span>
              <button class="setting-btn" :disabled="ocrBusy" @click="zoomOcr(0.25)">放大</button>
            </div>
            <div
              v-if="ocrItem.image_path"
              ref="ocrViewport"
              class="ocr-image-viewport"
              @wheel="onOcrWheel"
            >
              <div
                :class="['ocr-image-stage', { 'pan-mode': ocrTool === 'pan', panning: !!ocrPanStart }]"
                :style="ocrStageStyle"
                @pointerdown="onOcrSelectStart"
                @pointermove="onOcrSelectMove"
                @pointerup="onOcrSelectEnd"
                @pointercancel="onOcrSelectEnd"
              >
                <img
                  ref="ocrImageEl"
                  :src="imgFullSrc(ocrItem)"
                  class="ocr-image"
                  draggable="false"
                  alt=""
                  @load="onOcrImageLoad"
                />
                <div v-if="ocrSelection" class="ocr-selection" :style="ocrSelectionStyle"></div>
              </div>
            </div>
            <div class="ocr-hint">先放大，再用「拖动图片」平移定位；切回「框选」后拖出识别区域。Ctrl+滚轮也可缩放。</div>
            <div class="ocr-actions">
              <button class="setting-btn primary" :disabled="ocrBusy" @click="recognizeOcr('all')">识别整图</button>
              <button class="setting-btn" :disabled="ocrBusy || !ocrSelection" @click="recognizeOcr('selection')">识别选区</button>
              <button class="setting-btn" :disabled="ocrBusy || !ocrSelection" @click="clearOcrSelection">清除选区</button>
            </div>
          </div>
          <div class="ocr-result">
            <div class="setting-label">识别结果</div>
            <textarea
              v-model="ocrText"
              class="setting-textarea ocr-textarea"
              placeholder="识别后会显示在这里，你可以先确认或编辑，再加入列表。"
              :disabled="ocrBusy"
            ></textarea>
            <div class="ocr-message">{{ ocrMessage }}</div>
            <div class="modal-actions">
              <button class="setting-btn" :disabled="ocrBusy" @click="closeOcrDialog">取消</button>
              <button class="setting-btn" :disabled="ocrBusy || !hasOcrText" @click="copyOcrText">复制文字</button>
              <button class="setting-btn primary" :disabled="ocrBusy || !hasOcrText" @click="addOcrText">加入列表</button>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 二维码 -->
    <div v-if="qrItem" class="modal-bg" @click="qrItem = null">
      <div class="modal qr-modal" @click.stop>
        <div class="modal-head">
          <span>二维码</span>
          <button class="icon-btn close" @click="qrItem = null">×</button>
        </div>
        <div class="modal-body qr-body">
          <img v-if="qrDataUrl" :src="qrDataUrl" class="qr-img" alt="二维码" />
          <div class="qr-text">{{ preview(qrItem.text, 120) }}</div>
        </div>
      </div>
    </div>

    <!-- 设置 -->
    <div v-if="settingsOpen" class="modal-bg" @click="settingsOpen = false">
      <div class="modal" @click.stop>
        <div class="modal-head">
          <span>设置</span>
          <button class="icon-btn close" @click="settingsOpen = false">×</button>
        </div>
        <div class="modal-body">
          <div class="setting-row">
            <div class="setting-label">全局唤起快捷键</div>
            <div class="setting-control">
              <input
                v-if="hotkeyEditing"
                class="hotkey-input"
                :value="hotkeyValue"
                @keydown="captureHotkey"
                placeholder="按下组合键..."
                readonly
              />
              <code v-else class="hotkey-display">{{ hotkeyValue }}</code>
              <button v-if="!hotkeyEditing" class="setting-btn" @click="hotkeyEditing = true">修改</button>
              <button v-else class="setting-btn primary" @click="saveHotkey">保存</button>
              <button v-if="hotkeyEditing" class="setting-btn" @click="hotkeyEditing = false">取消</button>
            </div>
            <div v-if="hotkeyError" class="setting-error">{{ hotkeyError }}</div>
            <div class="setting-help">点击「修改」后按下组合键。例如 Ctrl+Shift+V</div>
          </div>
          <div class="setting-row">
            <div class="setting-label">隐私模式</div>
            <div v-if="privacyStatus" class="privacy-box">
              <label class="setting-check">
                <input v-model="privacyStatus.settings.enabled" type="checkbox" />
                <span>启用标题 / 应用忽略规则</span>
              </label>
              <label class="setting-check">
                <input v-model="privacyStatus.settings.ignore_incognito" type="checkbox" />
                <span>忽略浏览器无痕 / 隐身窗口</span>
              </label>
              <div class="privacy-status">
                <span>{{ pauseLabel(privacyStatus.paused_remaining_ms) }}</span>
                <span v-if="privacyStatus.blocking_reason">当前命中：{{ privacyStatus.blocking_reason }}</span>
              </div>
              <div class="pause-row">
                <button class="setting-btn" @click="pauseClipboard(5)">暂停 5 分钟</button>
                <button class="setting-btn" @click="pauseClipboard(15)">15 分钟</button>
                <button class="setting-btn" @click="pauseClipboard(30)">30 分钟</button>
                <button class="setting-btn" @click="pauseClipboard(0)">恢复</button>
              </div>
              <div class="setting-subrow">
                <div class="setting-label small">忽略应用</div>
                <input v-model="ignoredAppsText" class="setting-input" placeholder="chrome.exe, 1Password, KeePass" />
              </div>
              <div class="setting-subrow">
                <div class="setting-label small">敏感标题关键词</div>
                <input v-model="sensitiveKeywordsText" class="setting-input" placeholder="password, 登录, 密码" />
              </div>
              <div class="setting-help">按窗口标题和进程名判断；暂停期间不会补录被跳过的内容。</div>
              <button class="setting-btn primary" @click="savePrivacy">保存隐私规则</button>
            </div>
          </div>

          <div class="setting-row">
            <div class="setting-label">提示</div>
            <div class="setting-help">‧ 拖拽文件/图片到窗口可入库</div>
            <div class="setting-help">‧ 文本条目右侧可快速复制纯文本、打开链接、格式化 JSON、复制域名</div>
          </div>
        </div>
      </div>
    </div>

    <!-- 标签编辑 -->
    <div v-if="tagEditorItem" class="modal-bg" @click="tagEditorItem = null">
      <div class="modal tag-modal" @click.stop>
        <div class="modal-head">
          <span>编辑标签</span>
          <button class="icon-btn close" @click="tagEditorItem = null">×</button>
        </div>
        <div class="modal-body">
          <div class="setting-row">
            <div class="setting-label">用逗号分隔标签</div>
            <input v-model="tagEditorText" class="setting-input" placeholder="账号, 代码片段, 提示词" @keydown.enter="saveTags" />
            <div class="setting-help">常用标签可以直接在左侧筛选。</div>
          </div>
          <div class="modal-actions">
            <button class="setting-btn" @click="tagEditorItem = null">取消</button>
            <span class="modal-spacer"></span>
            <button class="setting-btn primary" @click="saveTags">保存</button>
          </div>
        </div>
      </div>
    </div>

    <!-- 统计 -->
    <div v-if="statsOpen && stats" class="modal-bg" @click="statsOpen = false">
      <div class="modal" @click.stop>
        <div class="modal-head">
          <span>统计</span>
          <button class="icon-btn close" @click="statsOpen = false">×</button>
        </div>
        <div class="modal-body">
          <div class="stat-grid">
            <div class="stat-cell"><div class="stat-num">{{ stats.total }}</div><div class="stat-name">总数</div></div>
            <div class="stat-cell"><div class="stat-num">{{ stats.text_count }}</div><div class="stat-name">文本</div></div>
            <div class="stat-cell"><div class="stat-num">{{ stats.image_count }}</div><div class="stat-name">图片</div></div>
            <div class="stat-cell"><div class="stat-num">{{ stats.pinned_count }}</div><div class="stat-name">收藏</div></div>
            <div class="stat-cell"><div class="stat-num">{{ stats.total_copies }}</div><div class="stat-name">粘贴次数</div></div>
          </div>
          <div class="stat-sec-title">高频使用 Top 5</div>
          <div v-if="stats.top_items.length === 0" class="stat-empty">还没有使用记录</div>
          <div v-else class="stat-top">
            <div v-for="it in stats.top_items" :key="it.id" class="stat-top-row">
              <span class="stat-rank-glyph">{{ it.content_type === 'text' ? '文' : '图' }}</span>
              <span class="stat-rank-text">{{ it.content_type === 'text' ? preview(it.text) : '[图片]' }}</span>
              <span class="stat-rank-count">{{ it.copy_count }} 次</span>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 新建片段 modal -->
    <div v-if="snippetEditorOpen" class="modal-bg" @click="snippetEditorOpen = false">
      <div class="modal snippet-modal" @click.stop>
        <div class="modal-head">
          <span>新建片段</span>
          <button class="icon-btn close" @click="snippetEditorOpen = false">×</button>
        </div>
        <div class="modal-body">
          <label class="setting-row">
            <span class="setting-label">内容</span>
            <textarea v-model="snippetText" class="setting-textarea" placeholder="输入片段内容，支持占位符：{{date}} {{time}} {{clipboard}}" rows="6" @keydown.ctrl.enter="saveSnippet"></textarea>
          </label>
          <label class="setting-row">
            <span class="setting-label">标签</span>
            <input v-model="snippetTags" class="setting-input" placeholder="邮件签名, SQL, 常用回复" @keydown.enter="saveSnippet" />
          </label>
          <div class="modal-actions">
            <button class="setting-btn" @click="snippetEditorOpen = false">取消</button>
            <button class="setting-btn primary" @click="saveSnippet">保存</button>
          </div>
        </div>
      </div>
    </div>

    <!-- 帮助 -->
    <div v-if="helpOpen" class="modal-bg" @click="helpOpen = false">
      <div class="modal help" @click.stop>
        <div class="modal-head">
          <span>帮 助</span>
          <button class="icon-btn close" @click="helpOpen = false">×</button>
        </div>
        <div class="modal-body help-body">
          <section class="help-sec">
            <h3 class="help-h">概览</h3>
            <p class="help-p">本地剪贴板历史与快速粘贴面板。后台记录系统剪贴板里的文本和图片，文本重复时只刷新时间，图片会转成 PNG 存入本地库；未收藏记录超过 200 条时会清理旧项，收藏项保留。唤出窗口后可搜索、筛选、打标签、收藏，再把选中内容写回剪贴板并粘贴到当前应用。</p>
          </section>

          <section class="help-sec">
            <h3 class="help-h">键盘</h3>
            <table class="help-table">
              <tr><td><kbd>Ctrl + Shift + V</kbd></td><td>默认全局唤起快捷键，可在「设置」修改</td></tr>
              <tr><td><kbd>Ctrl</kbd> / <kbd>⌘</kbd> + <kbd>K</kbd></td><td>聚焦搜索框</td></tr>
              <tr><td><kbd>/</kbd></td><td>聚焦搜索框</td></tr>
              <tr><td><kbd>Alt</kbd> + <kbd>1</kbd>–<kbd>9</kbd></td><td>快速粘贴对应序号的条目</td></tr>
              <tr><td><kbd>Ctrl</kbd> / <kbd>⌘</kbd> + <kbd>M</kbd></td><td>切换小窗 / 完整模式</td></tr>
              <tr><td><kbd>↑</kbd> <kbd>↓</kbd></td><td>上下选择</td></tr>
              <tr><td><kbd>↵</kbd></td><td>复制并自动粘贴选中条目</td></tr>
              <tr><td><kbd>Ctrl</kbd> + <kbd>↵</kbd></td><td>仅写剪贴板，不自动粘贴</td></tr>
              <tr><td><kbd>␣</kbd> 空格</td><td>预览选中条目，图片显示大图</td></tr>
              <tr><td><kbd>⇥</kbd> Tab</td><td>循环切换全部 / 文本 / 图片 / 收藏视图</td></tr>
              <tr><td><kbd>Esc</kbd></td><td>关闭弹窗、退出多选或隐藏主窗口</td></tr>
            </table>
          </section>

          <section class="help-sec">
            <h3 class="help-h">条目操作</h3>
            <ul class="help-ul">
              <li><b>点击条目</b>：写入剪贴板并自动粘贴</li>
              <li><b>文</b>：复制纯文本；图片条目会复制图片文件路径</li>
              <li><b>↗</b>：打开检测到的链接</li>
              <li><b>{}</b>：格式化 JSON 并复制结果</li>
              <li><b>域</b>：复制链接域名</li>
              <li><b>邮</b>：用默认邮件客户端打开检测到的邮箱</li>
              <li><b>夹</b>：在文件夹中定位检测到的本地路径</li>
              <li><b>二维码</b>：为文本条目生成二维码</li>
              <li><b>识别</b>：图片条目可打开 OCR 确认弹窗</li>
              <li><b>收藏</b>：收藏 / 取消收藏</li>
              <li><b>+ 标签</b>：编辑标签，标签可在左侧或小窗顶部筛选</li>
              <li><b>×</b>：删除该条</li>
            </ul>
          </section>

          <section class="help-sec">
            <h3 class="help-h">图片文字识别</h3>
            <ul class="help-ul">
              <li><b>识别整图</b>：对整张图片执行 OCR，结果先显示在弹窗中</li>
              <li><b>框选</b>：拖动图片画出区域，再点「识别选区」</li>
              <li><b>拖动图片</b>：放大后按住图片平移，定位后切回「框选」</li>
              <li><b>缩放</b>：使用放大 / 缩小按钮，或按住 Ctrl 滚动鼠标滚轮</li>
              <li><b>确认</b>：可先编辑识别结果，再复制文字或加入列表</li>
            </ul>
          </section>

          <section class="help-sec">
            <h3 class="help-h">片段占位符</h3>
            <p class="help-p">点击顶栏「片段」按钮可手动创建可复用文本片段(如邮件签名、常用回复、SQL 模板)。片段内容支持占位符,粘贴时自动替换：</p>
            <table class="help-table">
              <tr><td><code v-pre>{{date}}</code></td><td>替换为当前日期 (YYYY-MM-DD)</td></tr>
              <tr><td><code v-pre>{{time}}</code></td><td>替换为当前时间 (HH:MM:SS)</td></tr>
              <tr><td><code v-pre>{{clipboard}}</code></td><td>替换为系统剪贴板当前内容</td></tr>
            </table>
            <p class="help-p">示例: <code v-pre>会议记录 {{date}} - {{clipboard}}</code> 粘贴时变为 <code>会议记录 2026-06-22 - [剪贴板内容]</code></p>
          </section>

          <section class="help-sec">
            <h3 class="help-h">顶栏</h3>
            <ul class="help-ul">
              <li><b>搜索框</b>：搜索文本和标签；高级筛选可开启正则和区分大小写</li>
              <li><b>色点</b>：切换主题，浅色 7 个、深色 5 个</li>
              <li><b>?</b>：打开帮助</li>
              <li><b>齿轮</b>：设置全局快捷键和隐私规则，可暂停监听</li>
              <li><b>导入 / 导出</b>：读写 JSON 历史文件</li>
              <li><b>小窗 / 完整</b>：切换精简粘贴窗口和完整管理界面</li>
              <li><b>⋯</b>：日期分组、详细统计、清空未收藏</li>
              <li><b>窗口按钮</b>：最小化、最大化 / 还原、隐藏窗口</li>
            </ul>
          </section>

          <section class="help-sec">
            <h3 class="help-h">左侧栏</h3>
            <ul class="help-ul">
              <li><b>视图</b>：全部、文本、图片、收藏</li>
              <li><b>筛选</b>：按时间、复制次数、正则、大小写过滤</li>
              <li><b>标签</b>：按常用标签快速缩小结果</li>
              <li><b>指标</b>：今日新增、收藏数、图片数、累计复制次数</li>
              <li><b>批量选择</b>：全选、清选、批量收藏、取消收藏、删除</li>
              <li><b>窗口置顶</b>：让窗口保持在其他应用上方</li>
            </ul>
          </section>

          <section class="help-sec">
            <h3 class="help-h">拖拽入库</h3>
            <ul class="help-ul">
              <li>拖图片文件 → 转 PNG 入图片库</li>
              <li>拖文本 / 代码 / json 等 → 读内容入文本库</li>
              <li>拖其它类型 → 路径作为文本存</li>
            </ul>
          </section>

          <section class="help-sec">
            <h3 class="help-h">系统托盘</h3>
            <ul class="help-ul">
              <li><b>左键</b>：切换窗口显示</li>
              <li><b>右键</b>：显示窗口、最近 5 条快捷粘贴、清空未收藏历史、退出</li>
            </ul>
          </section>

          <section class="help-sec">
            <h3 class="help-h">导入 / 导出</h3>
            <p class="help-p">JSON 格式，包含所有条目（图片以路径形式）。导入时文本去重；图片只在原路径仍存在时才导入。</p>
          </section>

          <section class="help-sec">
            <h3 class="help-h">使用统计</h3>
            <p class="help-p">总数 / 文本 / 图片 / 收藏 / 累计粘贴次数 + 高频使用 Top 5。</p>
          </section>
        </div>
      </div>
    </div>

    <!-- 拖拽蒙层 -->
    <div v-if="dragOver" class="drag-mask">
      <div class="drag-mask-inner">
        <div class="drag-glyph">↓</div>
        <div class="drag-text">松开以加入剪贴板</div>
      </div>
    </div>

    <div class="resize-hint" aria-hidden="true"></div>
  </div>
</template>

<style>
/* ===== 主题：纯净中性灰（Linear / Vercel 风） ===== */
:root {
  --radius: 6px;
  --radius-sm: 4px;
  --bw: 1px;
  --transition: .22s cubic-bezier(.4,0,.2,1);
  --field: var(--panel-soft);
  --font-ui:
    "IBM Plex Sans", -apple-system, BlinkMacSystemFont, "Segoe UI Variable", "Segoe UI",
    "Microsoft YaHei UI", "Microsoft YaHei", "PingFang SC", "Hiragino Sans GB",
    system-ui, sans-serif;
  --font-mono: "Courier Prime", ui-monospace, "Cascadia Mono", Consolas, monospace;
  --font-display: "Zilla Slab", "Noto Serif SC", "Microsoft YaHei", serif;
}
/* ===== 基底 = 档案 archive(prototypes/d):牛皮纸 + 墨蓝/朱砂双色油墨 ===== */
:root {
  --bg: #e9dfc8;
  --panel: #f8f2e3;
  --panel-soft: #efe7d2;
  --panel-2: #f4edda;
  --text: #2c4460;
  --text-secondary: #5c7186;
  --text-tertiary: #8fa0b0;
  --border: rgba(44,68,96,.30);
  --border-strong: rgba(44,68,96,.55);
  --accent: #c14a2e;
  --accent-hover: #9c3520;
  --accent-ink: #f8f2e3;
  --accent-soft: rgba(193,74,46,.13);
  --shadow-sm: 0 2px 0 rgba(44,68,96,.10);
  --shadow:    0 4px 0 rgba(44,68,96,.14);
  --shadow-lg: 0 4px 0 rgba(44,68,96,.18), 10px 12px 0 rgba(44,68,96,.10);
  --shadow-sel: 2px 3px 0 rgba(44,68,96,.22);
  --radius: 5px; --radius-sm: 3px;
  --ruling: rgba(44,68,96,.05);   /* 卡纸横线 */
  --grain: rgba(44,68,96,.03);    /* 桌面斜纹 */
  --punch: rgba(44,68,96,.35);    /* 打孔描边 */
}
/* ===== 档案·夜:夜灯下的档案室 —— 深墨蓝桌面 + 米纸文字 + 提亮朱砂 ===== */
[data-theme="archive-dark"] {
  --bg: #141b26;
  --panel: #1e2836;
  --panel-soft: #253243;
  --panel-2: #1a2330;
  --text: #e7dfca;
  --text-secondary: #b4ad97;
  --text-tertiary: #74809a;
  --border: rgba(231,223,202,.14);
  --border-strong: rgba(231,223,202,.32);
  --accent: #e06a4a;
  --accent-hover: #eb8163;
  --accent-ink: #201009;
  --accent-soft: rgba(224,106,74,.16);
  --shadow-sm: 0 2px 0 rgba(0,0,0,.32);
  --shadow:    0 4px 0 rgba(0,0,0,.36);
  --shadow-lg: 0 4px 0 rgba(0,0,0,.42), 10px 12px 0 rgba(0,0,0,.24);
  --shadow-sel: 2px 3px 0 rgba(0,0,0,.45);
  --ruling: rgba(231,223,202,.055);
  --grain: rgba(231,223,202,.028);
  --punch: rgba(231,223,202,.35);
}

* { box-sizing: border-box; }
html, body, #app {
  margin: 0; padding: 0; height: 100vh; overflow: hidden;
  background: var(--bg);
  color: var(--text);
  font-family: var(--font-ui);
  font-size: 13px; line-height: 1.5;
  user-select: none;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  text-rendering: optimizeLegibility;
}

.app {
  position: relative;
  display: grid;
  grid-template-rows: 60px 1fr 34px;
  height: 100vh;
  background: var(--bg);
  border-radius: 0; border: 1px solid var(--border);
  overflow: hidden;
  transition: background-color var(--transition), color var(--transition);
}
.app.mini-mode {
  grid-template-rows: 46px 1fr;
  min-width: 0;
}
.app.drag-over { box-shadow: inset 0 0 0 2px var(--accent); }
.ic { width: 16px; height: 16px; display: block; flex-shrink: 0;
  fill: none; stroke: currentColor; stroke-width: 1.7;
  stroke-linecap: round; stroke-linejoin: round; }
.ic.fill { fill: currentColor; }

/* ===== 顶栏 ===== */
.topbar {
  display: flex; align-items: center; gap: 14px;
  padding: 0 12px 0 14px;
  background: var(--panel-2);
  border-bottom: var(--bw) solid var(--border);
  -webkit-app-region: drag;
  z-index: 100;
}
.brand { display: flex; align-items: center; gap: 10px; flex-shrink: 0; }
.mark {
  width: 30px; height: 30px; border-radius: 8px;
  display: grid; place-items: center;
  background: var(--accent); color: var(--accent-ink);
}
.mark .ic { width: 18px; height: 18px; stroke-width: 2; }
.brand-text { display: flex; flex-direction: column; line-height: 1; }
h1 {
  margin: 0; font-family: var(--font-display);
  font-size: 14px; font-weight: 700; letter-spacing: -.01em;
  color: var(--text); line-height: 1.15;
}
.sub {
  margin-top: 2px;
  font-family: var(--font-mono);
  font-size: 9.5px; letter-spacing: .04em; text-transform: uppercase;
  color: var(--text-tertiary);
}

.command {
  flex: 1; min-width: 0;
  display: flex; align-items: center; gap: 8px;
  height: 36px; padding: 0 10px 0 12px;
  background: var(--field);
  border: var(--bw) solid var(--border-strong);
  border-radius: var(--radius);
  -webkit-app-region: no-drag;
  transition: border-color var(--transition), background var(--transition), box-shadow var(--transition);
}
.command:focus-within {
  border-color: var(--accent); background: var(--panel);
  box-shadow: 0 0 0 3px var(--accent-soft);
}
.command .lead { color: var(--text-tertiary); flex-shrink: 0; display: flex; }
.command:focus-within .lead { color: var(--accent); }
.command .lead .ic { width: 15px; height: 15px; }
.cmd-input {
  flex: 1; min-width: 0; background: transparent; border: none; outline: none;
  color: var(--text); font-family: inherit; font-size: 13px;
}
.cmd-input::placeholder { color: var(--text-tertiary); }
.cmd-clear { cursor: pointer; color: var(--text-tertiary); font-size: 16px; line-height: 1; padding: 0 2px; }
.cmd-clear:hover { color: var(--text); }
.kbd {
  flex-shrink: 0; pointer-events: none; user-select: none;
  font-family: var(--font-mono);
  font-size: 11px; line-height: 1; color: var(--text-tertiary);
  border: 1px solid var(--border-strong); border-radius: 5px;
  padding: 3px 7px; background: var(--panel);
}
.command:focus-within .kbd { color: var(--accent); border-color: var(--accent); }

.top-actions {
  display: flex; align-items: center; gap: 4px; flex-shrink: 0;
  -webkit-app-region: no-drag;
}
.topbtn {
  display: inline-flex; align-items: center; gap: 5px;
  height: 30px; padding: 0 10px;
  background: var(--field);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  cursor: pointer; font-family: inherit; font-size: 12px; line-height: 1;
  transition: all var(--transition); white-space: nowrap;
}
.topbtn:hover { color: var(--accent); border-color: var(--accent); background: var(--accent-soft); }
.topbtn.icon { width: 30px; padding: 0; justify-content: center; }
.topbtn.primary { background: var(--accent); color: var(--accent-ink); border-color: var(--accent); }
.topbtn.primary:hover { background: var(--accent-hover); border-color: var(--accent-hover); color: var(--accent-ink); }
.topbtn.active { color: var(--accent); border-color: var(--accent); background: var(--accent-soft); }
.topbtn .ic { width: 15px; height: 15px; }
.mini-toggle-btn { min-width: 44px; justify-content: center; }

.winctrls { display: flex; align-items: center; gap: 2px; margin-left: 4px; -webkit-app-region: no-drag; }
.winbtn {
  width: 30px; height: 30px; display: grid; place-items: center;
  background: transparent; border: none; border-radius: var(--radius-sm);
  color: var(--text-tertiary); cursor: pointer; transition: all var(--transition);
}
.winbtn:hover { color: var(--text); background: var(--panel-soft); }
.winbtn.close:hover { color: #fff; background: #c0392b; }
.winbtn .ic { width: 15px; height: 15px; }

/* modal close button (复用) */
.icon-btn {
  width: 28px; height: 28px;
  display: inline-flex; align-items: center; justify-content: center;
  background: transparent; border: 1px solid transparent; border-radius: var(--radius-sm);
  color: var(--text-tertiary); font-family: inherit; font-size: 13px;
  cursor: pointer; transition: all var(--transition); line-height: 1; padding: 0;
}
.icon-btn:hover { color: var(--text); background: var(--accent-soft); }
.icon-btn.active { color: var(--accent); background: var(--accent-soft); }
.icon-btn.close:hover { color: #fff; background: #a04030; }
.theme-dot {
  width: 12px; height: 12px; border-radius: 50%;
  display: inline-block; border: 1px solid var(--border-strong);
}

.theme-switcher, .menu-wrap { position: relative; }
.theme-popover, .menu-popover {
  position: absolute; top: calc(100% + 6px); right: 0; z-index: 50;
  background: var(--panel); border: 1px solid var(--border-strong);
  border-radius: var(--radius); padding: 6px;
  min-width: 160px; box-shadow: var(--shadow-lg);
  max-height: 66vh; overflow-y: auto;
  animation: popIn .14s ease-out;
}
@keyframes popIn {
  from { opacity: 0; transform: translateY(-3px); }
  to   { opacity: 1; transform: translateY(0); }
}
.theme-section-label {
  font-size: 10px; color: var(--text-tertiary); letter-spacing: 0;
  padding: 6px 8px 4px; text-transform: uppercase;
}
.theme-row, .menu-row {
  display: flex; align-items: center; gap: 8px;
  width: 100%; padding: 7px 8px;
  background: transparent; border: none; border-radius: var(--radius-sm);
  color: var(--text); cursor: pointer;
  font-family: inherit; font-size: 12px;
  transition: background var(--transition);
  text-align: left;
}
.theme-row:hover, .menu-row:hover { background: var(--accent-soft); }
.theme-row.active { background: var(--accent-soft); color: var(--accent); }
.theme-row-dot {
  width: 14px; height: 14px; border-radius: 50%; flex-shrink: 0;
  border: 1px solid var(--border-strong);
}
.theme-row-name { flex-shrink: 0; font-weight: 500; letter-spacing: 0; }
.theme-row-id {
  margin-left: auto;
  font-family: var(--font-mono);
  font-size: 10px; color: var(--text-tertiary);
}
.theme-row.active .theme-row-id { color: var(--accent); }
.menu-glyph { width: 16px; text-align: center; color: var(--text-tertiary); }
.menu-row.danger:hover { color: #c04030; }
.menu-row.danger:hover .menu-glyph { color: #c04030; }
.menu-sep { height: 1px; background: var(--border); margin: 4px 0; }

.multi-bar {
  display: flex; align-items: center; gap: 6px;
  padding: 6px 18px;
  background: var(--accent-soft);
  border-bottom: 1px solid var(--border);
  font-size: 12px;
  flex-shrink: 0;
}
.multi-count {
  font-family: var(--font-mono);
  color: var(--accent); letter-spacing: 0; margin-right: 6px;
}
.multi-spacer { flex: 1; }
.multi-btn {
  padding: 4px 10px;
  background: var(--panel);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  cursor: pointer;
  font-family: inherit; font-size: 11px; letter-spacing: 0;
  transition: all var(--transition);
}
.multi-btn:hover:not(:disabled) { border-color: var(--accent); color: var(--accent); }
.multi-btn:disabled { opacity: .4; cursor: not-allowed; }
.multi-btn.danger:hover:not(:disabled) {
  background: #a04030; color: #fff; border-color: #a04030;
}

/* ===== 工作区 ===== */
.workspace {
  display: grid;
  grid-template-columns: 224px minmax(0, 1fr);
  min-height: 0;
  overflow: hidden;
}
.mini-mode .workspace {
  grid-template-columns: 1fr;
}

/* 左栏：视图 + 指标 */
.side {
  display: flex; flex-direction: column;
  background: var(--panel-2);
  border-right: var(--bw) solid var(--border);
  padding: 14px 12px;
  overflow-y: auto;
}
.section { margin-bottom: 18px; }
.eyebrow {
  display: flex; align-items: center; justify-content: space-between;
  margin: 0 0 8px; padding: 0 2px;
  font-family: var(--font-mono);
  font-size: 10px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase;
  color: var(--text-tertiary);
}
.eyebrow-link {
  background: transparent; border: none; cursor: pointer;
  color: var(--accent); font-family: inherit; font-size: 10px; letter-spacing: .02em;
  text-transform: none; padding: 0;
}
.eyebrow-link:hover { text-decoration: underline; }

.nav { display: flex; flex-direction: column; gap: 2px; }
.chip {
  display: flex; align-items: center; justify-content: space-between;
  padding: 7px 10px;
  background: transparent; border: 1px solid transparent; border-radius: var(--radius-sm);
  color: var(--text-secondary); cursor: pointer;
  font-family: inherit; font-size: 13px; text-align: left;
  transition: all var(--transition);
}
.chip:hover { background: var(--panel-soft); color: var(--text); }
.chip.active {
  background: var(--accent-soft); color: var(--accent);
  box-shadow: inset 3px 0 0 var(--accent);
}
.chip b {
  font-family: var(--font-mono);
  font-size: 11px; font-weight: 600; color: var(--text-tertiary);
}
.chip.active b { color: var(--accent); }
.filter-panel {
  display: flex; flex-direction: column; gap: 8px;
}
.filter-row {
  display: grid; grid-template-columns: 64px 1fr; align-items: center; gap: 8px;
  font-size: 11px; color: var(--text-secondary);
}
.field-mini {
  min-width: 0; height: 28px;
  padding: 0 8px;
  background: var(--panel-soft);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text);
  font-family: inherit; font-size: 11px;
  outline: none;
}
.field-mini:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
.filter-toggles { display: grid; grid-template-columns: 1fr 1fr; gap: 4px; }
.mini-toggle {
  height: 28px;
  background: var(--panel-soft);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  font-family: inherit; font-size: 10px;
  cursor: pointer;
}
.mini-toggle:hover, .mini-toggle.active {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-soft);
}
.filter-error {
  margin-bottom: 6px;
  color: #c0392b;
  font-size: 11px;
  line-height: 1.4;
}
.clear-filters { justify-self: flex-start; text-align: left; }
.tag-cloud {
  display: flex; flex-wrap: wrap; gap: 5px;
}
.tag-filter {
  max-width: 100%;
  min-height: 24px;
  padding: 3px 7px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--panel-soft);
  color: var(--text-secondary);
  font-family: inherit; font-size: 10px;
  cursor: pointer;
}
.tag-filter:hover, .tag-filter.active {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-soft);
}
.tag-filter b {
  margin-left: 4px;
  color: var(--text-tertiary);
  font-family: var(--font-mono);
}

.stats { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.stat {
  display: flex; flex-direction: column; gap: 3px;
  padding: 9px 10px;
  background: var(--panel-soft);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
}
.stat span { font-size: 10px; letter-spacing: .02em; color: var(--text-tertiary); }
.stat b {
  font-family: var(--font-mono);
  font-size: 18px; font-weight: 600; line-height: 1; color: var(--accent);
}

.side-bottom { margin-top: auto; display: flex; flex-direction: column; gap: 6px; padding-top: 12px; }
.topbtn.block { width: 100%; justify-content: center; height: 32px; }
.toggle {
  display: flex; align-items: center; justify-content: space-between;
  padding: 7px 10px;
  background: var(--panel-soft); border: 1px solid var(--border);
  border-radius: var(--radius-sm); cursor: pointer;
  color: var(--text-secondary); font-family: inherit; font-size: 12px;
  transition: all var(--transition);
}
.toggle:hover { border-color: var(--border-strong); color: var(--text); }
.switch {
  width: 30px; height: 17px; border-radius: 9px; position: relative; flex-shrink: 0;
  background: var(--border-strong); transition: background var(--transition);
}
.switch::after {
  content: ''; position: absolute; top: 2px; left: 2px;
  width: 13px; height: 13px; border-radius: 50%;
  background: var(--panel); transition: transform var(--transition);
}
.switch.on { background: var(--accent); }
.switch.on::after { transform: translateX(13px); }

/* 中栏：历史 */
.history {
  display: flex; flex-direction: column;
  min-width: 0; background: var(--bg);
  overflow: hidden;
}
.history-head {
  display: flex; align-items: center; justify-content: space-between;
  padding: 12px 16px 8px;
  flex-shrink: 0;
}
.history-head h2 {
  margin: 0; font-family: inherit;
  font-size: 13px; font-weight: 600; color: var(--text);
}
.history-head-actions { display: flex; align-items: center; gap: 10px; }
.ghostbtn {
  width: 26px; height: 24px;
  background: transparent; border: 1px solid var(--border);
  border-radius: var(--radius-sm); cursor: pointer;
  color: var(--text-tertiary); font-family: inherit; font-size: 12px;
  transition: all var(--transition);
}
.ghostbtn:hover { color: var(--text); border-color: var(--border-strong); }
.ghostbtn.active { color: var(--accent); border-color: var(--accent); background: var(--accent-soft); }
.head-count {
  font-family: var(--font-mono);
  font-size: 11px; color: var(--text-tertiary);
}
.queue { flex: 1; overflow-y: auto; padding: 2px 10px 10px; }
.mini-strip {
  display: flex; align-items: center; gap: 6px;
  padding: 0 8px 6px;
  overflow-x: auto;
  flex-shrink: 0;
}
.mini-tag {
  height: 22px;
  max-width: 82px;
  padding: 0 7px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--panel-2);
  color: var(--text-secondary);
  font-family: inherit;
  font-size: 10px;
  line-height: 1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  cursor: pointer;
  flex-shrink: 0;
}
.mini-tag:hover,
.mini-tag.active {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-soft);
}
.mini-tag.clear { border-style: dashed; }
.mini-count {
  margin-left: auto;
  color: var(--text-tertiary);
  font-family: var(--font-mono);
  font-size: 10px;
  white-space: nowrap;
  flex-shrink: 0;
}

.empty {
  padding: 60px 20px 40px; text-align: center;
  color: var(--text-tertiary);
}
.empty-text {
  font-size: 14px; letter-spacing: 0;
  color: var(--text-secondary); margin-bottom: 6px;
  font-family: inherit;
  font-weight: 500;
}
.empty-hint { font-size: 12px; letter-spacing: 0; color: var(--text-tertiary); }

.group { padding-bottom: 4px; }
.group-head {
  display: flex; align-items: center; gap: 8px;
  padding: 8px 18px 4px;
  font-size: 11px; letter-spacing: 0;
  color: var(--text-tertiary);
  font-family: inherit;
}
.group-head::before {
  content: ''; height: 1px; width: 14px;
  background: var(--border-strong);
}
.group-name { color: var(--text-secondary); font-family: var(--font-display); }
.group-count {
  font-family: var(--font-mono);
  font-size: 10px; color: var(--text-tertiary);
}

/* 卡片 */
.item {
  display: grid;
  grid-template-columns: 30px 1fr auto;
  gap: 11px;
  padding: 9px 10px;
  margin-bottom: 4px;
  align-items: start;
  cursor: pointer; position: relative;
  background: var(--panel-2);
  border: var(--bw) solid var(--border);
  border-radius: var(--radius);
  transition: border-color var(--transition), background var(--transition), box-shadow var(--transition);
}
.mini-mode .item {
  grid-template-columns: 24px minmax(0, 1fr) auto;
  gap: 8px;
  margin-bottom: 4px;
  padding: 7px 7px;
}
.item:hover { border-color: var(--border-strong); box-shadow: var(--shadow-sm); }
.item.selected {
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent), var(--shadow-sm);
}
.item.selected::before {
  content: ''; position: absolute; left: 0; top: 9px; bottom: 9px;
  width: 3px; border-radius: 0 2px 2px 0; background: var(--accent);
}
.item.multi-on { border-color: var(--accent); background: var(--accent-soft); }
.item.dragging { opacity: 0.5; cursor: grabbing; }
.item.drop-target { border-color: var(--accent); border-style: dashed; background: var(--accent-soft); }
.item[draggable="true"] { cursor: grab; }

/* Alt+1–9 快速粘贴角标 */
.quick-no {
  position: absolute; top: 6px; right: 8px; z-index: 1;
  min-width: 14px; height: 15px; padding: 0 3px;
  display: flex; align-items: center; justify-content: center;
  font-family: var(--font-mono);
  font-size: 9.5px; line-height: 1; font-weight: 600;
  color: var(--text-tertiary);
  background: var(--panel-soft);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  font-variant-numeric: tabular-nums;
  pointer-events: none;
  transition: opacity var(--transition);
}
.item:hover .quick-no { opacity: 0; }

.glyph {
  width: 30px; height: 30px;
  border: 1px solid var(--border-strong); border-radius: var(--radius-sm);
  display: flex; align-items: center; justify-content: center;
  font-family: var(--font-mono);
  font-size: 13px; font-weight: 500; color: var(--accent);
  background: var(--panel); flex-shrink: 0;
  transition: all var(--transition);
}
.item:hover .glyph, .item.selected .glyph {
  background: var(--accent); color: var(--accent-ink); border-color: var(--accent);
}

.item-main { min-width: 0; }
.item-title {
  display: flex; align-items: center; gap: 8px;
  font-family: var(--font-mono);
  font-size: 10px; letter-spacing: .04em; text-transform: uppercase;
  color: var(--text-tertiary); margin-bottom: 3px;
}
.detail-label {
  min-width: 0;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  color: var(--accent);
  font-family: inherit;
  font-size: 10px;
  letter-spacing: 0;
  text-transform: none;
}
.item-preview {
  font-size: 13px; line-height: 1.5; color: var(--text);
  overflow: hidden;
  display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical;
  word-break: break-all; user-select: text;
}
.mini-mode .item-preview {
  -webkit-line-clamp: 1;
  font-size: 12px;
  line-height: 1.35;
}
.item-preview .hit {
  background: var(--accent-soft);
  color: var(--accent);
  border-radius: 3px;
  box-shadow: 0 0 0 1px var(--accent-soft);
}
.item-tags {
  display: flex; flex-wrap: wrap; gap: 4px;
  margin-top: 6px;
}
.mini-mode .item-tags {
  flex-wrap: nowrap;
  overflow: hidden;
  margin-top: 4px;
}
.tag-pill, .tag-add {
  max-width: 140px;
  padding: 2px 6px;
  border: 1px solid var(--border);
  border-radius: 3px;
  background: var(--panel-soft);
  color: var(--text-tertiary);
  font-family: inherit;
  font-size: 10px;
  line-height: 1.35;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  cursor: pointer;
}
.tag-pill:hover, .tag-pill.active, .tag-add:hover {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-soft);
}
.tag-add {
  opacity: 0;
  border-style: dashed;
}
.item:hover .tag-add, .item.selected .tag-add, .item.multi-on .tag-add {
  opacity: 1;
}
.image-line { display: flex; align-items: center; padding: 2px 0; }
.thumb {
  max-width: 220px; max-height: 140px; object-fit: contain;
  border-radius: var(--radius-sm); border: 1px solid var(--border-strong);
  background: var(--panel); display: block; cursor: zoom-in;
  transition: max-width var(--transition), max-height var(--transition), box-shadow var(--transition), transform var(--transition);
}
.item.selected .thumb, .item:hover .thumb {
  max-width: 280px; max-height: 180px; box-shadow: var(--shadow);
}
.thumb:hover { transform: scale(1.02); }
.thumb-loading {
  display: flex; align-items: center; justify-content: center;
  width: 220px; height: 140px;
  color: var(--text-tertiary);
  border-radius: var(--radius-sm); border: 1px solid var(--border);
  background: var(--panel);
}

.meta {
  display: flex; gap: 6px; margin-top: 4px;
  font-size: 11px; color: var(--text-tertiary); letter-spacing: 0;
}
.mini-mode .meta {
  margin-top: 2px;
  font-size: 10px;
}
.pin-mark { color: var(--accent); }
.copy-mark { color: var(--text-secondary); }

/* 卡片右侧操作 */
.row-actions {
  display: flex; align-items: center; gap: 2px;
  align-self: center; flex-shrink: 0;
}
.enterhint {
  display: inline-flex; align-items: center; gap: 4px;
  margin-right: 4px; padding: 2px 7px;
  border-radius: 5px; background: var(--accent-soft); color: var(--accent);
  font-family: var(--font-mono);
  font-size: 10px; white-space: nowrap;
}
.mini-mode .enterhint { display: none; }
.enterhint .ic { width: 13px; height: 13px; }
.iconbtn {
  width: 28px; height: 28px; display: grid; place-items: center;
  background: transparent; border: 1px solid transparent; border-radius: var(--radius-sm);
  color: var(--text-tertiary); cursor: pointer; font-size: 13px;
  font-family: inherit; line-height: 1;
  opacity: 0; transition: all var(--transition);
}
.item:hover .iconbtn, .item.selected .iconbtn, .item.multi-on .iconbtn { opacity: 1; }
.mini-mode .iconbtn { opacity: 1; width: 24px; height: 24px; }
.iconbtn:hover { color: var(--accent); background: var(--accent-soft); }
.iconbtn.on { color: var(--accent); opacity: 1; }
.iconbtn.check { opacity: 1; }
.iconbtn.check.on { border-color: var(--accent); color: var(--accent); }
.iconbtn.danger:hover { color: #fff; background: #c0392b; }
.iconbtn .ic { width: 15px; height: 15px; }

.mini-mode .topbar {
  gap: 6px;
  padding: 0 6px;
}
.mini-mode .brand { gap: 0; }
.mini-mode .brand { display: none; }
.mini-mode .brand-text { display: none; }
.mini-mode .mark {
  width: 28px;
  height: 28px;
  border-radius: 7px;
}
.mini-mode .command {
  height: 32px;
  padding-left: 9px;
}
.mini-mode .top-actions > :not(.mini-toggle-btn):not(.winctrls) {
  display: none;
}
.mini-mode .top-actions {
  gap: 3px;
}
.mini-mode .winctrls {
  margin-left: 0;
}
.mini-mode .winbtn {
  width: 26px;
  height: 26px;
}
.mini-mode .history {
  background: var(--bg);
}
.mini-mode .queue {
  padding: 0 6px 6px;
}
.mini-mode .empty {
  padding: 44px 16px 24px;
}
.mini-mode .image-line {
  max-height: 56px;
  overflow: hidden;
}
.mini-mode .thumb,
.mini-mode .thumb-loading {
  max-width: 82px;
  max-height: 48px;
}
.mini-mode .glyph {
  width: 24px;
  height: 24px;
  font-size: 11px;
}
.mini-mode .item-title {
  margin-bottom: 2px;
  font-size: 9px;
}
.mini-mode .tag-pill {
  max-width: 92px;
  padding: 1px 5px;
  font-size: 9.5px;
}
.mini-mode .copy-mark,
.mini-mode .pin-mark {
  display: none;
}

.linkbtn {
  background: transparent; border: none; padding: 0; cursor: pointer;
  color: var(--accent); font-family: inherit; font-size: 11px;
}
.linkbtn:hover { text-decoration: underline; }

/* ===== 帮助 ===== */
.modal.help { width: min(560px, 92vw); }
.help-body { padding: 8px 18px 18px; }
.help-sec {
  padding: 12px 0;
  border-bottom: 1px solid var(--border);
}
.help-sec:last-child { border-bottom: none; }
.help-h {
  margin: 0 0 8px;
  font-size: 13px;
  font-weight: 500;
  letter-spacing: 0;
  color: var(--text);
  font-family: inherit;
}
.help-p {
  margin: 4px 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--text-secondary);
  letter-spacing: 0;
  user-select: text;
}
.help-ul {
  margin: 4px 0 0;
  padding-left: 18px;
  font-size: 12px;
  line-height: 1.85;
  color: var(--text-secondary);
  user-select: text;
}
.help-ul li { letter-spacing: 0; }
.help-ul b {
  color: var(--accent);
  font-weight: 500;
  margin-right: 2px;
}
.help-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  user-select: text;
}
.help-table td {
  padding: 5px 0;
  vertical-align: top;
  color: var(--text-secondary);
}
.help-table td:first-child {
  width: 38%;
  white-space: nowrap;
}
.help-table kbd {
  display: inline-block;
  padding: 1px 6px;
  margin-right: 3px;
  background: var(--panel-soft);
  border: 1px solid var(--border-strong);
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--accent);
  letter-spacing: 0;
}
.modal-actions {
  display: flex; align-items: center; gap: 6px;
  padding-top: 10px;
  border-top: 1px solid var(--border);
}
.modal-spacer { flex: 1; }

/* 拖拽调试反馈 */
.drag-debug {
  font-family: var(--font-mono);
  color: var(--accent);
  font-size: 10px;
  letter-spacing: 0;
  margin-left: 6px;
}

/* ===== 底栏 ===== */
.footer {
  display: flex; align-items: center; gap: 14px;
  padding: 0 16px;
  background: var(--panel-2);
  border-top: var(--bw) solid var(--border);
  font-size: 11px; color: var(--text-tertiary);
}
.foot-ok { display: inline-flex; align-items: center; gap: 6px; color: var(--text-secondary); }
.foot-dot {
  width: 7px; height: 7px; border-radius: 50%;
  background: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft);
}
.foot-status { color: var(--text-tertiary); }
.foot-hints { margin-left: auto; display: inline-flex; align-items: center; color: var(--text-tertiary); white-space: nowrap; }
.footer kbd {
  display: inline-block; padding: 1px 5px; margin: 0 4px 0 6px;
  background: var(--panel);
  border: 1px solid var(--border-strong); border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 10px; color: var(--text-secondary);
}
.footer kbd:first-child { margin-left: 0; }
.count {
  flex-shrink: 0;
  font-family: var(--font-mono);
  color: var(--text-secondary);
}

::-webkit-scrollbar { width: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--border-strong); border-radius: 3px; }
::-webkit-scrollbar-thumb:hover { background: var(--accent); }

.lightbox {
  position: fixed; inset: 0; z-index: 1000;
  background: rgba(0, 0, 0, 0.92);
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  cursor: zoom-out;
  animation: fadeOpacity .18s ease-out;
}
@keyframes fadeOpacity { from { opacity: 0; } to { opacity: 1; } }
.lightbox-img {
  max-width: 92vw; max-height: 78vh; object-fit: contain;
  border-radius: 6px; box-shadow: 0 24px 60px rgba(0,0,0,.6);
  cursor: default;
  animation: zoomIn .2s cubic-bezier(.4,0,.2,1);
}
@keyframes zoomIn {
  from { opacity: 0; transform: scale(.96); }
  to   { opacity: 1; transform: scale(1); }
}
.lightbox-bar {
  display: flex; align-items: center; gap: 16px;
  margin-top: 18px; padding: 10px 16px;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 8px;
  cursor: default;
}
.lightbox-info { color: rgba(255, 255, 255, 0.75); font-size: 12px; letter-spacing: 0; }
.lightbox-actions { display: flex; gap: 8px; }
.lb-btn {
  background: rgba(255, 255, 255, 0.1);
  border: 1px solid rgba(255, 255, 255, 0.18);
  color: #fff; padding: 6px 16px; border-radius: 6px;
  cursor: pointer; font-family: inherit; font-size: 12px; letter-spacing: 0;
  transition: all var(--transition);
}
.lb-btn:hover { background: rgba(255, 255, 255, 0.18); border-color: rgba(255, 255, 255, 0.3); }
.lb-btn.primary { background: var(--accent); border-color: var(--accent); color: var(--accent-ink); }
.lb-btn.primary:hover { background: var(--accent-hover); border-color: var(--accent-hover); }
.lightbox-hint { margin-top: 12px; color: rgba(255, 255, 255, 0.45); font-size: 11px; letter-spacing: 0; }

.modal-bg {
  position: fixed; inset: 0; z-index: 900;
  background: rgba(0,0,0,.55);
  display: flex; align-items: center; justify-content: center;
  animation: fadeOpacity .14s ease-out;
}
.modal {
  width: min(420px, 90vw);
  max-height: 80vh; overflow: auto;
  background: var(--panel);
  border: 1px solid var(--border-strong);
  border-radius: 8px;
  box-shadow: var(--shadow-lg);
  animation: zoomIn .16s cubic-bezier(.4,0,.2,1);
}
.modal-head {
  display: flex; align-items: center; justify-content: space-between;
  padding: 14px 16px 10px;
  border-bottom: 1px solid var(--border);
  font-family: inherit;
  font-size: 15px; letter-spacing: 0; color: var(--text);
}
.modal-body { padding: 14px 16px 16px; }
.setting-row { margin-bottom: 16px; }
.setting-subrow { margin-top: 10px; }
.setting-label {
  font-size: 12px; letter-spacing: 0;
  color: var(--text-secondary); margin-bottom: 8px;
}
.setting-label.small { font-size: 11px; margin-bottom: 5px; color: var(--text-tertiary); }
.setting-control { display: flex; align-items: center; gap: 8px; }
.hotkey-display, .hotkey-input {
  font-family: var(--font-mono);
  font-size: 12px;
  padding: 6px 10px;
  background: var(--panel-soft);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  color: var(--accent);
  min-width: 160px;
}
.hotkey-input { outline: none; }
.hotkey-input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
.setting-input {
  width: 100%;
  height: 32px;
  padding: 0 10px;
  background: var(--panel-soft);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  color: var(--text);
  font-family: inherit;
  font-size: 12px;
  outline: none;
}
.setting-input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
.setting-textarea {
  width: 100%;
  padding: 8px 10px;
  background: var(--panel-soft);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  color: var(--text);
  font-family: inherit;
  font-size: 12px;
  line-height: 1.5;
  resize: vertical;
  outline: none;
}
.setting-textarea:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
.ocr-modal {
  width: min(860px, 94vw);
}
.ocr-body {
  display: grid;
  grid-template-columns: minmax(260px, 1fr) minmax(260px, 340px);
  gap: 16px;
}
.ocr-preview {
  min-width: 0;
}
.ocr-toolbar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 8px;
}
.ocr-zoom-label {
  min-width: 44px;
  text-align: center;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-secondary);
}
.ocr-image-viewport {
  width: 100%;
  height: min(56vh, 520px);
  min-height: 280px;
  overflow: auto;
  background: var(--panel-soft);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
}
.ocr-image-stage {
  position: relative;
  display: block;
  min-width: 1px;
  min-height: 1px;
  margin: 0 auto;
  overflow: hidden;
  cursor: crosshair;
  user-select: none;
  touch-action: none;
}
.ocr-image-stage.pan-mode { cursor: grab; }
.ocr-image-stage.panning { cursor: grabbing; }
.ocr-image {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: fill;
  pointer-events: none;
}
.ocr-selection {
  position: absolute;
  border: 2px solid var(--accent);
  background: color-mix(in srgb, var(--accent) 18%, transparent);
  box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0.32);
  pointer-events: none;
}
.ocr-hint,
.ocr-message {
  margin-top: 8px;
  font-size: 11px;
  line-height: 1.5;
  color: var(--text-tertiary);
}
.ocr-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 10px;
}
.ocr-result {
  display: flex;
  flex-direction: column;
  min-width: 0;
}
.ocr-textarea {
  min-height: 260px;
  resize: vertical;
}
.setting-btn:disabled {
  opacity: .45;
  cursor: not-allowed;
}
.privacy-box {
  display: flex; flex-direction: column; gap: 8px;
  padding: 10px;
  background: var(--panel-soft);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
}
.setting-check {
  display: flex; align-items: center; gap: 8px;
  color: var(--text-secondary);
  font-size: 12px;
}
.setting-check input { accent-color: var(--accent); }
.privacy-status {
  display: flex; flex-wrap: wrap; gap: 8px;
  color: var(--text-tertiary);
  font-size: 11px;
}
.privacy-status span:first-child {
  color: var(--accent);
  font-family: var(--font-mono);
}
.pause-row {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 5px;
}
.setting-btn {
  padding: 5px 12px;
  background: var(--panel-soft);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  cursor: pointer;
  font-family: inherit; font-size: 11px; letter-spacing: 0;
  transition: all var(--transition);
}
.setting-btn:hover { color: var(--accent); border-color: var(--accent); }
.setting-btn.active { color: var(--accent); border-color: var(--accent); background: var(--accent-soft); }
.setting-btn.primary { background: var(--accent); color: var(--accent-ink); border-color: var(--accent); }
.setting-btn.primary:hover { background: var(--accent-hover); border-color: var(--accent-hover); }
.setting-error { margin-top: 6px; font-size: 11px; color: #c04030; }
.setting-help { margin-top: 6px; font-size: 11px; color: var(--text-tertiary); letter-spacing: 0; }

.stat-grid {
  display: grid; grid-template-columns: repeat(5, 1fr);
  gap: 6px; margin-bottom: 18px;
}
.stat-cell {
  text-align: center; padding: 10px 4px;
  background: var(--panel-soft);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
}
.stat-num {
  font-family: var(--font-mono);
  font-size: 18px; color: var(--accent); line-height: 1;
}
.stat-name { margin-top: 4px; font-size: 10px; color: var(--text-tertiary); letter-spacing: 0; }
.stat-sec-title {
  font-size: 12px; letter-spacing: 0;
  color: var(--text-secondary); margin-bottom: 8px;
  font-family: inherit;
}
.stat-empty { font-size: 12px; color: var(--text-tertiary); text-align: center; padding: 16px; }
.stat-top-row {
  display: flex; align-items: center; gap: 8px;
  padding: 6px 4px; border-bottom: 1px solid var(--border);
  font-size: 12px;
}
.stat-top-row:last-child { border-bottom: none; }
.stat-rank-glyph {
  width: 22px; height: 22px; flex-shrink: 0;
  display: inline-flex; align-items: center; justify-content: center;
  border: 1px solid var(--border-strong); border-radius: 3px;
  color: var(--accent);
  font-family: inherit;
}
.stat-rank-text {
  flex: 1; min-width: 0;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  color: var(--text);
}
.stat-rank-count {
  font-family: var(--font-mono);
  color: var(--accent); font-size: 11px; letter-spacing: 0;
}

.drag-mask {
  position: fixed; inset: 0; z-index: 800;
  background: rgba(0,0,0,.45);
  display: flex; align-items: center; justify-content: center;
  pointer-events: none;
  animation: fadeOpacity .12s ease-out;
}
.drag-mask-inner {
  text-align: center;
  padding: 32px 56px;
  background: var(--panel);
  border: 2px dashed var(--accent);
  border-radius: 12px;
  box-shadow: var(--shadow-lg);
}
.drag-glyph {
  font-size: 38px; color: var(--accent); line-height: 1;
  margin-bottom: 8px;
  font-family: inherit;
}
.drag-text {
  font-size: 14px; letter-spacing: 0;
  color: var(--text-secondary);
  font-family: inherit;
}

.resize-hint {
  position: absolute; bottom: 2px; right: 2px;
  width: 14px; height: 14px; pointer-events: none; z-index: 50;
  background:
    linear-gradient(
      135deg,
      transparent 0%, transparent 60%,
      var(--border-strong) 60%, var(--border-strong) 65%,
      transparent 65%, transparent 75%,
      var(--border-strong) 75%, var(--border-strong) 80%,
      transparent 80%, transparent 90%,
      var(--border-strong) 90%, var(--border-strong) 95%,
      transparent 95%
    );
  opacity: .5;
}

.item { animation: fadeUp .22s cubic-bezier(.4,0,.2,1); }
@keyframes fadeUp {
  from { opacity: 0; transform: translateY(-2px); }
  to   { opacity: 1; transform: translateY(0); }
}

/* ===== 响应式 ===== */
@media (max-width: 1040px) {
  .workspace { grid-template-columns: 196px minmax(0, 1fr); }
}
@media (max-width: 820px) {
  .workspace {
    grid-template-columns: 1fr;
    grid-auto-rows: min-content;
    overflow-y: auto;
  }
  .side { border-right: none; border-bottom: 1px solid var(--border); }
  .side-bottom { margin-top: 12px; }
  .history { min-height: 320px; }
  .sub { display: none; }
  .ocr-body { grid-template-columns: 1fr; }
}

/* ===== Track 3 自动标签 / 色块 / 二维码 ===== */
.auto-tag {
  font-size: 10.5px; line-height: 1;
  padding: 3px 6px; border-radius: 999px;
  color: var(--text-secondary);
  background: transparent;
  border: 1px dashed var(--border-strong);
  cursor: pointer;
  transition: color var(--transition), background var(--transition), border-color var(--transition);
}
.auto-tag:hover { color: var(--accent); border-color: var(--accent); }
.auto-tag.active { color: var(--accent-ink); background: var(--accent); border-color: var(--accent); border-style: solid; }
.color-swatch {
  width: 13px; height: 13px; border-radius: 3px;
  border: 1px solid var(--border-strong);
  display: inline-block; flex-shrink: 0; align-self: center;
}
.qr-modal { width: min(320px, 90vw); }
.qr-body { display: flex; flex-direction: column; align-items: center; gap: 12px; }
.qr-img {
  width: 220px; height: 220px;
  image-rendering: pixelated;
  background: #fff; padding: 8px; border-radius: var(--radius-sm);
  border: 1px solid var(--border);
}
.qr-text {
  font-size: 11px; color: var(--text-tertiary);
  word-break: break-all; text-align: center;
  font-family: var(--font-mono);
}


/* ===== 档案设计语言:结构元素(置于末尾以覆盖基础规则;颜色走变量,档案·夜自动生效) ===== */
/* 窗体:牛皮纸/夜档斜纹肌理 */
.app { background-image: repeating-linear-gradient(45deg, transparent 0 10px, var(--grain) 10px 11px); }

/* 索引卡:卡纸横线 + 常驻投影;hover 抬起;选中 = 偏移硬投影 */
.item {
  box-shadow: var(--shadow-sm);
  background-image: repeating-linear-gradient(0deg, transparent 0 21px, var(--ruling) 21px 22px);
  transition: border-color var(--transition), background var(--transition),
    box-shadow var(--transition), transform .12s ease;
}
.item:hover { transform: translateY(-1px); box-shadow: var(--shadow); }
.item.selected { box-shadow: var(--shadow-sel); }

/* 左缘打孔(取代选中彩条) */
.app .item::before {
  content: ""; position: absolute; left: 4px; top: 50%; transform: translateY(-50%);
  width: 6px; height: 6px; border-radius: 50%;
  background: var(--bg); border: 1px solid var(--punch); z-index: 1;
}

/* 蜡封收藏章 */
.item.pinned::after {
  content: "★"; position: absolute; right: -5px; top: -8px; z-index: 2;
  width: 20px; height: 20px; border-radius: 50%;
  background: var(--accent); color: var(--accent-ink);
  display: flex; align-items: center; justify-content: center; font-size: 10px;
  box-shadow: 0 2px 0 var(--accent-hover); transform: rotate(9deg);
}

/* 打字机流水号列(迷你模式收起) */
.serial { display: none; }
.app:not(.mini-mode) .item { grid-template-columns: 50px 30px 1fr auto; }
.app:not(.mini-mode) .serial {
  display: flex; align-items: center; height: 30px;
  font-family: var(--font-mono); font-size: 10px; font-weight: 700; letter-spacing: .05em;
  color: var(--text-tertiary); font-variant-numeric: tabular-nums;
}
.serial::before { content: "NO."; }
.item.selected .serial { color: var(--accent); }

/* 盖章式类型徽章(藏/图/链/邮/色/码/档/文) */
.glyph {
  background: transparent; border-width: 2px; border-color: var(--text-secondary);
  color: var(--text); font-family: var(--font-display); font-weight: 700; font-size: 13px;
  transform: rotate(-3deg);
}
.item.pinned .glyph { border-color: var(--accent); color: var(--accent); border-radius: 50%; }

/* 分组头 = 抽屉标签 */
.group-head::before { display: none; }
.group-name {
  font-family: var(--font-display); font-weight: 700; font-size: 12px;
  color: var(--panel); background: var(--text);
  padding: 3px 14px 3px 9px; border-radius: 2px;
  clip-path: polygon(0 0, 100% 0, calc(100% - 8px) 100%, 0 100%);
}
.group-head::after { content: ""; flex: 1; border-top: 2px dotted var(--text-tertiary); }
.group-count { font-family: var(--font-mono); }

/* 品牌 = 圆形印章「贴」+ 打字机期号 */
.mark {
  background: transparent; border: 2px solid var(--accent); color: var(--accent);
  border-radius: 50%; transform: rotate(-6deg);
}
.mark .ic { display: none; }
.mark::before { content: "贴"; font-family: var(--font-display); font-weight: 700; font-size: 14px; }
.sub { letter-spacing: .14em; }

/* 小节标题衬线化 */
.eyebrow { font-family: var(--font-display); font-weight: 700; }
</style>
