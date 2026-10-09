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
    getConfig,
    hideOverlay,
    formatBytes,
    formatTime,
    type AppConfig,
    type ClipEntry,
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
        if (e.ctrlKey) {
          e.preventDefault();
          const entry = selectedEntry();
          if (entry) await removeEntry(entry);
        }
        return;
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

    unlisteners.push(
      await listen("clipstack://changed", () => refresh()),
      await listen("clipstack://paused", () => {}),
      await listen("tauri://focus", () => {
        // 每次浮层重新获得焦点都从最新一条开始。
        // 之前 selected 会沿用上次的下标（refresh() 只做 clamp），而新复制的内容总是
        // 插在列表最前面，于是"同一个下标"指向的条目整体下移 → 直接回车粘错条目。
        selected = 0;
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
            role="button"
            tabindex="-1"
            onclick={() => (selected = i)}
            ondblclick={() => pasteAt(i)}
          >
            <div class="thumb">
              {#if entry.kind === "image" && entry.thumbPath}
                <img src={convertFileSrc(entry.thumbPath)} alt="图片缩略图" />
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
            {#if entry.pinned != null}
              <span class="pin" title={`置顶槽 ${entry.pinned}`}>{entry.pinned}</span>
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
    <span class="hint"><span class="kbd">↑</span><span class="kbd">↓</span> 选择</span>
    <span class="hint"><span class="kbd">Enter</span> 粘贴</span>
    <span class="hint"><span class="kbd">Ctrl</span>+<span class="kbd">Shift</span>+<span class="kbd">1-9</span> 直贴槽</span>
    <span class="hint"><span class="kbd">Ctrl</span>+<span class="kbd">P</span> 置顶</span>
    <span class="hint"><span class="kbd">Ctrl</span>+<span class="kbd">Del</span> 删除</span>
    <span class="spacer"></span>
    <span class="hint"><span class="kbd">Esc</span> 关闭</span>
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
    gap: 11px;
    padding: 7px 13px;
    border-top: 1px solid var(--border);
    font-size: 11px;
    color: var(--fg-muted);
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
</style>
