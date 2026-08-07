import { invoke } from "@tauri-apps/api/core";
import type {
  AccountInfo,
  CheckResult,
  GameStatus,
  SdoArea,
  Settings,
} from "./types";

/** 后端 reject 出来的一般是中文字符串，这里统一转成可展示的文本。 */
export function errMsg(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

// ── 设置 ──
export const getSettings = () => invoke<Settings>("get_settings");
export const saveSettings = (settings: Settings) =>
  invoke<void>("save_settings", { settings });

// ── 账号 ──
export const listAccounts = () => invoke<AccountInfo[]>("list_accounts");
export const setDefaultAccount = (sndaId: string) =>
  invoke<void>("set_default_account", { sndaId });
export const removeAccount = (sndaId: string) =>
  invoke<void>("remove_account", { sndaId });

// ── 登录 ──
export const qrLoginStart = () => invoke<string>("qr_login_start");
export const qrLoginWait = () => invoke<AccountInfo>("qr_login_wait");
export const pushLoginStart = (account: string) =>
  invoke<string | null>("push_login_start", { account });
export const pushLoginWait = () => invoke<AccountInfo>("push_login_wait");
export const passwordLogin = (account: string, password: string) =>
  invoke<AccountInfo>("password_login", { account, password });
export const autoLogin = (sndaId: string) =>
  invoke<AccountInfo>("auto_login", { sndaId });

// ── 大区与游戏 ──
export const listAreas = () => invoke<SdoArea[]>("list_areas");
export const gameStatus = () => invoke<GameStatus>("game_status");
export const checkGame = (areaId: string) =>
  invoke<CheckResult>("check_game", { areaId });
export const updateGame = (areaId: string) =>
  invoke<string>("update_game", { areaId });
export const launchGame = (sndaId: string, areaId: string) =>
  invoke<string>("launch_game", { sndaId, areaId });
export const gameRootValid = () => invoke<boolean>("game_root_valid");
