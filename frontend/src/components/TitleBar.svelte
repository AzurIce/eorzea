<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";

  const win = getCurrentWindow();
</script>

<div class="titlebar">
  <div class="drag" data-tauri-drag-region="">
    <svg
      class="logo"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M12 2 L19 12 L12 22 L5 12 Z" />
      <path d="M12 7 L15.5 12 L12 17 L8.5 12 Z" />
    </svg>
    <span class="app-title">FFXIV 国服启动器</span>
  </div>
  <div class="win-btns">
    <button class="win-btn" title="最小化" onclick={() => win.minimize()}>
      <svg viewBox="0 0 12 12"><path d="M2 6h8" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" /></svg>
    </button>
    <button class="win-btn" title="最大化 / 还原" onclick={() => win.toggleMaximize()}>
      <svg viewBox="0 0 12 12"><rect x="2.5" y="2.5" width="7" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" /></svg>
    </button>
    <button class="win-btn close" title="关闭" onclick={() => win.close()}>
      <svg viewBox="0 0 12 12"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" /></svg>
    </button>
  </div>
</div>

<style>
  .titlebar {
    height: 40px;
    flex-shrink: 0;
    display: flex;
    align-items: stretch;
    background: var(--bg-1);
    border-bottom: 1px solid var(--border);
    user-select: none;
  }
  .drag {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    min-width: 0;
  }
  .logo {
    width: 16px;
    height: 16px;
    color: var(--accent);
    flex-shrink: 0;
  }
  .app-title {
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.5px;
    color: var(--text-dim);
  }
  .win-btns {
    display: flex;
    align-items: stretch;
  }
  .win-btn {
    width: 44px;
    border: none;
    border-radius: 0;
    background: transparent;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition:
      background 0.12s,
      color 0.12s;
  }
  .win-btn svg {
    width: 12px;
    height: 12px;
  }
  .win-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }
  .win-btn.close:hover {
    background: #e81123;
    color: #fff;
  }
</style>
