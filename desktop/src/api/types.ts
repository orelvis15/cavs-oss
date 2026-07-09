// Shared types mirroring the Rust command surface (src-tauri/src/commands.rs).

export type Lang = "es" | "en";
export type Theme = "dark" | "light";

export interface Settings {
  language: Lang;
  theme: Theme;
  defaultOutputFolder: string | null;
  localServerPort: number;
  showCliPreview: boolean;
  recentProjectsLimit: number;
}

export interface Project {
  id: string;
  name: string;
  icon: string | null;
  engine: string;
  outputFolder: string;
  createdAt: string;
  updatedAt: string;
}

export interface NewProject {
  name: string;
  icon: string | null;
  engine: string;
  outputFolder: string;
}

export interface AppInfo {
  appVersion: string;
  sdkVersion: string;
  abiVersion: string;
  os: string;
  arch: string;
  operations: string[];
}

export interface OperationRecord {
  id: string;
  section: string;
  kind: string;
  title: string;
  status: "completed" | "failed";
  createdAt: string;
  params: any;
  result: any;
  artifactDir: string;
  error: OperationError | null;
  files: string[];
}

export interface OperationError {
  code: string;
  message: string;
  recoverable: boolean;
}

export interface DesktopError {
  code: string;
  title: string;
  description: string;
  suggestedActions: string[];
  technical: string | null;
  recoverable: boolean;
}

export interface RunRequest {
  projectId: string;
  section: string;
  kind: string;
  title: string;
  params: Record<string, any>;
}

export interface ToolStatus {
  name: string;
  available: boolean;
  path: string | null;
  version: string | null;
}

export interface ServerStatus {
  running: boolean;
  port: number | null;
  dir: string | null;
  url: string | null;
  startedAt: string | null;
  requests: number;
  bytesServed: number;
  lastError: string | null;
}

export interface RequestLog {
  time: string;
  method: string;
  path: string;
  status: number;
  bytes: number;
  durationMs: number;
}

export interface ProgressPayload {
  opId: string;
  event: {
    type: string;
    operation: string;
    phase?: string;
    currentBytes?: number;
    totalBytes?: number;
    percentage?: number;
    message?: string;
  };
}
