<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import {
    getConfig,
    setConfig,
    clearEntries,
    setPaused,
    getPaused,
    dataDir,
    formatBytes,
    pinnedSlots,
    pinEntry,
    openDataDir,
    migrateDataDir,
    resetDataDir,
    restartApp,
    setRecording,
    EVENT_CONFIG_CHANGED,
    type AppConfig,
    type ClipEntry,
    type ThemeMode,
  } from "../lib/api";
  import { applyTheme } from "../lib/theme";

  /** 后端在剪贴板历史发生变化时推送的事件（浮层里改动置顶后靠它同步）。 */
  const EVENT_CHANGED = "betterpaste://changed";

  /** 这些组合被系统/常见应用占用，抢到手会直接破坏用户的日常操作。 */
  const DANGEROUS_HOTKEYS = new Set([
    "Ctrl+V",
    "Ctrl+C",
    "Ctrl+X",
    "Ctrl+Z",
    "Ctrl+A",
    "Ctrl+S",
    "Ctrl+P",
    "Ctrl+F",
  ]);

  let cfg: AppConfig | null = null;
  let dir = "";
  let savedAt = "";
  let toast = "";
  let toastKind: "info" | "warn" = "info";
  let paused = false;
  let saving = false;
  let dirty = false;
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let unlisteners: UnlistenFn[] = [];

  /** 9 个置顶槽当前的内容（下标 0 = 槽 1，空槽是 null）。 */
  let slots: (ClipEntry | null)[] = Array.from({ length: 9 }, () => null);
  /** 「数据位置」卡片里可编辑的目标目录，默认填当前目录。 */
  let dirInput = "";
  let migrating = false;
  let resettingDir = false;
  /** 呼出热键输入框是否正在录制（录制期间后端不抢全局热键）。 */
  let recording = false;
  let hotkeyEl: HTMLInputElement | null = null;

  const DEFAULTS: AppConfig = {
    maxEntries: 500,
    maxAgeDays: 30,
    maxItemMb: 10,
    historyLimit: 500,
    captureText: true,
    captureImages: true,
    hotkey: "Ctrl+Shift+V",
    slotHotkeys: Array.from({ length: 9 }, (_, i) => `Ctrl+Shift+${i + 1}`),
    theme: "system",
    closeAfterPaste: true,
    hideOnBlur: true,
    autostart: false,
    ignoreSourceApps: [],
  };

  function showToast(msg: string, kind: "info" | "warn" = "info") {
    toast = msg;
    toastKind = kind;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ""), 2800);
  }

  function markDirty() {
    dirty = true;
    savedAt = "";
  }

  /* ---------------------------------------------------------------- 热键录制
     录制期间要告诉后端「别抢全局热键」，否则用户按下 Ctrl+Shift+V 会被浮层
     截胡、根本写不进输入框。任何一条退出路径都必须关掉录制，这里全部走
     `recordingOff()`；它是幂等的，重复调用无害。 */

  /** 只负责通知后端，不碰本地状态（卸载时也能安全调用）。 */
  function notifyRecordingOff() {
    if (!recording) return;
    void setRecording(false).catch((e) => showToast(`关闭录制失败：${e}`, "warn"));
  }

  async function recordingOn() {
    if (recording) return;
    recording = true;
    try {
      await setRecording(true);
    } catch (e) {
      recording = false;
      showToast(`无法进入录制状态：${e}`, "warn");
    }
  }

  function recordingOff() {
    notifyRecordingOff();
    recording = false;
  }

  /** 把主题选择立刻套到 <html>：system = 跟随系统（去掉属性），其余写成 light/dark。 */
  function applyThemeNow(mode: ThemeMode) {
    if (mode === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.setAttribute("data-theme", mode);
  }

  function onThemeChange(e: Event) {
    applyThemeNow((e.currentTarget as HTMLSelectElement).value as ThemeMode);
    markDirty();
  }

  /** 录到这些组合时给出具体到键位的提示，别只说"不行"。 */
  const KEY_MEANING: Record<string, string> = {
    V: "粘贴",
    C: "复制",
    X: "剪切",
    Z: "撤销",
    A: "全选",
    S: "保存",
    P: "打印",
    F: "查找",
  };

  function dangerousHint(s: string): string {
    const key = s.slice("Ctrl+".length);
    return `${s} 是系统${KEY_MEANING[key] ?? "快捷键"}，注册后会被浮层抢走、任何地方都用不了，请换一个组合（例如 Ctrl+Shift+${key}）`;
  }

  function isDangerous(s: string): boolean {
    return DANGEROUS_HOTKEYS.has(s);
  }

  /** 槽位内容的单行预览：文本取前 24 字（换行 → ↵），图片给出尺寸。 */
  function slotPreview(e: ClipEntry): string {
    if (e.kind === "image") return `图片 ${e.width ?? "?"}×${e.height ?? "?"}`;
    const text = (e.content ?? "").slice(0, 24).replace(/\r?\n/g, "↵");
    return text || "（空文本）";
  }

  /** 悬浮提示用：图片给路径、文本给前 200 字。 */
  function slotFullText(e: ClipEntry): string {
    if (e.kind === "image") return e.imagePath ?? slotPreview(e);
    return (e.content ?? "").slice(0, 200) || "（空文本）";
  }

  async function refreshSlots() {
    try {
      slots = await pinnedSlots();
    } catch (e) {
      showToast(`无法读取置顶槽：${e}`, "warn");
    }
  }

  async function clearSlot(slot: number) {
    const entry = slots[slot - 1];
    if (!entry) return;
    try {
      await pinEntry(entry.id, null);
      showToast(`已取消槽 ${slot} 的置顶`);
      await refreshSlots();
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  async function openDir() {
    try {
      await openDataDir();
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  async function migrateDir() {
    const target = dirInput.trim();
    if (!target) {
      showToast("请先填写目标目录", "warn");
      return;
    }
    if (
      !confirm(
        `确定把数据目录迁移到：\n${target}\n\n会把现有历史 / 图片 / 配置复制到新目录，原目录保留不删，随后自动重启生效。`
      )
    )
      return;
    migrating = true;
    try {
      const next = await migrateDataDir(target);
      showToast(`已迁移到 ${next}，正在重启…`);
      await restartApp();
    } catch (e) {
      showToast(String(e), "warn");
    } finally {
      migrating = false;
    }
  }

  async function restoreDefaultDir() {
    if (!confirm("确定恢复默认数据目录吗？\n\n会记录回默认位置，随后自动重启生效；当前目录的数据不会被删除。"))
      return;
    resettingDir = true;
    try {
      const next = await resetDataDir();
      showToast(`已恢复默认位置 ${next}，正在重启…`);
      await restartApp();
    } catch (e) {
      showToast(String(e), "warn");
    } finally {
      resettingDir = false;
    }
  }

  async function save() {
    if (!cfg) return;
    saving = true;
    try {
      cfg = await setConfig(cfg);
      dirty = false;
      savedAt = new Date().toLocaleTimeString();
      await applyTheme();
      await refreshSlots();
      showToast("已保存");
    } catch (e) {
      showToast(String(e), "warn");
    } finally {
      saving = false;
    }
  }

  async function resetDefaults() {
    if (!cfg) return;
    const keepHotkey = cfg.hotkey;
    cfg = { ...DEFAULTS, hotkey: keepHotkey };
    markDirty();
    showToast("已恢复默认值，别忘了保存");
  }

  async function togglePause() {
    try {
      paused = await setPaused(!paused);
      showToast(paused ? "已暂停记录" : "已恢复记录");
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  async function wipe(keepPinned: boolean) {
    const what = keepPinned ? "未置顶的历史记录" : "全部历史记录";
    if (!confirm(`确定要清空${what}吗？此操作不可撤销。`)) return;
    try {
      const n = await clearEntries(keepPinned);
      showToast(`已删除 ${n} 条记录`);
    } catch (e) {
      showToast(String(e), "warn");
    }
  }

  /** 把一次按键事件转成 betterPaste 的热键串，例如 "Ctrl+Shift+V"。 */
  function shortcutFromEvent(e: KeyboardEvent): string | null {
    const mods: string[] = [];
    if (e.ctrlKey) mods.push("Ctrl");
    if (e.altKey) mods.push("Alt");
    if (e.shiftKey) mods.push("Shift");
    if (e.metaKey) mods.push("Super");

    const code = e.code;
    let key: string | null = null;
    if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
    else if (/^Digit[0-9]$/.test(code)) key = code.slice(5);
    else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) key = code;
    else if (["Space", "Escape", "Tab", "Enter", "Backspace"].includes(code)) key = code;

    if (!key) return null;
    // 修饰键本身不能作为主键
    if (["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight"].includes(code))
      return null;
    if (mods.length === 0) return null; // 至少要有一个修饰键，避免抢普通输入
    return [...mods, key].join("+");
  }

  function onHotkeyFocus() {
    if (!cfg) return;
    hotkeyEl?.select();
    void recordingOn();
  }

  /** blur 也要兜底关掉录制（用户点到别处、切窗口、窗口失焦都会走这里）。 */
  function onHotkeyBlur() {
    recordingOff();
  }

  async function captureHotkey(e: KeyboardEvent) {
    e.preventDefault();
    if (!cfg) return;
    const s = shortcutFromEvent(e);
    if (!s) {
      showToast("请按「修饰键 + 主键」组合，例如 Ctrl+Shift+V", "warn");
      return;
    }
    if (isDangerous(s)) {
      showToast(dangerousHint(s), "warn");
      recordingOff();
      return;
    }
    cfg.hotkey = s;
    markDirty();
    recordingOff();
    showToast(`已录到 ${s}，点右上角「保存」后生效`);
  }

  onMount(async () => {
    try {
      cfg = await getConfig();
      await applyTheme();
      dir = await dataDir();
      dirInput = dir;
      paused = await getPaused();
    } catch (e) {
      showToast(`无法读取配置：${e}`, "warn");
    }
    await refreshSlots();

    // 主题在浮层 / 设置面板 / 托盘之间同步；置顶槽在浮层里被改动后也要跟着刷新。
    unlisteners = await Promise.all([
      listen(EVENT_CONFIG_CHANGED, () => {
        void applyTheme().then((mode) => {
          if (cfg && cfg.theme !== mode) cfg.theme = mode;
        });
      }),
      listen(EVENT_CHANGED, () => void refreshSlots()),
    ]);
  });

  onDestroy(() => {
    clearTimeout(toastTimer);
    // 兜底：面板被关掉 / 组件卸载时绝不能把全局热键留在「录制中」状态。
    notifyRecordingOff();
    for (const un of unlisteners) un();
  });
</script>

<div class="wrap">
  <header>
    <div>
      <h1>betterPaste 设置</h1>
      <p class="muted sub">本地离线剪贴板历史 · 数据只存在你自己的电脑上</p>
    </div>
    <div class="header-actions">
      <button class="btn" onclick={togglePause}>{paused ? "恢复记录" : "暂停记录"}</button>
      <button class="btn primary" onclick={save} disabled={saving || !dirty}>
        {saving ? "保存中…" : dirty ? "保存" : "已保存"}
      </button>
    </div>
  </header>

  {#if toast}
    <div class="toast" class:warn={toastKind === "warn"}>{toast}</div>
  {/if}

  {#if !cfg}
    <div class="card muted">载入中…</div>
  {:else}
    <section class="card">
      <h2>历史与清理</h2>
      <p class="muted note">超出上限的最旧记录会被自动删除，已置顶的条目不受条数上限影响。</p>

      <label class="field">
        <span>历史条数上限</span>
        <input
          type="number"
          min="0"
          max="100000"
          bind:value={cfg.maxEntries}
          oninput={markDirty}
        />
        <em class="muted">条（0 = 不限制）</em>
      </label>

      <label class="field">
        <span>保留天数</span>
        <input type="number" min="0" max="3650" bind:value={cfg.maxAgeDays} oninput={markDirty} />
        <em class="muted">天（0 = 不限制）</em>
      </label>

      <label class="field">
        <span>单条体积上限</span>
        <input type="number" min="1" max="1024" bind:value={cfg.maxItemMb} oninput={markDirty} />
        <em class="muted">MB</em>
      </label>

      <label class="field">
        <span>浮层最多列出</span>
        <input type="number" min="10" max="5000" bind:value={cfg.historyLimit} oninput={markDirty} />
        <em class="muted">条</em>
      </label>

      <div class="danger-row">
        <button class="btn" onclick={() => wipe(true)}>清空未置顶记录</button>
        <button class="btn danger" onclick={() => wipe(false)}>清空全部记录</button>
      </div>
    </section>

    <section class="card">
      <h2>记录内容</h2>
      <label class="switch">
        <input type="checkbox" bind:checked={cfg.captureText} onchange={markDirty} />
        <span>记录文本</span>
        <em class="muted">纯文本；代码缩进与换行逐字节保真</em>
      </label>
      <label class="switch">
        <input type="checkbox" bind:checked={cfg.captureImages} onchange={markDirty} />
        <span>记录图片</span>
        <em class="muted">位图存为 PNG，并生成缩略图</em>
      </label>
      <p class="muted note">文件 / 文件夹与富文本（HTML、RTF）本期不支持，会被自动忽略。</p>
    </section>

    <section class="card">
      <h2>热键</h2>
      <p class="muted note">点进输入框后直接按下想要的组合键即可录制。</p>

      <label class="field">
        <span>呼出浮层</span>
        <input
          class="hotkey"
          class:recording
          readonly
          value={cfg.hotkey}
          placeholder="请按下组合键…"
          bind:this={hotkeyEl}
          onkeydown={captureHotkey}
          onfocus={onHotkeyFocus}
          onblur={onHotkeyBlur}
        />
      </label>
      <p class="hotkey-hint faint">
        点进输入框后直接按下组合键即可录制；<strong>录制完要点右上角「保存」才会生效</strong>。
      </p>

      <div class="slots">
        <span class="slots-label">置顶槽直贴</span>
        <p class="muted note slot-note">
          空槽（在浮层里选中条目按 F2 置顶，或按本槽热键 Ctrl+Shift+N 直接贴过去）
        </p>
        <div class="slot-rows">
          {#each Array(9) as _, i}
            {@const entry = slots[i]}
            <div class="slot-row">
              <span class="slot-no faint">槽 {i + 1}</span>
              <div
                class="slot-content"
                class:empty={!entry}
                title={entry ? slotFullText(entry) : "该槽还没有置顶内容"}
              >
                {entry ? slotPreview(entry) : "空槽"}
              </div>
              <span
                class="kbd slot-kbd"
                title="槽位热键固定为 Ctrl+Shift+{i + 1}，不可修改"
              >Ctrl+Shift+{i + 1}</span>
              {#if entry}
                <button
                  class="btn tiny"
                  type="button"
                  title={`取消槽 ${i + 1} 的置顶`}
                  onclick={() => clearSlot(i + 1)}>清空</button
                >
              {:else}
                <span class="slot-holder"></span>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    </section>

    <section class="card">
      <h2>浮层行为</h2>
      <label class="switch">
        <input type="checkbox" bind:checked={cfg.closeAfterPaste} onchange={markDirty} />
        <span>粘贴后关闭浮层</span>
      </label>
      <label class="switch">
        <input type="checkbox" bind:checked={cfg.hideOnBlur} onchange={markDirty} />
        <span>失去焦点时自动关闭浮层</span>
      </label>

      <label class="field">
        <span>界面主题</span>
        <select bind:value={cfg.theme} onchange={onThemeChange}>
          <option value="system">跟随系统</option>
          <option value="light">浅色</option>
          <option value="dark">深色</option>
        </select>
        <em class="muted">改完立即生效，无需保存</em>
      </label>

      <label class="switch">
        <input type="checkbox" bind:checked={cfg.autostart} onchange={markDirty} />
        <span>开机自动启动</span>
      </label>
    </section>

    <section class="card">
      <h2>数据位置</h2>
      <p class="mono path">{dir || "…"}</p>
      <p class="muted note">
        数据库、图片与缩略图都在这里。完全离线运行，不会向任何服务器发送数据。
      </p>

      <div class="field">
        <span>目标目录</span>
        <input
          class="mono dir-input"
          type="text"
          bind:value={dirInput}
          placeholder="例如 D:\betterPaste-data"
          spellcheck="false"
        />
      </div>

      <div class="danger-row">
        <button class="btn" onclick={openDir}>打开目录</button>
        <button class="btn primary" onclick={migrateDir} disabled={migrating}>
          {migrating ? "迁移中…" : "迁移并重启"}
        </button>
        <button class="btn" onclick={restoreDefaultDir} disabled={resettingDir}>
          {resettingDir ? "处理中…" : "恢复默认位置"}
        </button>
        <button class="btn" onclick={resetDefaults}>恢复默认设置</button>
      </div>

      <p class="muted note after-actions">
        迁移会把现有历史 / 图片 / 配置复制到新目录（原目录保留不删），重启后生效。
      </p>
    </section>

    <footer class="foot muted">
      betterPaste v0.2.1 · MIT License
      {#if savedAt}· 上次保存 {savedAt}{/if}
    </footer>
  {/if}
</div>

<style>
  .wrap {
    max-width: 720px;
    margin: 0 auto;
    padding: 22px 24px 40px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  /* 页头吸顶：滚到底部改设置时「保存」按钮始终看得见。 */
  header {
    position: sticky;
    top: 0;
    z-index: 20;
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    padding: 8px 0 10px;
    margin-top: -8px;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
  }

  h1 {
    margin: 0;
    font-size: 19px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }

  .sub {
    margin: 5px 0 0;
    font-size: 12px;
  }

  h2 {
    margin: 0 0 10px;
    font-size: 13px;
    font-weight: 650;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }

  .header-actions {
    display: flex;
    gap: 8px;
    flex: none;
  }

  .card {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-elev);
    padding: 15px 16px;
  }

  .note {
    margin: 0 0 12px;
    font-size: 12px;
    line-height: 1.5;
  }

  .field {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 0;
    font-size: 13px;
  }

  .field > span {
    flex: none;
    width: 120px;
    color: var(--fg-muted);
  }

  .field input,
  .field select {
    padding: 6px 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    background: var(--bg-sunken);
    outline: none;
    min-width: 0;
  }

  .field input:focus,
  .field select:focus {
    border-color: var(--accent);
  }

  .field input[type="number"] {
    width: 120px;
  }

  .field em {
    font-style: normal;
    font-size: 12px;
  }

  .hotkey {
    flex: 1;
    max-width: 220px;
    font-family: var(--mono);
    cursor: pointer;
  }

  /* 录制中：边框点亮 + 轻微光晕，让用户明确知道「现在在录」。 */
  .hotkey.recording {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }

  .hotkey.recording::placeholder {
    color: var(--accent);
    opacity: 1;
  }

  .hotkey-hint {
    margin: -2px 0 10px;
    padding-left: 130px;
    font-size: 11.5px;
    line-height: 1.5;
  }

  .hotkey-hint strong {
    color: var(--fg);
    font-weight: 650;
  }

  .slots {
    padding: 7px 0;
    font-size: 13px;
  }

  .slots-label {
    display: block;
    color: var(--fg-muted);
    margin-bottom: 8px;
  }

  .slot-note {
    margin: 8px 0 0;
  }

  .slot-rows {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .slot-row {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
  }

  .slot-no {
    flex: none;
    width: 34px;
    font-size: 11px;
    letter-spacing: 0.02em;
  }

  /* 内容预览框：`.path` 的观感（sunken 底 + 1px 边 + mono 小字），flex: 1 占满。 */
  .slot-content {
    flex: 1;
    min-width: 0;
    padding: 6px 9px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-sunken);
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--fg);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .slot-content.empty {
    color: var(--fg-faint);
  }

  .slot-kbd {
    flex: none;
    min-width: 74px;
    height: auto;
    padding: 3px 6px;
    color: var(--fg-muted);
  }

  /* 空槽时占住「清空」按钮的宽度，保证每一行的快捷键列对齐。 */
  .slot-holder {
    flex: none;
    width: 34px;
  }

  .btn.tiny {
    flex: none;
    padding: 1px 6px;
    font-size: 10.5px;
    line-height: 1.5;
  }

  .switch {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 7px 0;
    font-size: 13px;
    cursor: pointer;
  }

  .switch input {
    flex: none;
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
  }

  .switch em {
    font-style: normal;
    font-size: 12px;
  }

  .btn {
    padding: 7px 13px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    background: var(--bg-sunken);
    font-size: 12.5px;
    transition: filter 0.12s ease;
  }

  .btn:hover:not(:disabled) {
    filter: brightness(1.08);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-fg);
    font-weight: 600;
  }

  .btn.danger {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 45%, var(--border-strong));
  }

  .danger-row {
    display: flex;
    gap: 8px;
    margin-top: 12px;
    flex-wrap: wrap;
  }

  .dir-input {
    flex: 1;
    min-width: 0;
  }

  .after-actions {
    margin: 10px 0 0;
  }

  /* 固定悬浮：滚到页面任何位置都能看到反馈。 */
  .toast {
    position: fixed;
    left: 50%;
    bottom: 22px;
    transform: translateX(-50%);
    z-index: 60;
    max-width: 88vw;
    padding: 8px 13px;
    border-radius: var(--radius-sm);
    box-shadow: var(--shadow);
    /* accent-soft 是半透明的，悬浮层下面会透出正文，这里混一点不透明底色。 */
    background: color-mix(in srgb, var(--accent-soft) 75%, var(--bg-elev));
    font-size: 12.5px;
    text-align: center;
  }

  .toast.warn {
    background: color-mix(in srgb, var(--warn) 22%, var(--bg-elev));
  }

  .mono {
    font-family: var(--mono);
    font-size: 12px;
    word-break: break-all;
  }

  .path {
    margin: 0 0 8px;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    background: var(--bg-sunken);
    border: 1px solid var(--border);
  }

  .foot {
    text-align: center;
    font-size: 11.5px;
    padding-top: 6px;
  }
</style>
