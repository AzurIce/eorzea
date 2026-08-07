<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "./lib/api";
  import type { AccountInfo } from "./lib/types";
  import TitleBar from "./components/TitleBar.svelte";
  import LoginView from "./components/LoginView.svelte";
  import MainView from "./components/MainView.svelte";
  import SettingsView from "./components/SettingsView.svelte";

  type View = "login" | "main" | "settings";

  let view = $state<View>("login");
  let accounts = $state<AccountInfo[]>([]);
  /** 本次会话内已成功登录（后端已缓存 token）的账号。 */
  let loggedIn = $state<Set<string>>(new Set());
  let errorMsg = $state("");
  let noticeMsg = $state("");
  let ready = $state(false);

  async function refreshAccounts() {
    accounts = await api.listAccounts();
  }

  function showError(e: unknown) {
    errorMsg = api.errMsg(e);
    noticeMsg = "";
  }

  function showNotice(msg: string) {
    noticeMsg = msg;
    errorMsg = "";
  }

  function clearMessages() {
    errorMsg = "";
    noticeMsg = "";
  }

  function markLoggedIn(sndaId: string) {
    const next = new Set(loggedIn);
    next.add(sndaId);
    loggedIn = next;
  }

  function onLoggedIn(acc: AccountInfo) {
    markLoggedIn(acc.snda_id);
    refreshAccounts().catch(showError);
    showNotice(`登录成功：${acc.display_name}`);
    view = "main";
  }

  function onAccountsChanged() {
    refreshAccounts().catch(showError);
  }

  function switchView(v: View) {
    clearMessages();
    view = v;
  }

  onMount(async () => {
    try {
      await refreshAccounts();
      // 已有可用账号时直接进入主界面
      view = accounts.length > 0 ? "main" : "login";
    } catch (e) {
      showError(e);
      view = "login";
    } finally {
      ready = true;
    }
  });
</script>

<div class="app">
  <TitleBar />

  <div class="banner-layer">
    {#if errorMsg}
      <div class="banner error">
        <span>{errorMsg}</span>
        <button class="close" onclick={() => (errorMsg = "")}>×</button>
      </div>
    {/if}
    {#if noticeMsg}
      <div class="banner notice">
        <span>{noticeMsg}</span>
        <button class="close" onclick={() => (noticeMsg = "")}>×</button>
      </div>
    {/if}
  </div>

  <div class="body">
    <aside class="sidebar">
      <button
        class="nav-item"
        class:active={view === "main"}
        onclick={() => switchView("main")}
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M6 11h4l1.5-4 3 8 1.5-4h2" />
          <rect x="2" y="6" width="20" height="12" rx="4" />
        </svg>
        主界面
      </button>
      <button
        class="nav-item"
        class:active={view === "login"}
        onclick={() => switchView("login")}
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="8" r="4" />
          <path d="M4 21c0-4 3.5-6.5 8-6.5s8 2.5 8 6.5" />
        </svg>
        账号登录
      </button>
      <button
        class="nav-item"
        class:active={view === "settings"}
        onclick={() => switchView("settings")}
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1 1.55V21a2 2 0 1 1-4 0v-.09a1.7 1.7 0 0 0-1-1.55 1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-1.55-1H3a2 2 0 1 1 0-4h.09a1.7 1.7 0 0 0 1.55-1 1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34h.01a1.7 1.7 0 0 0 1-1.55V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1 1.55 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87v.01a1.7 1.7 0 0 0 1.55 1H21a2 2 0 1 1 0 4h-.09a1.7 1.7 0 0 0-1.55 1z" />
        </svg>
        设置
      </button>
      <div class="sidebar-foot muted">v0.1.0</div>
    </aside>

    <main class="content">
      {#if !ready}
        <p class="muted">加载中…</p>
      {:else if view === "login"}
        <LoginView
          {accounts}
          {onLoggedIn}
          {onAccountsChanged}
          onError={showError}
        />
      {:else if view === "main"}
        <MainView
          {accounts}
          {loggedIn}
          {markLoggedIn}
          onError={showError}
          onNotice={showNotice}
          goSettings={() => switchView("settings")}
          goLogin={() => switchView("login")}
        />
      {:else}
        <SettingsView onError={showError} onNotice={showNotice} />
      {/if}
    </main>
  </div>
</div>

<style>
  .sidebar {
    width: 172px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 14px 10px;
    background: var(--bg-1);
    border-right: 1px solid var(--border);
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 38px;
    padding: 0 12px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-dim);
    font-size: 13px;
    text-align: left;
    width: 100%;
  }
  .nav-item svg {
    width: 17px;
    height: 17px;
    flex-shrink: 0;
  }
  .nav-item:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text);
    transform: none;
  }
  .nav-item.active {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .sidebar-foot {
    margin-top: auto;
    padding: 4px 12px;
    font-size: 11px;
  }
</style>
