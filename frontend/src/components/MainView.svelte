<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import * as api from "../lib/api";
  import type {
    AccountInfo,
    CheckResult,
    GameStatus,
    PatchProgress,
    SdoArea,
  } from "../lib/types";

  let {
    accounts,
    loggedIn,
    markLoggedIn,
    onError,
    onNotice,
    goSettings,
    goLogin,
  }: {
    accounts: AccountInfo[];
    loggedIn: Set<string>;
    markLoggedIn: (sndaId: string) => void;
    onError: (e: unknown) => void;
    onNotice: (msg: string) => void;
    goSettings: () => void;
    goLogin: () => void;
  } = $props();

  let areas = $state<SdoArea[]>([]);
  let selectedAccount = $state("");
  let selectedArea = $state("");
  let rootValid = $state(false);
  let status = $state<GameStatus | null>(null);
  let statusError = $state("");
  let checkResult = $state<CheckResult | null>(null);
  let checking = $state(false);
  let updating = $state(false);
  let progress = $state<PatchProgress | null>(null);
  let launching = $state(false);
  let launchMsg = $state("");

  // 默认选中默认账号（无默认账号时选第一个）
  $effect(() => {
    if (
      accounts.length > 0 &&
      !accounts.some((a) => a.snda_id === selectedAccount)
    ) {
      selectedAccount = (
        accounts.find((a) => a.is_default) ?? accounts[0]
      ).snda_id;
    }
  });

  const progressPct = $derived(
    progress && progress.total > 0
      ? Math.min(100, (progress.downloaded / progress.total) * 100)
      : 0,
  );

  onMount(async () => {
    try {
      const list = await api.listAreas();
      areas = [...list].sort((a, b) => a.AreaOrder - b.AreaOrder);
      if (areas.length > 0) selectedArea = areas[0].Areaid;
    } catch (e) {
      onError(e);
    }
    await refreshStatus();
  });

  async function refreshStatus() {
    try {
      rootValid = await api.gameRootValid();
      if (rootValid) {
        status = await api.gameStatus();
        statusError = "";
      } else {
        status = null;
      }
    } catch (e) {
      status = null;
      statusError = api.errMsg(e);
    }
  }

  async function doCheck() {
    if (!selectedArea) return;
    checking = true;
    checkResult = null;
    try {
      checkResult = await api.checkGame(selectedArea);
    } catch (e) {
      onError(e);
    } finally {
      checking = false;
    }
  }

  async function doUpdate() {
    if (!selectedArea || updating) return;
    updating = true;
    progress = null;
    const unlisten = await listen<PatchProgress>(
      "patch-progress",
      (ev) => (progress = ev.payload),
    );
    try {
      const msg = await api.updateGame(selectedArea);
      onNotice(msg);
      checkResult = null;
      await refreshStatus();
    } catch (e) {
      onError(e);
    } finally {
      unlisten();
      updating = false;
      progress = null;
    }
  }

  async function doLaunch() {
    if (!selectedAccount || !selectedArea || launching) return;
    launching = true;
    launchMsg = "";
    try {
      // 后端只缓存本次会话内登录成功的 token，未登录则先尝试自动登录
      if (!loggedIn.has(selectedAccount)) {
        const acc = accounts.find((a) => a.snda_id === selectedAccount);
        if (!acc?.can_auto_login) {
          onError("该账号本次会话尚未登录且无法自动登录，请先在「账号登录」页登录");
          goLogin();
          return;
        }
        await api.autoLogin(selectedAccount);
        markLoggedIn(selectedAccount);
      }
      launchMsg = await api.launchGame(selectedAccount, selectedArea);
    } catch (e) {
      onError(e);
    } finally {
      launching = false;
    }
  }

  function fmtBytes(n: number): string {
    if (n >= 1 << 30) return (n / (1 << 30)).toFixed(2) + " GB";
    if (n >= 1 << 20) return (n / (1 << 20)).toFixed(1) + " MB";
    if (n >= 1 << 10) return (n / (1 << 10)).toFixed(1) + " KB";
    return n + " B";
  }
</script>

<div class="page-title">
  <h2>主界面</h2>
  <p>选择账号与大区，检查更新并启动游戏</p>
</div>

{#if accounts.length === 0}
  <div class="card">
    <div class="empty">
      <p>还没有可用账号，请先登录一个盛大账号。</p>
      <button class="primary" onclick={goLogin}>去登录</button>
    </div>
  </div>
{:else}
  <div class="card">
    <h3 class="card-title">账号与大区</h3>
    <div class="row">
      <div class="grow field">
        <label class="field-label" for="sel-account">账号</label>
        <select id="sel-account" bind:value={selectedAccount}>
          {#each accounts as acc (acc.snda_id)}
            <option value={acc.snda_id}>
              {acc.display_name}{acc.is_default ? "（默认）" : ""}
            </option>
          {/each}
        </select>
      </div>
      <div class="grow field">
        <label class="field-label" for="sel-area">大区</label>
        <select id="sel-area" bind:value={selectedArea}>
          {#each areas as area (area.Areaid)}
            <option value={area.Areaid}>{area.AreaName}</option>
          {/each}
        </select>
      </div>
    </div>
  </div>

  <div class="card">
    <h3 class="card-title">游戏状态</h3>
    {#if !rootValid}
      <div class="empty">
        <p>尚未配置有效的游戏目录（需包含 boot / game / sdo）。</p>
        <button onclick={goSettings}>前往设置</button>
      </div>
    {:else if statusError}
      <div class="empty">
        <p>获取游戏状态失败：{statusError}</p>
        <button onclick={goSettings}>前往设置</button>
      </div>
    {:else if status}
      <div class="version-line">
        <span class="muted">当前版本</span>
        <span class="version">{status.ffxiv}</span>
      </div>
      <div class="row update-row">
        <button onclick={doCheck} disabled={checking || updating}>
          {checking ? "检查中…" : "检查更新"}
        </button>
        {#if checkResult}
          {#if checkResult.up_to_date}
            <span class="up-to-date">✓ 已是最新版本</span>
          {:else}
            <span class="muted">
              有 {checkResult.patch_count} 个补丁（共 {fmtBytes(
                checkResult.total_bytes,
              )}）
            </span>
            <button class="primary" onclick={doUpdate} disabled={updating}>
              {updating ? "更新中…" : "更新游戏"}
            </button>
          {/if}
        {/if}
      </div>
      {#if updating}
        <div class="progress-track">
          {#if progress && progress.stage === "download" && progress.total > 0}
            <div class="progress-fill" style="width: {progressPct}%"></div>
          {:else}
            <div class="progress-fill indeterminate"></div>
          {/if}
        </div>
        <p class="muted progress-text">
          {#if progress && progress.stage === "download"}
            正在下载补丁：{fmtBytes(progress.downloaded)} / {fmtBytes(
              progress.total,
            )}（{progressPct.toFixed(1)}%）
          {:else if progress && progress.stage === "install"}
            正在安装补丁…
          {:else if progress && progress.stage === "done"}
            补丁安装完成
          {:else}
            正在准备更新…
          {/if}
        </p>
      {/if}
    {:else}
      <p class="muted">读取中…</p>
    {/if}
  </div>

  <div class="launch-wrap">
    <button
      class="launch-btn"
      onclick={doLaunch}
      disabled={launching || !rootValid || !selectedAccount || !selectedArea}
    >
      {launching ? "启动中…" : "启动游戏"}
    </button>
    {#if launchMsg}<p class="muted launch-msg">{launchMsg}</p>{/if}
  </div>
{/if}

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .field-label {
    font-size: 12px;
    color: var(--text-dim);
  }
  .version-line {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin-bottom: 12px;
  }
  .version {
    font-size: 15px;
    font-weight: 650;
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .update-row {
    flex-wrap: wrap;
  }
  .up-to-date {
    color: var(--ok);
    font-size: 13px;
  }
  .progress-text {
    margin: 0;
  }

  .launch-wrap {
    text-align: center;
    padding: 10px 0 4px;
  }
  .launch-btn {
    height: 54px;
    padding: 0 72px;
    font-size: 19px;
    font-weight: 700;
    letter-spacing: 4px;
    color: #fff;
    border: none;
    border-radius: 12px;
    background: linear-gradient(180deg, #5f9bff 0%, #3a72e8 100%);
    box-shadow:
      0 4px 18px rgba(63, 123, 242, 0.45),
      inset 0 1px 0 rgba(255, 255, 255, 0.25);
    transition:
      box-shadow 0.2s,
      filter 0.2s,
      transform 0.05s;
  }
  .launch-btn:hover:not(:disabled) {
    filter: brightness(1.1);
    box-shadow:
      0 6px 26px rgba(63, 123, 242, 0.6),
      inset 0 1px 0 rgba(255, 255, 255, 0.25);
  }
  .launch-btn:active:not(:disabled) {
    transform: translateY(1px);
  }
  .launch-msg {
    margin: 10px 0 0;
  }
</style>
