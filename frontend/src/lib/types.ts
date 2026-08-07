/** 与后端 IPC 命令对应的 DTO 定义（字段名与 Rust serde 序列化结果一致）。 */

export type StartupType = "auto" | "managed" | "custom" | "system";

export interface DxvkSettings {
  enabled: boolean;
  hud: string | null;
  frame_limit: number | null;
}

export interface Settings {
  game_path: string | null;
  startup_type: StartupType;
  custom_path: string | null;
  prefix: string | null;
  esync: boolean;
  fsync: boolean;
  msync: boolean;
  debug_vars: string | null;
  env: Record<string, string>;
  dxvk: DxvkSettings;
  gamemode: boolean;
}

export interface AccountInfo {
  snda_id: string;
  display_name: string;
  can_auto_login: boolean;
  is_default: boolean;
}

/** 大区信息（字段名为盛趣接口原始命名）。 */
export interface SdoArea {
  Areaid: string;
  AreaName: string;
  AreaOrder: number;
  AreaStat: number;
  AreaLobby: string;
  AreaGm: string;
  AreaPatch: string;
  AreaConfigUpload: string;
  Areatype: number;
}

export interface GameStatus {
  boot: string;
  ffxiv: string;
  ex1: string;
  ex2: string;
  ex3: string;
  ex4: string;
  ex5: string;
}

export interface CheckResult {
  up_to_date: boolean;
  patch_count: number;
  total_bytes: number;
}

/** `patch-progress` 事件负载。 */
export interface PatchProgress {
  stage: "download" | "install" | "done" | string;
  downloaded: number;
  total: number;
}
