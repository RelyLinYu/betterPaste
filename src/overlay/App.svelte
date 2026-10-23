<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import {
    listEntries,
    searchEntries,
    pasteEntry,
    pasteSlot,
    copyEntry,
    pinEntry,
    deleteEntry,
    deleteEntries,
    pinnedSlots,
    takeStartupHint,
    openSettings,
    getConfig,
    setConfig,
    hideOverlay,
    formatBytes,
    formatTime,
    EVENT_CONFIG_CHANGED,
    type AppConfig,
    type ClipEntry,
    type ThemeMode,
  } from "../lib/api";
  import { applyTheme } from "../lib/theme";

  let query = "";
  let entries: ClipEntry[] = [];
  let selected = 0;
  let loading = true;
  let cfg: AppConfig | null = null;
  let toast = "";
  let toastKind: "info" | "warn" = "info";
  let searchInput: HTMLInputElement;
  let listEl: HTMLDivElement;
  /** 勾选（多选删除）的条目 id */
  let checked = new Set<string>();
  /** 正在为哪一条挑置顶槽（打开槽位菜单） */
  let pinMenuFor: string | null = null;
  /** 槽位 1~9 的占用情况，用来画槽位菜单 */
  let slots: (ClipEntry | null)[] = [];
  let lastClicked = -1;

  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let unlisteners: UnlistenFn[] = [];

  const selectedEntry = (): ClipEntry | undefined => entries[selected];

  const historyLimit = () => cfg?.historyLimit ?? 500;

  async function refresh() {
    try {
      entries = query.trim()
        ? await searchEntries(query, historyLimit())
        : await listEntries(historyLimit());
      if (selected >= entries.length) selected = Math.max(0, entries.length - 1);
      loading = false;
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  function scheduleSearch() {
    clearTimeout(searchTimer);
    // 输入即过滤；本地库很小，40ms 防抖已经足够跟手
    searchTimer = setTimeout(() => {
      selected = 0;
      refresh();
    }, 40);
  }

  function showToast(msg: string, kind: "info" | "warn" = "info") {
    toast = msg;
    toastKind = kind;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ""), 2600);
  }

  function move(delta: number) {
    if (!entries.length) return;
    selected = Math.min(entries.length - 1, Math.max(0, selected + delta));
    scrollIntoView();
  }

  function scrollIntoView() {
    const el = listEl?.querySelector<HTMLElement>(".row.selected");
    el?.scrollIntoView({ block: "nearest" });
  }

  function afterPaste(result: string) {
    if (result === "manualFallback") {
      showToast("内容已复制 — 目标窗口权限较高，请手动按 Ctrl+V", "warn");
      setTimeout(() => hideOverlay(), 1800);
    } else if (cfg?.closeAfterPaste !== false) {
      hideOverlay();
    }
  }

  async function pasteAt(index: number) {
    const entry = entries[index];
    if (!entry) return;
    try {
      afterPaste(await pasteEntry(entry.id));
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  async function pasteSlotAt(slot: number) {
    try {
      afterPaste(await pasteSlot(slot));
    } catch (e) {
      showToast(`槽位 ${slot} 为空或粘贴失败：${e}`, "warn");
    }
  }

  async function copyOnly(entry: ClipEntry) {
    try {
      await copyEntry(entry.id);
      showToast("已复制到剪贴板（未粘贴）");
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  /* ---------------------------------------------------------------- 主题开关 */

  /** 当前实际是不是深色（theme = system 时按系统偏好算）。 */
  function isDarkNow(): boolean {
    const mode = cfg?.theme ?? "system";
    if (mode === "dark") return true;
    if (mode === "light") return false;
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  }

  /** 浅色 / 深色一键切换：改配置（持久化）并立刻套用，不用去设置面板。 */
  async function toggleTheme() {
    const next: ThemeMode = isDarkNow() ? "light" : "dark";
    try {
      // 不用 `if (!cfg) return`：早先浮层的 cfg 万一没取到，按钮就会变成
      // "点了没反应"。这里补取一次，取不到也把原因说出来。
      const base = cfg ?? (await getConfig());
      cfg = await setConfig({ ...base, theme: next });
      await applyTheme();
      showToast(next === "dark" ? "已切到深色" : "已切到浅色");
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  function firstFreeSlot(): number | null {
    const used = new Set<number>();
    for (const e of entries) if (e.pinned != null) used.add(e.pinned);
    for (let i = 1; i <= 9; i++) if (!used.has(i)) return i;
    return null;
  }

  async function togglePin(entry: ClipEntry) {
    try {
      const next = entry.pinned == null ? firstFreeSlot() : null;
      if (entry.pinned == null && next == null) {
        showToast("置顶槽 1~9 已满", "warn");
        return;
      }
      await pinEntry(entry.id, next);
      showToast(next == null ? "已取消置顶" : `已置顶到槽 ${next}`);
      await refresh();
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  async function removeEntry(entry: ClipEntry) {
    try {
      await deleteEntry(entry.id);
      await refresh();
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  /* ---------------------------------------------------------- 多选删除 */

  function toggleChecked(id: string) {
    const next = new Set(checked);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    checked = next;
  }

  function setChecked(ids: string[], on: boolean) {
    const next = new Set(checked);
    for (const id of ids) {
      if (on) next.add(id);
      else next.delete(id);
    }
    checked = next;
  }

  function allChecked() {
    return entries.length > 0 && entries.every((e) => checked.has(e.id));
  }

  function toggleAll() {
    checked = allChecked() ? new Set<string>() : new Set(entries.map((e) => e.id));
  }

  /** 点行：普通点击 = 选中；Ctrl 点击 = 勾选；Shift 点击 = 范围勾选。 */
  function onRowClick(e: MouseEvent, i: number) {
    selected = i;
    pinMenuFor = null;
    const entry = entries[i];
    if (!entry) return;
    if (e.ctrlKey || e.metaKey) {
      toggleChecked(entry.id);
      lastClicked = i;
      return;
    }
    if (e.shiftKey && lastClicked >= 0) {
      const [from, to] = lastClicked < i ? [lastClicked, i] : [i, lastClicked];
      setChecked(entries.slice(from, to + 1).map((x) => x.id), true);
      return;
    }
    lastClicked = i;
  }

  async function removeChecked() {
    const ids = [...checked];
    if (!ids.length) return;
    try {
      const n = await deleteEntries(ids);
      checked = new Set<string>();
      showToast(`已删除 ${n} 条`);
      await refresh();
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  /* ---------------------------------------------------------- 置顶槽 */

  async function openPinMenu(entry: ClipEntry) {
    try {
      slots = await pinnedSlots();
    } catch {
      slots = [];
    }
    pinMenuFor = pinMenuFor === entry.id ? null : entry.id;
  }

  async function assignSlot(entry: ClipEntry, slot: number | null) {
    try {
      await pinEntry(entry.id, slot);
      showToast(slot == null ? "已取消置顶" : `已置顶到槽 ${slot}`);
      pinMenuFor = null;
      await refresh();
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  /** 页面里（含输入框）此刻是否有非空的文本选区。 */
  function hasTextSelection(): boolean {
    const el = document.activeElement;
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
      if (el.selectionStart !== el.selectionEnd) return true;
    }
    const sel = window.getSelection();
    return !!sel && !sel.isCollapsed && sel.toString().trim() !== "";
  }

  async function onKeydown(e: KeyboardEvent) {
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        hideOverlay();
        return;
      case "ArrowDown":
        e.preventDefault();
        move(1);
        return;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        return;
      case "PageDown":
        e.preventDefault();
        move(8);
        return;
      case "PageUp":
        e.preventDefault();
        move(-8);
        return;
      case "Home":
        if (e.ctrlKey) {
          e.preventDefault();
          selected = 0;
          scrollIntoView();
        }
        return;
      case "End":
        if (e.ctrlKey) {
          e.preventDefault();
          selected = Math.max(0, entries.length - 1);
          scrollIntoView();
        }
        return;
      case "Enter": {
        e.preventDefault();
        const entry = selectedEntry();
        if (!entry) return;
        if (e.shiftKey) await copyOnly(entry);
        else await pasteAt(selected);
        return;
      }
      case "Delete":
        e.preventDefault();
        // 有勾选就批量删，否则删当前高亮的一条
        if (checked.size) await removeChecked();
        else {
          const entry = selectedEntry();
          if (entry) await removeEntry(entry);
        }
        return;
      case ",":
        if (e.ctrlKey || e.metaKey) {
          e.preventDefault();
          await openSettings();
          await hideOverlay();
        }
        return;
      case " ":
        // 空格 = 勾选/取消勾选当前高亮行（列表通用习惯）。
        // 但搜索框有内容时要把空格让给它，否则没法输入空格。
        if (query.trim() === "" && !e.ctrlKey && !e.metaKey && !e.altKey) {
          e.preventDefault();
          const entry = selectedEntry();
          if (entry) toggleChecked(entry.id);
        }
        return;
      case "F2": {
        // WebView2 会把 Ctrl+P 当成浏览器的"打印"处理掉（页面收不到 keydown），
        // 所以置顶用 F2 这个一定送得到的键位。
        e.preventDefault();
        const entry = selectedEntry();
        if (entry) await togglePin(entry);
        return;
      }
    }

    // Ctrl+Shift+1..9 → 直贴对应置顶槽（浮层内也支持）
    if (e.ctrlKey && e.shiftKey && /^Digit[1-9]$/.test(e.code)) {
      e.preventDefault();
      await pasteSlotAt(Number(e.code.slice(5)));
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.code === "KeyP") {
      e.preventDefault();
      const entry = selectedEntry();
      if (entry) await togglePin(entry);
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.code === "KeyC") {
      // 有选区时把 Ctrl+C 让给浏览器：用户可能只想复制列表/预览里选中的一小段文字。
      // 旧写法无条件抢走 Ctrl+C 去"复制整条"，于是选中的片段根本进不了剪贴板，
      // 也就不会被采集线程收进历史（用户视角：我复制了，历史里却没有）。
      // 让原生复制走，这段新内容就会被当作一次普通复制正常入库。
      if (hasTextSelection()) return;
      e.preventDefault();
      const entry = selectedEntry();
      if (entry) await copyOnly(entry);
    }
  }

  onMount(async () => {
    cfg = await getConfig().catch(() => null);
    await applyTheme();
    await refresh();
    searchInput?.focus();

    // 启动提示：可能"页面先 mount、后端后设置提示"，所以取不到就重试几次；
    // 拿到之后 6 秒自动收起（它就是打招呼，不该一直占着屏幕）。
    let hintDone = false;
    const grabHint = async () => {
      if (hintDone) return;
      const h = await takeStartupHint().catch(() => null);
      if (!h) return;
      hintDone = true;
      showToast(h, "info");
      setTimeout(() => hideOverlay(), 6000);
    };
    await grabHint();
    [400, 900, 1600, 2600].forEach((d) => setTimeout(grabHint, d));

    unlisteners.push(
      await listen("betterpaste://changed", () => refresh()),
      await listen("betterpaste://paused", () => {}),
      // 设置面板改了主题 / 配置：立刻跟上，不用重开浮层
      await listen(EVENT_CONFIG_CHANGED, async () => {
        cfg = await getConfig().catch(() => cfg);
        await applyTheme();
      }),
      await listen("tauri://focus", () => {
        selected = 0;
        pinMenuFor = null;
        searchInput?.focus();
        refresh().then(() => scrollIntoView());
      }),
    );

    // 在浮层外面点一下 = 关闭（window 失焦事件）
    unlisteners.push(
      await getCurrentWindow()
        .onFocusChanged(({ payload: focused }) => {
          if (!focused && cfg?.hideOnBlur !== false) hideOverlay();
        })
        .catch(() => () => {}),
    );

    window.addEventListener("keydown", onKeydown);
  });

  onDestroy(() => {
    unlisteners.forEach((u) => u());
    window.removeEventListener("keydown", onKeydown);
    clearTimeout(searchTimer);
    clearTimeout(toastTimer);
  });
</script>

<div class="panel" role="dialog" aria-label="剪贴板历史">
  <header class="search-row">
    <button
      class="icon-btn"
      title="打开设置（Ctrl+,）"
      aria-label="打开设置"
      onclick={async () => {
        await openSettings();
        await hideOverlay();
      }}
    >
      <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
        <path
          d="M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
        />
        <path
          d="M19.4 13a7.5 7.5 0 0 0 0-2l2-1.5-2-3.4-2.3 1a7.6 7.6 0 0 0-1.7-1l-.3-2.5h-4l-.3 2.5a7.6 7.6 0 0 0-1.7 1l-2.3-1-2 3.4L4.6 11a7.5 7.5 0 0 0 0 2l-2 1.5 2 3.4 2.3-1a7.6 7.6 0 0 0 1.7 1l.3 2.5h4l.3-2.5a7.6 7.6 0 0 0 1.7-1l2.3 1 2-3.4-2-1.5Z"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linejoin="round"
        />
      </svg>
    </button>
    <svg class="logo" viewBox="0 0 24 24" width="17" height="17" aria-hidden="true">
      <rect x="4" y="3" width="16" height="18" rx="3.5" fill="none" stroke="currentColor" stroke-width="1.8" />
      <rect x="9" y="1.4" width="6" height="4" rx="1.6" fill="currentColor" />
      <path d="M8.5 11h7M8.5 14.5h7M8.5 18h4" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
    </svg>
    <input
      class="search"
      bind:this={searchInput}
      bind:value={query}
      oninput={scheduleSearch}
      placeholder="搜索剪贴板历史…"
      spellcheck="false"
      autocomplete="off"
    />
    {#if query}
      <button
        class="clear"
        title="清除搜索"
        onclick={() => {
          query = "";
          selected = 0;
          refresh();
        }}>✕</button
      >
    {/if}
    <button
      class="icon-btn"
      title={isDarkNow() ? "切到浅色界面" : "切到深色界面"}
      aria-label="切换浅色/深色"
      onclick={toggleTheme}
    >
      {#if isDarkNow()}
        <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
          <circle cx="12" cy="12" r="4.2" fill="none" stroke="currentColor" stroke-width="1.8" />
          <path
            d="M12 2.6v2.2M12 19.2v2.2M2.6 12h2.2M19.2 12h2.2M5.2 5.2l1.6 1.6M17.2 17.2l1.6 1.6M18.8 5.2l-1.6 1.6M6.8 17.2l-1.6 1.6"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
          />
        </svg>
      {:else}
        <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
          <path
            d="M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5Z"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linejoin="round"
          />
        </svg>
      {/if}
    </button>
  </header>

  {#if toast}
    <div class="toast" class:warn={toastKind === "warn"}>{toast}</div>
  {/if}

  <div class="body">
    <div class="list" bind:this={listEl}>
      {#if loading}
        <div class="empty">载入中…</div>
      {:else if entries.length === 0}
        <div class="empty">{query ? "没有匹配的记录" : "还没有剪贴板历史 — 复制点东西试试"}</div>
      {:else}
        {#each entries as entry, i (entry.id)}
          <div
            class="row"
            class:selected={i === selected}
            class:checked={checked.has(entry.id)}
            role="button"
            tabindex="-1"
            onclick={(e) => onRowClick(e, i)}
            ondblclick={() => pasteAt(i)}
          >
            <label
              class="check"
              title="勾选（Ctrl+点击、Shift+点击连选）"
              onclick={(e) => e.stopPropagation()}
            >
              <input
                type="checkbox"
                checked={checked.has(entry.id)}
                onchange={() => toggleChecked(entry.id)}
                aria-label="选择这条记录"
              />
            </label>
            <div class="thumb">
              {#if entry.kind === "image" && entry.thumbPath}
                <img src={convertFileSrc(entry.thumbPath)} alt="图片缩略图" loading="lazy" />
              {:else}
                <span class="text-glyph">T</span>
              {/if}
            </div>
            <div class="row-main">
              {#if entry.kind === "text"}
                <div class="row-text">{entry.content}</div>
              {:else}
                <div class="row-text faint">图片 · {entry.width}×{entry.height}</div>
              {/if}
              <div class="row-meta">
                <span>{formatTime(entry.createdAt)}</span>
                <span class="dot">·</span>
                <span>{formatBytes(entry.bytes)}</span>
                {#if entry.sourceApp}
                  <span class="dot">·</span>
                  <span>{entry.sourceApp}</span>
                {/if}
              </div>
            </div>

            <button
              class="pin-btn"
              class:pinned={entry.pinned != null}
              title={entry.pinned != null ? `已置顶到槽 ${entry.pinned}（点击换槽或取消）` : "置顶到槽位 1~9"}
              aria-label="置顶"
              onclick={(e) => {
                e.stopPropagation();
                openPinMenu(entry);
              }}
            >
              {entry.pinned != null ? `📌${entry.pinned}` : "📌"}
            </button>

            {#if pinMenuFor === entry.id}
              <div class="slot-menu" role="menu" onclick={(e) => e.stopPropagation()}>
                <div class="slot-menu-title">置顶到槽位</div>
                <div class="slot-menu-grid">
                  {#each Array(9) as _, k}
                    <button
                      class="slot-cell"
                      class:occupied={slots[k] != null}
                      title={slots[k]
                        ? `槽 ${k + 1} 已占用：${slots[k]!.kind === "image" ? "图片" : (slots[k]!.content ?? "").slice(0, 18)}（点击会替换）`
                        : `槽 ${k + 1} 是空的`}
                      onclick={() => assignSlot(entry, k + 1)}
                    >
                      {k + 1}
                    </button>
                  {/each}
                </div>
                {#if entry.pinned != null}
                  <button class="slot-unpin" onclick={() => assignSlot(entry, null)}>取消置顶（槽 {entry.pinned}）</button>
                {/if}
              </div>
            {/if}
          </div>
        {/each}
      {/if}
    </div>

    <aside class="preview">
      {#if selectedEntry()}
        {#if selectedEntry()!.kind === "image" && selectedEntry()!.imagePath}
          <img class="preview-img" src={convertFileSrc(selectedEntry()!.imagePath!)} alt="图片预览" />
          <div class="preview-meta muted">
            {selectedEntry()!.width}×{selectedEntry()!.height} · {formatBytes(selectedEntry()!.bytes)}
          </div>
        {:else}
          <pre class="preview-text">{selectedEntry()!.content}</pre>
          <div class="preview-meta muted">
            {selectedEntry()!.content?.length ?? 0} 字符 · {selectedEntry()!.content?.split("\n").length ?? 0} 行
            · {formatBytes(selectedEntry()!.bytes)}
          </div>
        {/if}
      {:else}
        <div class="preview-hint faint">选中一条记录即可预览</div>
      {/if}
    </aside>
  </div>

  <footer class="footer">
    {#if checked.size}
      <span class="bulk">
        已勾选 <b>{checked.size}</b> 条
        <button class="bulk-btn" onclick={removeChecked}>删除勾选</button>
        <button class="bulk-btn" onclick={toggleAll}>{allChecked() ? "取消全选" : "全选"}</button>
        <button class="bulk-btn" onclick={() => (checked = new Set<string>())}>取消选择</button>
      </span>
      <span class="spacer"></span>
      <span class="hint"><span class="kbd">Del</span> 删除勾选</span>
    {:else}
      <span class="hint"><span class="kbd">↑</span><span class="kbd">↓</span> 选择</span>
      <span class="hint"><span class="kbd">Enter</span> 粘贴</span>
      <span class="hint"><span class="kbd">Ctrl</span>+<span class="kbd">Shift</span>+<span class="kbd">1-9</span> 直贴</span>
      <span class="hint"><span class="kbd">F2</span> 置顶</span>
      <span class="hint"><span class="kbd">Space</span> 勾选</span>
      <span class="hint"><span class="kbd">Del</span> 删除</span>
      <span class="hint"><span class="kbd">Ctrl</span>+点击 多选</span>
      <span class="hint"><span class="kbd">Esc</span> 关闭</span>
    {/if}
    <span class="count faint">{entries.length} 条</span>
  </footer>
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100vh;
    margin: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    /* Opaque on purpose: backdrop-filter is undependable inside WebView2, so a
       "frosted" panel would let the window behind bleed through. */
    background: var(--bg-elev);
    box-shadow: var(--shadow);
    overflow: hidden;
  }

  .search-row {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 11px 14px;
    border-bottom: 1px solid var(--border);
    color: var(--accent);
  }

  .logo {
    flex: none;
  }

  .icon-btn {
    flex: none;
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    color: var(--fg-muted);
  }

  .icon-btn:hover {
    background: var(--bg-sunken);
    color: var(--accent);
  }

  .search {
    flex: 1;
    min-width: 0;
    border: none;
    outline: none;
    background: transparent;
    font-size: 15px;
  }

  .search::placeholder {
    color: var(--fg-faint);
  }

  .clear {
    color: var(--fg-faint);
    font-size: 12px;
    padding: 2px 6px;
    border-radius: 6px;
  }

  .clear:hover {
    background: var(--bg-sunken);
  }

  .toast {
    padding: 7px 14px;
    font-size: 12px;
    background: var(--accent-soft);
    border-bottom: 1px solid var(--border);
  }

  .toast.warn {
    background: color-mix(in srgb, var(--warn) 18%, transparent);
  }

  .body {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .list {
    flex: 1.15;
    min-width: 0;
    overflow-y: auto;
    padding: 6px;
  }

  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 9px;
    border-radius: var(--radius-sm);
    cursor: default;
  }

  .row.selected {
    background: var(--accent-soft);
  }

  .row.checked {
    outline: 1px solid color-mix(in srgb, var(--accent) 55%, transparent);
  }

  .check {
    flex: none;
    display: grid;
    place-items: center;
    width: 18px;
  }

  .check input {
    width: 14px;
    height: 14px;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .pin-btn {
    flex: none;
    padding: 2px 6px;
    border-radius: 6px;
    font-size: 12px;
    line-height: 1.4;
    color: var(--fg-faint);
    filter: grayscale(1) opacity(0.7);
  }

  .pin-btn:hover {
    background: var(--bg-sunken);
    filter: none;
  }

  .pin-btn.pinned {
    color: var(--accent-fg);
    background: var(--accent);
    filter: none;
    font-weight: 600;
  }

  .slot-menu {
    position: absolute;
    right: 8px;
    top: 100%;
    z-index: 20;
    width: 208px;
    padding: 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    background: var(--bg-elev);
    box-shadow: var(--shadow);
  }

  .slot-menu-title {
    font-size: 11px;
    color: var(--fg-muted);
    margin-bottom: 7px;
  }

  .slot-menu-grid {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: 5px;
  }

  .slot-cell {
    height: 26px;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    background: var(--bg-sunken);
    font-family: var(--mono);
    font-size: 11.5px;
  }

  .slot-cell:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  .slot-cell.occupied {
    background: var(--accent-soft);
    color: var(--accent);
  }

  .slot-unpin {
    margin-top: 8px;
    width: 100%;
    padding: 4px 0;
    border-radius: 6px;
    font-size: 11.5px;
    color: var(--danger);
    border: 1px solid color-mix(in srgb, var(--danger) 40%, var(--border-strong));
  }

  .thumb {
    flex: none;
    width: 34px;
    height: 34px;
    display: grid;
    place-items: center;
    border-radius: 7px;
    border: 1px solid var(--border);
    background: var(--bg-sunken);
    overflow: hidden;
  }

  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .text-glyph {
    font-family: var(--mono);
    font-size: 13px;
    color: var(--fg-muted);
  }

  .row-main {
    flex: 1;
    min-width: 0;
  }

  .row-text {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .row-meta {
    margin-top: 2px;
    font-size: 11px;
    color: var(--fg-faint);
    display: flex;
    gap: 5px;
  }

  .dot {
    opacity: 0.6;
  }

  .pin {
    flex: none;
    display: grid;
    place-items: center;
    width: 19px;
    height: 19px;
    border-radius: 6px;
    background: var(--accent);
    color: var(--accent-fg);
    font-size: 11px;
    font-weight: 600;
  }

  .preview {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
    padding: 10px;
    overflow: hidden;
  }

  .preview-text {
    flex: 1;
    margin: 0;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.55;
    user-select: text;
  }

  .preview-img {
    flex: 1;
    min-height: 0;
    object-fit: contain;
    border-radius: var(--radius-sm);
    background: var(--bg-sunken);
  }

  .preview-meta {
    margin-top: 8px;
    font-size: 11px;
    text-align: center;
  }

  .preview-hint {
    margin: auto;
    font-size: 12px;
  }

  .empty {
    padding: 34px 12px;
    text-align: center;
    color: var(--fg-faint);
  }

  .footer {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px 9px;
    padding: 6px 12px;
    border-top: 1px solid var(--border);
    font-size: 11px;
    color: var(--fg-muted);
  }

  .count {
    margin-left: auto;
  }

  .hint {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    white-space: nowrap;
  }

  .spacer {
    flex: 1;
  }

  .bulk {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    color: var(--fg);
  }

  .bulk-btn {
    padding: 3px 9px;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    background: var(--bg-sunken);
    font-size: 11px;
  }

  .bulk-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
