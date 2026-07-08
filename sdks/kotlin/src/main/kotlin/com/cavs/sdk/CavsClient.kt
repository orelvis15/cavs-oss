package com.cavs.sdk

import com.cavs.sdk.model.AnalyzeReport
import com.cavs.sdk.model.AnalyzeRequest
import com.cavs.sdk.model.ApplyPlanRequest
import com.cavs.sdk.model.ApplyResult
import com.cavs.sdk.model.BenchmarkReport
import com.cavs.sdk.model.BenchmarkRequest
import com.cavs.sdk.model.CreatePlanRequest
import com.cavs.sdk.model.PackDirectoryRequest
import com.cavs.sdk.model.PackResult
import com.cavs.sdk.model.PlanResult
import com.cavs.sdk.model.PreviewReport
import com.cavs.sdk.model.PreviewRequest
import com.cavs.sdk.model.ProgressEvent
import com.cavs.sdk.model.SavingsReport
import com.cavs.sdk.model.SavingsRequest
import com.cavs.sdk.model.VerifyRequest
import com.cavs.sdk.model.VerifyResult
import com.cavs.sdk.nativebridge.NativeBridge
import kotlinx.serialization.Serializable
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.encodeToJsonElement
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.CompletableFuture
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicLong

private const val SCHEMA_VERSION = "1.0"

private val cavsJson = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
    encodeDefaults = false
}

@Serializable
private data class Envelope(val schemaVersion: String, val data: JsonElement)

/**
 * The CAVS SDK client. It loads the same compiled Rust core the CAVS CLI
 * uses (through a stable C ABI) and exposes typed operations. Instances own
 * a native context and must be [close]d; they are safe to share across
 * threads (calls are serialized so a per-call progress sink is never
 * clobbered by a concurrent call).
 */
class CavsClient private constructor(private val bridge: NativeBridge) : AutoCloseable {

    private val ctx: Long = bridge.contextNew().also {
        check(it != 0L) { "cavs: failed to create native context" }
    }
    private val lock = Any()
    private val executor = Executors.newCachedThreadPool { runnable ->
        Thread(runnable, "cavs-sdk-${threadCounter.incrementAndGet()}").apply { isDaemon = true }
    }

    @Volatile
    private var closed = false

    /** The native SDK version. */
    fun version(): String = bridge.version()

    /** The native C ABI contract version. */
    fun abiVersion(): String = bridge.abiVersion()

    // ---- Synchronous operations ----

    fun analyze(request: AnalyzeRequest, progress: ((ProgressEvent) -> Unit)? = null): AnalyzeReport =
        call("analyze", request, progress)

    fun packDirectory(request: PackDirectoryRequest, progress: ((ProgressEvent) -> Unit)? = null): PackResult =
        call("packDirectory", request, progress)

    fun preview(request: PreviewRequest, progress: ((ProgressEvent) -> Unit)? = null): PreviewReport =
        call("previewUpdate", request, progress)

    fun createPlan(request: CreatePlanRequest, progress: ((ProgressEvent) -> Unit)? = null): PlanResult =
        call("createPlan", request, progress)

    fun applyPlan(request: ApplyPlanRequest, progress: ((ProgressEvent) -> Unit)? = null): ApplyResult =
        call("applyPlan", request, progress)

    fun verifyInstall(request: VerifyRequest): VerifyResult =
        call("verifyInstall", request, null)

    fun benchmark(request: BenchmarkRequest, progress: ((ProgressEvent) -> Unit)? = null): BenchmarkReport =
        call("benchmark", request, progress)

    fun estimateSavings(request: SavingsRequest): SavingsReport =
        call("estimateSavings", request, null)

    // ---- Asynchronous operations ----

    fun previewAsync(request: PreviewRequest): CompletableFuture<PreviewReport> =
        CompletableFuture.supplyAsync({ preview(request) }, executor)

    fun createPlanAsync(request: CreatePlanRequest): CompletableFuture<PlanResult> =
        CompletableFuture.supplyAsync({ createPlan(request) }, executor)

    fun applyPlanAsync(request: ApplyPlanRequest): CompletableFuture<ApplyResult> =
        CompletableFuture.supplyAsync({ applyPlan(request) }, executor)

    override fun close() {
        synchronized(lock) {
            if (closed) return
            closed = true
            executor.shutdown()
            bridge.contextFree(ctx)
        }
    }

    // ---- Core ----

    private inline fun <reified Req, reified Resp> call(
        operation: String,
        request: Req,
        noinline progress: ((ProgressEvent) -> Unit)?,
    ): Resp = synchronized(lock) {
        check(!closed) { "cavs: client is closed" }
        val body = cavsJson.encodeToString(Envelope(SCHEMA_VERSION, cavsJson.encodeToJsonElement(request)))
        if (progress != null) {
            bridge.setProgressCallback(ctx) { raw -> forwardProgress(raw, progress) }
        }
        try {
            val job = bridge.startJob(ctx, operation, body)
            if (job == 0L) {
                throw CavsException(CavsErrorCode.INVALID_REQUEST.wire, "native library rejected the request")
            }
            val envelope = try {
                awaitJob(job)
            } finally {
                bridge.freeJob(job)
            }
            decode<Resp>(envelope)
        } finally {
            if (progress != null) bridge.setProgressCallback(ctx, null)
        }
    }

    private fun awaitJob(job: Long): String {
        while (true) {
            bridge.pollJob(job)?.let { return it }
            Thread.sleep(0, 200_000)
        }
    }

    private fun forwardProgress(rawJson: String, sink: (ProgressEvent) -> Unit) {
        // A malformed progress event must never break the operation.
        runCatching { sink(cavsJson.decodeFromString<ProgressEvent>(rawJson)) }
    }

    private inline fun <reified T> decode(envelope: String): T {
        val root = cavsJson.parseToJsonElement(envelope).jsonObject
        if (root["ok"]?.jsonPrimitive?.booleanOrNull != true) {
            val err = root["error"]?.jsonObject
            throw CavsException(
                err?.get("code")?.jsonPrimitive?.contentOrNull ?: CavsErrorCode.UNKNOWN.wire,
                err?.get("message")?.jsonPrimitive?.contentOrNull ?: "operation failed",
                err?.get("recoverable")?.jsonPrimitive?.booleanOrNull ?: false,
            )
        }
        return cavsJson.decodeFromJsonElement(root["data"] ?: JsonNull)
    }

    companion object {
        private val threadCounter = AtomicLong()

        fun create(): CavsClient = create(CavsOptions.defaults())

        fun create(options: CavsOptions): CavsClient = CavsClient(options.bridge)
    }
}
