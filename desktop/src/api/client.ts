// Thin, typed wrappers around Tauri `invoke` + dialog/opener plugins.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type {
  AppInfo,
  NewProject,
  OperationRecord,
  ProgressPayload,
  Project,
  RequestLog,
  RunRequest,
  ServerStatus,
  Settings,
  ToolStatus,
} from "./types";

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),

  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) =>
    invoke<Settings>("save_settings", { settings }),

  listProjects: () => invoke<Project[]>("list_projects"),
  createProject: (project: NewProject) => invoke<Project>("create_project", { project }),
  updateProject: (project: Project) => invoke<Project>("update_project", { project }),
  deleteProject: (id: string) => invoke<void>("delete_project", { id }),

  listOperations: (projectId: string, section: string) =>
    invoke<OperationRecord[]>("list_operations", { projectId, section }),
  listProjectOperations: (projectId: string) =>
    invoke<OperationRecord[]>("list_project_operations", { projectId }),
  getOperation: (id: string) =>
    invoke<OperationRecord | null>("get_operation", { id }),
  deleteOperation: (id: string) => invoke<void>("delete_operation", { id }),
  runOperation: (request: RunRequest) =>
    invoke<OperationRecord>("run_operation", { request }),

  openPath: (path: string) => invoke<void>("open_path", { path }),
  detectTools: () => invoke<ToolStatus[]>("detect_tools"),

  serverStart: (dir: string, port: number) =>
    invoke<ServerStatus>("server_start", { dir, port }),
  serverStop: () => invoke<ServerStatus>("server_stop"),
  serverStatus: () => invoke<ServerStatus>("server_status"),
  serverLogs: () => invoke<RequestLog[]>("server_logs"),
};

export function onProgress(
  handler: (p: ProgressPayload) => void
): Promise<UnlistenFn> {
  return listen<ProgressPayload>("cavs://progress", (e) => handler(e.payload));
}

/** Native file/folder picker (Tauri dialog plugin). */
export async function pickPath(opts: {
  directory?: boolean;
  multiple?: boolean;
  title?: string;
}): Promise<string | null> {
  const selected = await openDialog({
    directory: opts.directory ?? false,
    multiple: opts.multiple ?? false,
    title: opts.title,
  });
  if (Array.isArray(selected)) return selected[0] ?? null;
  return selected ?? null;
}

/** Best-effort extraction of a DesktopError-shaped rejection. */
export function errMessage(e: unknown): string {
  if (e && typeof e === "object") {
    const anyE = e as any;
    if (anyE.description) return anyE.description as string;
    if (anyE.message) return anyE.message as string;
  }
  return String(e);
}
