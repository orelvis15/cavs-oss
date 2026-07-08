// Request and response shapes for the CAVS SDK. Field names match the
// engine's camelCase JSON exactly.

export interface ProgressEvent {
  type: string;
  operation: string;
  phase?: string;
  currentBytes?: number;
  totalBytes?: number;
  percentage?: number;
  message?: string;
}

export type RoutePolicy =
  | "balanced"
  | "networkMin"
  | "cpuMin"
  | "ramMin"
  | "diskIoMin"
  | "hddFriendly"
  | "developerFast";

export interface CallOptions {
  /** Stream progress events. Note: enabling progress runs the operation
   *  synchronously (see README); omit it for the non-blocking path. */
  onProgress?: (event: ProgressEvent) => void;
  /** Cancel the operation (non-progress path only). */
  signal?: AbortSignal;
}

export interface AnalyzeRequest {
  oldPath: string;
  newPath: string;
  engineHint?: string;
  maxWorstFiles?: number;
}

export interface WorstFile {
  path: string;
  status: string;
  isPack: boolean;
  oldSizeBytes: number;
  newSizeBytes: number;
  estimatedDownloadBytes: number;
  reuseRatio: number;
  entropyBits: number;
}

export interface Recommendation {
  severity: string;
  kind: string;
  title: string;
  file?: string;
  estimatedWastedBytes: number;
  why: string;
  fix: string;
  expectedImprovement: string;
}

export interface AnalyzeReport {
  summary: {
    oldSizeBytes: number;
    newSizeBytes: number;
    estimatedUpdateBytes: number;
    estimatedSteamPipeBytes: number;
    cavsReuseRatio: number;
    steamPipeReuseRatio: number;
    filesUnchanged: number;
    filesModified: number;
    filesAdded: number;
    filesDeleted: number;
    worstFiles: WorstFile[];
  };
  engine: string;
  warnings: string[];
  recommendations: Recommendation[];
  note: string;
}

export interface PackDirectoryRequest {
  inputDir: string;
  outputCavs: string;
  profile?: string;
  compression?: string;
  signKeyPath?: string;
  ignore?: string[];
}

export interface PackResult {
  outputCavs: string;
  totalSizeBytes: number;
  chunkCount: number;
  logicalChunks: number;
  logicalRawBytes: number;
  storedBytes: number;
  merkleRoot: string;
  filesPacked: number;
  entriesIgnored: number;
  signed: boolean;
  profile: string;
  elapsedMs: number;
}

export interface PreviewRequest {
  oldPath: string;
  newPath: string;
  engineHint?: string;
  routes?: string[];
  policy?: RoutePolicy;
}

export interface Route {
  name: string;
  networkBytes: number;
  diffMs?: number;
  applyMs?: number | null;
  available: boolean;
}

export interface PreviewReport {
  recommendedRoute: string;
  oldSizeBytes: number;
  newSizeBytes: number;
  routes: Route[];
  explanation: string;
}

export interface CreatePlanRequest {
  oldPath?: string;
  oldSignature?: string;
  newPath: string;
  outputPlan: string;
  planKind?: "portable" | "analysis";
  blockKib?: number;
  zstdLevel?: number;
}

export interface PlanResult {
  planPath: string;
  planBytes: number;
  planKind: string;
  mode: string;
  operationCount: number;
  copyOps: number;
  inlineOps: number;
  reusedBytes: number;
  inlineBytes: number;
  estimatedNetworkBytes: number;
  expectedOutputSize: number;
  files: number;
  unchangedFiles: number;
  deleted: number;
  elapsedMs: number;
}

export interface ApplyPlanRequest {
  oldPath: string;
  planPath: string;
  outputPath: string;
  checkOld?: boolean;
  deleteRemoved?: boolean;
}

export interface ApplyResult {
  outputPath: string;
  verified: boolean;
  mode: string;
  filesTotal: number;
  filesWritten: number;
  filesNoop: number;
  dirsCreated: number;
  symlinksCreated: number;
  deleted: number;
  bytesWritten: number;
  bytesFromOld: number;
  bytesFromBlob: number;
  elapsedMs: number;
}

export interface VerifyRequest {
  target: string;
  signature?: string;
  manifest?: string;
  allowExtra?: boolean;
}

export interface VerifyResult {
  verified: boolean;
  filesChecked: number;
  bytesChecked: number;
  mismatches: {
    modified: string[];
    missing: string[];
    extra: string[];
  };
  elapsedMs: number;
}

export interface BenchmarkRequest {
  oldPath: string;
  newPath: string;
  engineHint?: string;
  measureApply?: boolean;
}

export interface BenchmarkReport {
  oldSizeBytes: number;
  newSizeBytes: number;
  recommendedRoute: string;
  routes: Route[];
  reuseRatio: number;
}

export interface SavingsRequest {
  pricePerGb: number;
  monthlyDownloads: number;
  averageFullDownloadBytes: number;
  averageCavsDownloadBytes: number;
}

export interface SavingsReport {
  fullDownloadMonthlyCost: number;
  cavsMonthlyCost: number;
  estimatedMonthlySavings: number;
  savingsPercent: number;
}
