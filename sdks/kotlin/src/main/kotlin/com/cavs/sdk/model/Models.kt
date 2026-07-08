package com.cavs.sdk.model

import kotlinx.serialization.Serializable

/** One progress event emitted during a long-running operation. */
@Serializable
data class ProgressEvent(
    val type: String = "",
    val operation: String = "",
    val phase: String? = null,
    val currentBytes: Long = 0,
    val totalBytes: Long = 0,
    val percentage: Double? = null,
    val message: String? = null,
)

// ---- Analyze ----

@Serializable
data class AnalyzeRequest(
    val oldPath: String,
    val newPath: String,
    val engineHint: String? = null,
    val maxWorstFiles: Int? = null,
)

@Serializable
data class WorstFile(
    val path: String,
    val status: String,
    val isPack: Boolean = false,
    val oldSizeBytes: Long = 0,
    val newSizeBytes: Long = 0,
    val estimatedDownloadBytes: Long = 0,
    val reuseRatio: Double = 0.0,
    val entropyBits: Double = 0.0,
)

@Serializable
data class Recommendation(
    val severity: String,
    val kind: String,
    val title: String,
    val file: String? = null,
    val estimatedWastedBytes: Long = 0,
    val why: String = "",
    val fix: String = "",
    val expectedImprovement: String = "",
)

@Serializable
data class AnalyzeSummary(
    val oldSizeBytes: Long = 0,
    val newSizeBytes: Long = 0,
    val estimatedUpdateBytes: Long = 0,
    val estimatedSteamPipeBytes: Long = 0,
    val cavsReuseRatio: Double = 0.0,
    val steamPipeReuseRatio: Double = 0.0,
    val filesUnchanged: Int = 0,
    val filesModified: Int = 0,
    val filesAdded: Int = 0,
    val filesDeleted: Int = 0,
    val worstFiles: List<WorstFile> = emptyList(),
)

@Serializable
data class AnalyzeReport(
    val summary: AnalyzeSummary,
    val engine: String = "",
    val warnings: List<String> = emptyList(),
    val recommendations: List<Recommendation> = emptyList(),
    val note: String = "",
)

// ---- Pack ----

@Serializable
data class PackDirectoryRequest(
    val inputDir: String,
    val outputCavs: String,
    val profile: String? = null,
    val compression: String? = null,
    val signKeyPath: String? = null,
    val ignore: List<String>? = null,
)

@Serializable
data class PackResult(
    val outputCavs: String,
    val totalSizeBytes: Long = 0,
    val chunkCount: Long = 0,
    val logicalChunks: Long = 0,
    val logicalRawBytes: Long = 0,
    val storedBytes: Long = 0,
    val merkleRoot: String = "",
    val filesPacked: Long = 0,
    val entriesIgnored: Long = 0,
    val signed: Boolean = false,
    val profile: String = "",
    val elapsedMs: Long = 0,
)

// ---- Preview ----

@Serializable
data class PreviewRequest(
    val oldPath: String,
    val newPath: String,
    val engineHint: String? = null,
    val routes: List<String>? = null,
    val policy: String? = null,
)

@Serializable
data class Route(
    val name: String,
    val networkBytes: Long = 0,
    val diffMs: Long? = null,
    val applyMs: Long? = null,
    val available: Boolean = true,
)

@Serializable
data class PreviewReport(
    val recommendedRoute: String = "",
    val oldSizeBytes: Long = 0,
    val newSizeBytes: Long = 0,
    val routes: List<Route> = emptyList(),
    val explanation: String = "",
)

// ---- Plan ----

@Serializable
data class CreatePlanRequest(
    val newPath: String,
    val outputPlan: String,
    val oldPath: String? = null,
    val oldSignature: String? = null,
    val planKind: String? = null,
    val blockKib: Int? = null,
    val zstdLevel: Int? = null,
)

@Serializable
data class PlanResult(
    val planPath: String,
    val planBytes: Long = 0,
    val planKind: String = "",
    val mode: String = "",
    val operationCount: Long = 0,
    val copyOps: Long = 0,
    val inlineOps: Long = 0,
    val reusedBytes: Long = 0,
    val inlineBytes: Long = 0,
    val estimatedNetworkBytes: Long = 0,
    val expectedOutputSize: Long = 0,
    val files: Long = 0,
    val unchangedFiles: Long = 0,
    val deleted: Long = 0,
    val elapsedMs: Long = 0,
)

// ---- Apply ----

@Serializable
data class ApplyPlanRequest(
    val oldPath: String,
    val planPath: String,
    val outputPath: String,
    val checkOld: Boolean? = null,
    val deleteRemoved: Boolean? = null,
)

@Serializable
data class ApplyResult(
    val outputPath: String,
    val verified: Boolean = false,
    val mode: String = "",
    val filesTotal: Long = 0,
    val filesWritten: Long = 0,
    val filesNoop: Long = 0,
    val dirsCreated: Long = 0,
    val symlinksCreated: Long = 0,
    val deleted: Long = 0,
    val bytesWritten: Long = 0,
    val bytesFromOld: Long = 0,
    val bytesFromBlob: Long = 0,
    val elapsedMs: Long = 0,
)

// ---- Verify ----

@Serializable
data class VerifyRequest(
    val target: String,
    val signature: String? = null,
    val manifest: String? = null,
    val allowExtra: Boolean? = null,
)

@Serializable
data class Mismatches(
    val modified: List<String> = emptyList(),
    val missing: List<String> = emptyList(),
    val extra: List<String> = emptyList(),
)

@Serializable
data class VerifyResult(
    val verified: Boolean = false,
    val filesChecked: Long = 0,
    val bytesChecked: Long = 0,
    val mismatches: Mismatches = Mismatches(),
    val elapsedMs: Long = 0,
)

// ---- Benchmark ----

@Serializable
data class BenchmarkRequest(
    val oldPath: String,
    val newPath: String,
    val engineHint: String? = null,
    val measureApply: Boolean? = null,
)

@Serializable
data class BenchmarkReport(
    val oldSizeBytes: Long = 0,
    val newSizeBytes: Long = 0,
    val recommendedRoute: String = "",
    val routes: List<Route> = emptyList(),
    val reuseRatio: Double = 0.0,
)

// ---- Savings ----

@Serializable
data class SavingsRequest(
    val pricePerGb: Double,
    val monthlyDownloads: Double,
    val averageFullDownloadBytes: Double,
    val averageCavsDownloadBytes: Double,
)

@Serializable
data class SavingsReport(
    val fullDownloadMonthlyCost: Double = 0.0,
    val cavsMonthlyCost: Double = 0.0,
    val estimatedMonthlySavings: Double = 0.0,
    val savingsPercent: Double = 0.0,
)
