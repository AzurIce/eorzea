<script lang="ts">
  import * as api from "../lib/api";
  import type { AccountInfo } from "../lib/types";

  let {
    accounts,
    onLoggedIn,
    onAccountsChanged,
    onError,
  }: {
    accounts: AccountInfo[];
    onLoggedIn: (acc: AccountInfo) => void;
    onAccountsChanged: () => void;
    onError: (e: unknown) => void;
  } = $props();

  type Tab = "qr" | "push" | "password";
  let tab = $state<Tab>("qr");

  // ── 扫码登录 ──
  // qrGen 用于实现"取消"：发起新一轮/取消时递增，旧的等待 Promise 结果直接丢弃。
  let qrImg = $state<string | null>(null);
  let qrStatus = $state("");
  let qrGen = 0;

  async function startQr() {
    const gen = ++qrGen;
    qrImg = null;
    qrStatus = "正在获取二维码…";
    try {
      const b64 = await api.qrLoginStart();
      if (gen !== qrGen) return;
      qrImg = b64;
      qrStatus = "请用叨鱼 App 扫码登录";
      const acc = await api.qrLoginWait();
      if (gen !== qrGen) return;
      cancelQr();
      onLoggedIn(acc);
    } catch (e) {
      if (gen !== qrGen) return;
      cancelQr();
      onError(e);
    }
  }

  function cancelQr() {
    qrGen++;
    qrImg = null;
    qrStatus = "";
  }

  // ── 推送登录 ──
  let pushAccount = $state("");
  let pushSeq = $state<string | null>(null);
  let pushBusy = $state(false);
  let pushGen = 0;

  async function startPush() {
    if (!pushAccount.trim()) {
      onError("请输入账号");
      return;
    }
    const gen = ++pushGen;
    pushBusy = true;
    pushSeq = null;
    try {
      const seq = await api.pushLoginStart(pushAccount.trim());
      if (gen !== pushGen) return;
      pushSeq = seq;
      const acc = await api.pushLoginWait();
      if (gen !== pushGen) return;
      cancelPush();
      onLoggedIn(acc);
    } catch (e) {
      if (gen !== pushGen) return;
      cancelPush();
      onError(e);
    }
  }

  function cancelPush() {
    pushGen++;
    pushBusy = false;
    pushSeq = null;
  }

  // ── 密码登录 ──
  let pwdAccount = $state("");
  let pwdPassword = $state("");
  let pwdBusy = $state(false);

  async function submitPassword(e: Event) {
    e.preventDefault();
    if (!pwdAccount.trim() || !pwdPassword) {
      onError("请输入账号和密码");
      return;
    }
    pwdBusy = true;
    try {
      const acc = await api.passwordLogin(pwdAccount.trim(), pwdPassword);
      pwdPassword = "";
      onLoggedIn(acc);
    } catch (err) {
      onError(err);
    } finally {
      pwdBusy = false;
    }
  }

  // ── 账号列表操作 ──
  let rowBusy = $state("");
  let confirmRemoveId = $state("");

  async function doAutoLogin(acc: AccountInfo) {
    rowBusy = acc.snda_id;
    try {
      onLoggedIn(await api.autoLogin(acc.snda_id));
    } catch (e) {
      onError(e);
    } finally {
      rowBusy = "";
    }
  }

  async function doSetDefault(acc: AccountInfo) {
    rowBusy = acc.snda_id;
    try {
      await api.setDefaultAccount(acc.snda_id);
      onAccountsChanged();
    } catch (e) {
      onError(e);
    } finally {
      rowBusy = "";
    }
  }

  async function doRemove(acc: AccountInfo) {
    if (confirmRemoveId !== acc.snda_id) {
      confirmRemoveId = acc.snda_id;
      return;
    }
    confirmRemoveId = "";
    rowBusy = acc.snda_id;
    try {
      await api.removeAccount(acc.snda_id);
      onAccountsChanged();
    } catch (e) {
      onError(e);
    } finally {
      rowBusy = "";
    }
  }
</script>

<div class="page-title">
  <h2>账号登录</h2>
  <p>登录盛大账号以启动游戏，支持扫码、推送与密码三种方式</p>
</div>

<div class="card">
  <h3 class="card-title">已保存的账号</h3>
  {#if accounts.length === 0}
    <div class="empty">
      <p>还没有保存的账号，通过下方任意方式登录后会自动保存。</p>
    </div>
  {:else}
    <div class="account-list">
      {#each accounts as acc (acc.snda_id)}
        <div class="account-row">
          <div class="avatar">{acc.display_name.slice(0, 1)}</div>
          <div class="grow acc-info">
            <div class="acc-name">{acc.display_name}</div>
            <div class="acc-tags">
              {#if acc.is_default}<span class="tag">默认</span>{/if}
              {#if acc.can_auto_login}<span class="tag ok">可自动登录</span>{/if}
            </div>
          </div>
          {#if acc.can_auto_login}
            <button
              class="small primary"
              disabled={rowBusy === acc.snda_id}
              onclick={() => doAutoLogin(acc)}>自动登录</button
            >
          {/if}
          {#if !acc.is_default}
            <button
              class="small"
              disabled={rowBusy === acc.snda_id}
              onclick={() => doSetDefault(acc)}>设为默认</button
            >
          {/if}
          <button
            class="small danger"
            disabled={rowBusy === acc.snda_id}
            onclick={() => doRemove(acc)}
          >
            {confirmRemoveId === acc.snda_id ? "确认删除？" : "删除"}
          </button>
        </div>
      {/each}
    </div>
  {/if}
</div>

<div class="card">
  <div class="tabs">
    <button class:active={tab === "qr"} onclick={() => (tab = "qr")}>
      扫码登录
    </button>
    <button class:active={tab === "push"} onclick={() => (tab = "push")}>
      推送登录
    </button>
    <button
      class:active={tab === "password"}
      onclick={() => (tab = "password")}
    >
      密码登录
    </button>
  </div>

  {#if tab === "qr"}
    <div class="qr-area">
      {#if qrImg}
        <div class="qr-frame">
          <img src={"data:image/png;base64," + qrImg} alt="登录二维码" />
        </div>
      {/if}
      {#if qrStatus}<p class="muted qr-status">{qrStatus}</p>{/if}
      <div class="row">
        <button class="primary" onclick={startQr}>
          {qrImg ? "刷新二维码" : "获取二维码"}
        </button>
        {#if qrImg || qrStatus}
          <button onclick={cancelQr}>取消</button>
        {/if}
      </div>
    </div>
  {:else if tab === "push"}
    <form
      class="push-form"
      onsubmit={(e) => {
        e.preventDefault();
        startPush();
      }}
    >
      <div class="row">
        <input
          class="grow"
          type="text"
          placeholder="盛趣账号"
          bind:value={pushAccount}
          disabled={pushBusy}
        />
        <button class="primary" type="submit" disabled={pushBusy}>
          发起推送
        </button>
      </div>
    </form>
    {#if pushBusy}
      <div class="push-wait">
        {#if pushSeq}
          <p class="muted">验证序号</p>
          <div class="seq">{pushSeq}</div>
          <p class="muted">请在叨鱼 App 上核对序号并确认登录</p>
        {:else}
          <p class="muted">请在叨鱼 App 上确认登录…</p>
        {/if}
        <button onclick={cancelPush}>取消</button>
      </div>
    {/if}
  {:else}
    <form class="pwd-form" onsubmit={submitPassword}>
      <input
        type="text"
        placeholder="盛趣账号"
        bind:value={pwdAccount}
      />
      <div class="row">
        <input
          class="grow"
          type="password"
          placeholder="密码"
          bind:value={pwdPassword}
        />
        <button class="primary" type="submit" disabled={pwdBusy}>
          {pwdBusy ? "登录中…" : "登录"}
        </button>
      </div>
    </form>
  {/if}
</div>

<style>
  .account-list {
    display: flex;
    flex-direction: column;
  }
  .account-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 9px 8px;
    border-radius: var(--radius-sm);
    transition: background 0.12s;
  }
  .account-row + .account-row {
    border-top: 1px solid var(--border);
  }
  .account-row:hover {
    background: rgba(255, 255, 255, 0.03);
  }
  .avatar {
    width: 32px;
    height: 32px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 14px;
    font-weight: 600;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .acc-info {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .acc-name {
    font-size: 13px;
    font-weight: 600;
  }
  .acc-tags {
    display: flex;
    gap: 6px;
  }
  .tag {
    font-size: 11px;
    color: var(--text-dim);
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    padding: 1px 6px;
  }
  .tag.ok {
    color: var(--ok);
    border-color: rgba(70, 192, 122, 0.4);
    background: var(--ok-soft);
  }

  .tabs {
    display: inline-flex;
    gap: 2px;
    padding: 3px;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    margin-bottom: 16px;
  }
  .tabs button {
    border: none;
    border-radius: 6px;
    height: 28px;
    padding: 0 16px;
    font-size: 12.5px;
    color: var(--text-dim);
    background: transparent;
  }
  .tabs button:hover {
    color: var(--text);
    background: rgba(255, 255, 255, 0.05);
  }
  .tabs button.active {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }

  .qr-area {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 8px 0 4px;
  }
  .qr-frame {
    background: #fff;
    padding: 12px;
    border-radius: 12px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.45);
    line-height: 0;
  }
  .qr-frame img {
    width: 180px;
    height: 180px;
    border-radius: 4px;
  }
  .qr-status {
    margin: 0;
  }

  .push-form {
    max-width: 460px;
  }
  .push-wait {
    margin-top: 16px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }
  .push-wait p {
    margin: 0;
  }
  .seq {
    font-size: 26px;
    font-weight: 700;
    letter-spacing: 4px;
    color: var(--accent);
    padding: 6px 18px;
    background: var(--accent-soft);
    border: 1px solid rgba(79, 140, 255, 0.3);
    border-radius: var(--radius-sm);
  }

  .pwd-form {
    max-width: 460px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
</style>
