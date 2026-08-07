<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../lib/api";
  import type { Settings, StartupType } from "../lib/types";

  let {
    onError,
    onNotice,
  }: {
    onError: (e: unknown) => void;
    onNotice: (msg: string) => void;
  } = $props();

  /** 加载时的原始设置，保存时在其基础上展开修改，未暴露的字段原样带回。 */
  let original = $state<Settings | null>(null);
  let gamePath = $state("");
  let startupType = $state<StartupType>("auto");
  let customPath = $state("");
  let prefix = $state("");
  let esync = $state(false);
  let fsync = $state(false);
  let gamemode = $state(false);
  let dxvkEnabled = $state(true);
  let saving = $state(false);

  const startupTypes: { value: StartupType; label: string }[] = [
    { value: "auto", label: "自动（auto）" },
    { value: "managed", label: "内置管理（managed）" },
    { value: "custom", label: "自定义（custom）" },
    { value: "system", label: "系统 Wine（system）" },
  ];

  onMount(async () => {
    try {
      const s = await api.getSettings();
      original = s;
      gamePath = s.game_path ?? "";
      startupType = s.startup_type;
      customPath = s.custom_path ?? "";
      prefix = s.prefix ?? "";
      esync = s.esync;
      fsync = s.fsync;
      gamemode = s.gamemode;
      dxvkEnabled = s.dxvk.enabled;
    } catch (e) {
      onError(e);
    }
  });

  async function save() {
    if (!original || saving) return;
    saving = true;
    try {
      const s: Settings = {
        ...original,
        // 空字符串表示未设置
        game_path: gamePath.trim(),
        startup_type: startupType,
        custom_path:
          startupType === "custom" && customPath.trim()
            ? customPath.trim()
            : null,
        prefix: prefix.trim() ? prefix.trim() : null,
        esync,
        fsync,
        gamemode,
        dxvk: { ...original.dxvk, enabled: dxvkEnabled },
      };
      await api.saveSettings(s);
      original = s;
      onNotice("设置已保存");
    } catch (e) {
      onError(e);
    } finally {
      saving = false;
    }
  }
</script>

<div class="page-title">
  <h2>设置</h2>
  <p>游戏目录与 Wine 运行环境配置</p>
</div>

{#if !original}
  <p class="muted">加载中…</p>
{:else}
  <div class="card">
    <h3 class="card-title">游戏目录</h3>
    <input
      type="text"
      placeholder="游戏根目录（含 boot/game/sdo 的目录）"
      bind:value={gamePath}
    />
    <p class="hint muted">
      指向最终幻想14的安装根目录，目录下应包含 boot、game、sdo 子目录。
    </p>
  </div>

  <div class="card">
    <h3 class="card-title">Wine 设置</h3>
    <div class="form-row">
      <span class="label">启动方式</span>
      <select class="grow" bind:value={startupType}>
        {#each startupTypes as t (t.value)}
          <option value={t.value}>{t.label}</option>
        {/each}
      </select>
    </div>
    {#if startupType === "custom"}
      <div class="form-row">
        <span class="label">自定义 Wine 路径</span>
        <input
          class="grow"
          type="text"
          placeholder="wine64 可执行文件或所在 bin 目录"
          bind:value={customPath}
        />
      </div>
    {/if}
    <div class="form-row">
      <span class="label">Wine Prefix</span>
      <input
        class="grow"
        type="text"
        placeholder="留空使用默认 ~/.xiv-launcher-rs/prefix"
        bind:value={prefix}
      />
    </div>
    <div class="form-row checks">
      <label class="checkbox">
        <input type="checkbox" bind:checked={esync} /> esync
      </label>
      <label class="checkbox">
        <input type="checkbox" bind:checked={fsync} /> fsync
      </label>
      <label class="checkbox">
        <input type="checkbox" bind:checked={dxvkEnabled} /> 启用 DXVK
      </label>
      <label class="checkbox">
        <input type="checkbox" bind:checked={gamemode} /> gamemode
      </label>
    </div>
  </div>

  <div class="save-row">
    <button class="primary" onclick={save} disabled={saving}>
      {saving ? "保存中…" : "保存设置"}
    </button>
  </div>
{/if}

<style>
  .hint {
    margin: 8px 0 0;
  }
  .form-row {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 12px;
  }
  .form-row:last-child {
    margin-bottom: 0;
  }
  .label {
    width: 120px;
    flex-shrink: 0;
    font-size: 13px;
    color: var(--text-dim);
  }
  .checks {
    gap: 22px;
    padding-top: 2px;
  }
  .save-row {
    display: flex;
    justify-content: flex-end;
  }
</style>
