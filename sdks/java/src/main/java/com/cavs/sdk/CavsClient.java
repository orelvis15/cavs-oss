package com.cavs.sdk;

import com.cavs.sdk.model.*;
import com.cavs.sdk.nativebridge.NativeBridge;
import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;

import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.locks.ReentrantLock;
import java.util.function.Consumer;

/**
 * The CAVS SDK client. It loads the same compiled Rust core the CAVS CLI
 * uses (through a stable C ABI) and exposes typed operations. Instances own
 * a native context and must be {@link #close() closed}; they are safe to
 * share across threads (calls are serialized so a per-call progress sink is
 * never clobbered by a concurrent call).
 */
public final class CavsClient implements AutoCloseable {

    private static final String SCHEMA_VERSION = "1.0";

    private final NativeBridge bridge;
    private final long ctx;
    private final ObjectMapper mapper;
    private final ExecutorService executor;
    private final ReentrantLock lock = new ReentrantLock();
    private volatile boolean closed;

    private CavsClient(NativeBridge bridge) {
        this.bridge = bridge;
        this.ctx = bridge.contextNew();
        if (ctx == 0) {
            throw new IllegalStateException("cavs: failed to create native context");
        }
        this.mapper = new ObjectMapper()
                .configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false);
        AtomicLong n = new AtomicLong();
        this.executor = Executors.newCachedThreadPool(r -> {
            Thread t = new Thread(r, "cavs-sdk-" + n.incrementAndGet());
            t.setDaemon(true);
            return t;
        });
    }

    public static CavsClient create() {
        return create(CavsOptions.defaults());
    }

    public static CavsClient create(CavsOptions options) {
        return new CavsClient(options.bridge());
    }

    /** The native SDK version. */
    public String version() {
        return bridge.version();
    }

    /** The native C ABI contract version. */
    public String abiVersion() {
        return bridge.abiVersion();
    }

    // ---- Synchronous operations ----

    public AnalyzeReport analyze(AnalyzeRequest request) {
        return analyze(request, null);
    }

    public AnalyzeReport analyze(AnalyzeRequest request, Consumer<ProgressEvent> progress) {
        return call("analyze", request, AnalyzeReport.class, progress);
    }

    public PackResult packDirectory(PackDirectoryRequest request) {
        return packDirectory(request, null);
    }

    public PackResult packDirectory(PackDirectoryRequest request, Consumer<ProgressEvent> progress) {
        return call("packDirectory", request, PackResult.class, progress);
    }

    public PreviewReport preview(PreviewRequest request) {
        return preview(request, null);
    }

    public PreviewReport preview(PreviewRequest request, Consumer<ProgressEvent> progress) {
        return call("previewUpdate", request, PreviewReport.class, progress);
    }

    public PlanResult createPlan(CreatePlanRequest request) {
        return createPlan(request, null);
    }

    public PlanResult createPlan(CreatePlanRequest request, Consumer<ProgressEvent> progress) {
        return call("createPlan", request, PlanResult.class, progress);
    }

    public ApplyResult applyPlan(ApplyPlanRequest request) {
        return applyPlan(request, null);
    }

    public ApplyResult applyPlan(ApplyPlanRequest request, Consumer<ProgressEvent> progress) {
        return call("applyPlan", request, ApplyResult.class, progress);
    }

    public VerifyResult verifyInstall(VerifyRequest request) {
        return call("verifyInstall", request, VerifyResult.class, null);
    }

    public BenchmarkReport benchmark(BenchmarkRequest request) {
        return benchmark(request, null);
    }

    public BenchmarkReport benchmark(BenchmarkRequest request, Consumer<ProgressEvent> progress) {
        return call("benchmark", request, BenchmarkReport.class, progress);
    }

    public SavingsReport estimateSavings(SavingsRequest request) {
        return call("estimateSavings", request, SavingsReport.class, null);
    }

    // ---- Asynchronous operations ----

    public CompletableFuture<PreviewReport> previewAsync(PreviewRequest request) {
        return async(() -> preview(request));
    }

    public CompletableFuture<PlanResult> createPlanAsync(CreatePlanRequest request) {
        return async(() -> createPlan(request));
    }

    public CompletableFuture<ApplyResult> applyPlanAsync(ApplyPlanRequest request) {
        return async(() -> applyPlan(request));
    }

    private <T> CompletableFuture<T> async(java.util.function.Supplier<T> body) {
        return CompletableFuture.supplyAsync(body, executor);
    }

    // ---- Core ----

    private <T> T call(String operation, Object request, Class<T> responseType, Consumer<ProgressEvent> progress) {
        if (closed) {
            throw new IllegalStateException("cavs: client is closed");
        }
        lock.lock();
        try {
            String body = encode(request);
            if (progress != null) {
                bridge.setProgressCallback(ctx, raw -> forwardProgress(raw, progress));
            }
            try {
                long job = bridge.startJob(ctx, operation, body);
                if (job == 0) {
                    throw new CavsException(CavsErrorCode.INVALID_REQUEST.wire(),
                            "native library rejected the request", false, Map.of());
                }
                String envelope;
                try {
                    envelope = await(job);
                } finally {
                    bridge.freeJob(job);
                }
                return decode(envelope, responseType);
            } finally {
                if (progress != null) {
                    bridge.setProgressCallback(ctx, null);
                }
            }
        } finally {
            lock.unlock();
        }
    }

    private String await(long job) {
        while (true) {
            String env = bridge.pollJob(job);
            if (env != null) {
                return env;
            }
            try {
                Thread.sleep(0, 200_000);
            } catch (InterruptedException e) {
                bridge.cancelJob(job);
                Thread.currentThread().interrupt();
                throw new CavsException(CavsErrorCode.CANCELLED.wire(), "operation interrupted", true, Map.of());
            }
        }
    }

    private void forwardProgress(String rawJson, Consumer<ProgressEvent> sink) {
        try {
            sink.accept(mapper.readValue(rawJson, ProgressEvent.class));
        } catch (Exception ignored) {
            // A malformed progress event must never break the operation.
        }
    }

    private String encode(Object request) {
        try {
            JsonNode data = mapper.valueToTree(request);
            return mapper.writeValueAsString(mapper.createObjectNode()
                    .put("schemaVersion", SCHEMA_VERSION)
                    .set("data", data));
        } catch (Exception e) {
            throw new CavsException(CavsErrorCode.INVALID_REQUEST.wire(),
                    "failed to encode request: " + e.getMessage(), false, Map.of());
        }
    }

    @SuppressWarnings("unchecked")
    private <T> T decode(String envelope, Class<T> responseType) {
        try {
            JsonNode root = mapper.readTree(envelope);
            if (!root.path("ok").asBoolean(false)) {
                JsonNode err = root.path("error");
                Map<String, Object> details = err.has("details")
                        ? mapper.convertValue(err.get("details"), Map.class)
                        : Map.of();
                throw new CavsException(
                        err.path("code").asText("CAVS-E-UNKNOWN"),
                        err.path("message").asText("operation failed"),
                        err.path("recoverable").asBoolean(false),
                        details);
            }
            return mapper.treeToValue(root.get("data"), responseType);
        } catch (CavsException e) {
            throw e;
        } catch (Exception e) {
            throw new CavsException(CavsErrorCode.INTERNAL.wire(),
                    "failed to decode response: " + e.getMessage(), false, Map.of());
        }
    }

    @Override
    public void close() {
        lock.lock();
        try {
            if (closed) {
                return;
            }
            closed = true;
            executor.shutdown();
            bridge.contextFree(ctx);
        } finally {
            lock.unlock();
        }
    }
}
