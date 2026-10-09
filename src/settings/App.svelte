<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    getConfig,
    setConfig,
    clearEntries,
    setPaused,
    getPaused,
    dataDir,
    formatBytes,
    type AppConfig,
    type ThemeMode,
  } from "../lib/api";
  import { applyTheme } from "../lib/theme";

  let cfg: AppConfig | null = null;
  let dir = "";
  let savedAt = "";
  let toast = "";
  let toastKind: "info" | "warn" = "info";
  let paused = false;
  let saving = false;
  let dirty = false;
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

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

  async function save() {
    if (!cfg) return;
    saving = true;
    try {
      cfg = await setConfig(cfg);
      dirty = false;
      savedAt = new Date().toLocaleTimeString();
      await applyTheme();
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

  /** 把一次按键事件转成 clipstack 的热键串，例如 "Ctrl+Shift+V"。 */
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

  function captureHotkey(e: KeyboardEvent) {
    e.preventDefault();
    if (!cfg) return;
    const s = shortcutFromEvent(e);
    if (!s) {
      showToast("请按「修饰键 + 主键」组合，例如 Ctrl+Shift+V", "warn");
      return;
    }
    cfg.hotkey = s;
    markDirty();
  }

  function captureSlot(e: KeyboardEvent, slot: number) {
    e.preventDefault();
    if (!cfg) return;
    const s = shortcutFromEvent(e);
    if (!s) {
      showToast("请按「修饰键 + 主键」组合", "warn");
      return;
    }
    cfg.slotHotkeys[slot - 1] = s;
    markDirty();
  }

  onMount(async () => {
    try {
      cfg = await getConfig();
      await applyTheme();
      dir = await dataDir();
      paused = await getPaused();
    } catch (e) {
      showToast(`无法读取配置：${e}`, "warn");
    }
  });

  onDestroy(() => clearTimeout(toastTimer));
</script>

<div class="wrap">
  <header>
    <div>
      <h1>clipstack 设置</h1>
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
          readonly
          value={cfg.hotkey}
          onkeydown={captureHotkey}
          onfocus={(e) => e.currentTarget.select()}
        />
      </label>

      <div class="slots">
        <span class="slots-label">置顶槽直贴</span>
        <div class="slot-grid">
          {#each cfg.slotHotkeys as s, i}
            <input
              class="hotkey small"
              readonly
              value={s}
              onkeydown={(e) => captureSlot(e, i + 1)}
              title={`槽位 ${i + 1}`}
            />
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
        <select bind:value={cfg.theme} onchange={markDirty}>
          <option value="system">跟随系统</option>
          <option value="light">浅色</option>
          <option value="dark">深色</option>
        </select>
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
      <div class="danger-row">
        <button class="btn" onclick={resetDefaults}>恢复默认设置</button>
      </div>
    </section>

    <footer class="foot muted">
      clipstack v0.1.0 · MIT License
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

  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    padding-bottom: 4px;
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

  .hotkey.small {
    max-width: none;
    width: 100%;
    padding: 5px 7px;
    font-size: 11.5px;
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

  .slot-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 7px;
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

  .toast {
    padding: 8px 13px;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    font-size: 12.5px;
  }

  .toast.warn {
    background: color-mix(in srgb, var(--warn) 18%, transparent);
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
