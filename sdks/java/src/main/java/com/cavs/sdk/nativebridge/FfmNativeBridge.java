package com.cavs.sdk.nativebridge;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;

/**
 * {@link NativeBridge} backed by the Java Foreign Function &amp; Memory API
 * (JEP 454, finalized in Java 22). It needs no third-party dependency: the
 * whole binding is JDK-native.
 */
public final class FfmNativeBridge implements NativeBridge {

    private static final Linker LINKER = Linker.nativeLinker();

    private final MethodHandle version;
    private final MethodHandle abiVersion;
    private final MethodHandle capabilities;
    private final MethodHandle ctxNew;
    private final MethodHandle ctxFree;
    private final MethodHandle setProgress;
    private final MethodHandle execute;
    private final MethodHandle startJson;
    private final MethodHandle jobPoll;
    private final MethodHandle jobCancel;
    private final MethodHandle jobFree;
    private final MethodHandle resultJson;
    private final MethodHandle resultOk;
    private final MethodHandle resultErrCode;
    private final MethodHandle resultErrMsg;
    private final MethodHandle resultFree;
    private final MethodHandle stringFree;

    /** Per-context upcall state, kept alive for the context's lifetime. */
    private final ConcurrentHashMap<Long, CtxState> contexts = new ConcurrentHashMap<>();

    private record CtxState(Arena arena, MemorySegment stub, Consumer<String> sink) {
    }

    public FfmNativeBridge() {
        NativeLibraryLoader.ensureLoaded();
        SymbolLookup lookup = SymbolLookup.loaderLookup();
        var ADDR = ValueLayout.ADDRESS;
        var INT = ValueLayout.JAVA_INT;

        this.version = down(lookup, "cavs_sdk_version", FunctionDescriptor.of(ADDR));
        this.abiVersion = down(lookup, "cavs_sdk_abi_version", FunctionDescriptor.of(ADDR));
        this.capabilities = down(lookup, "cavs_sdk_capabilities_json", FunctionDescriptor.of(ADDR));
        this.ctxNew = down(lookup, "cavs_context_new", FunctionDescriptor.of(ADDR, ADDR));
        this.ctxFree = down(lookup, "cavs_context_free", FunctionDescriptor.ofVoid(ADDR));
        this.setProgress = down(lookup, "cavs_context_set_progress_callback",
                FunctionDescriptor.of(INT, ADDR, ADDR, ADDR));
        this.execute = down(lookup, "cavs_execute_json", FunctionDescriptor.of(ADDR, ADDR, ADDR, ADDR));
        this.startJson = down(lookup, "cavs_start_json", FunctionDescriptor.of(ADDR, ADDR, ADDR, ADDR));
        this.jobPoll = down(lookup, "cavs_job_poll", FunctionDescriptor.of(ADDR, ADDR));
        this.jobCancel = down(lookup, "cavs_job_cancel", FunctionDescriptor.of(INT, ADDR));
        this.jobFree = down(lookup, "cavs_job_free", FunctionDescriptor.ofVoid(ADDR));
        this.resultJson = down(lookup, "cavs_result_json", FunctionDescriptor.of(ADDR, ADDR));
        this.resultOk = down(lookup, "cavs_result_ok", FunctionDescriptor.of(INT, ADDR));
        this.resultErrCode = down(lookup, "cavs_result_error_code", FunctionDescriptor.of(ADDR, ADDR));
        this.resultErrMsg = down(lookup, "cavs_result_error_message", FunctionDescriptor.of(ADDR, ADDR));
        this.resultFree = down(lookup, "cavs_result_free", FunctionDescriptor.ofVoid(ADDR));
        this.stringFree = down(lookup, "cavs_string_free", FunctionDescriptor.ofVoid(ADDR));
    }

    private static MethodHandle down(SymbolLookup lookup, String name, FunctionDescriptor fd) {
        MemorySegment sym = lookup.find(name)
                .orElseThrow(() -> new UnsatisfiedLinkError("cavs: missing native symbol " + name));
        return LINKER.downcallHandle(sym, fd);
    }

    @Override
    public String version() {
        return staticString(version);
    }

    @Override
    public String abiVersion() {
        return staticString(abiVersion);
    }

    @Override
    public String capabilitiesJson() {
        try {
            MemorySegment ptr = (MemorySegment) capabilities.invoke();
            String s = readString(ptr);
            stringFree.invoke(ptr);
            return s;
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    @Override
    public long contextNew() {
        try {
            MemorySegment ctx = (MemorySegment) ctxNew.invoke(MemorySegment.NULL);
            return ctx.address();
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    @Override
    public void contextFree(long ctx) {
        CtxState st = contexts.remove(ctx);
        try {
            ctxFree.invoke(MemorySegment.ofAddress(ctx));
        } catch (Throwable t) {
            throw sneaky(t);
        } finally {
            if (st != null) {
                st.arena().close();
            }
        }
    }

    @Override
    public void setProgressCallback(long ctx, Consumer<String> sink) {
        try {
            CtxState prev = contexts.remove(ctx);
            if (prev != null) {
                prev.arena().close();
            }
            if (sink == null) {
                setProgress.invoke(MemorySegment.ofAddress(ctx), MemorySegment.NULL, MemorySegment.NULL);
                return;
            }
            Arena arena = Arena.ofShared();
            // void trampoline(const char* eventJson, void* userData)
            MethodHandle handler = MethodHandles.lookup()
                    .bind(new Trampoline(sink), "onEvent",
                            MethodType.methodType(void.class, MemorySegment.class, MemorySegment.class));
            MemorySegment stub = LINKER.upcallStub(
                    handler, FunctionDescriptor.ofVoid(ValueLayout.ADDRESS, ValueLayout.ADDRESS), arena);
            contexts.put(ctx, new CtxState(arena, stub, sink));
            setProgress.invoke(MemorySegment.ofAddress(ctx), stub, MemorySegment.NULL);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    @Override
    public String executeSync(long ctx, String operation, String requestJson) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment op = arena.allocateFrom(operation);
            MemorySegment req = arena.allocateFrom(requestJson);
            MemorySegment result = (MemorySegment) execute.invoke(MemorySegment.ofAddress(ctx), op, req);
            return drainResult(result);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    @Override
    public long startJob(long ctx, String operation, String requestJson) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment op = arena.allocateFrom(operation);
            MemorySegment req = arena.allocateFrom(requestJson);
            MemorySegment job = (MemorySegment) startJson.invoke(MemorySegment.ofAddress(ctx), op, req);
            return job.address();
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    @Override
    public String pollJob(long job) {
        try {
            MemorySegment result = (MemorySegment) jobPoll.invoke(MemorySegment.ofAddress(job));
            if (result.address() == 0) {
                return null;
            }
            return drainResult(result);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    @Override
    public void cancelJob(long job) {
        try {
            jobCancel.invoke(MemorySegment.ofAddress(job));
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    @Override
    public void freeJob(long job) {
        try {
            jobFree.invoke(MemorySegment.ofAddress(job));
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    /** Read the result envelope JSON and free the native result. */
    private String drainResult(MemorySegment result) throws Throwable {
        if (result.address() == 0) {
            return "{\"ok\":false,\"error\":{\"code\":\"CAVS-E-INTERNAL\","
                    + "\"message\":\"native returned null result\",\"recoverable\":false}}";
        }
        MemorySegment json = (MemorySegment) resultJson.invoke(result);
        String envelope = readString(json);
        resultFree.invoke(result);
        return envelope;
    }

    private String staticString(MethodHandle h) {
        try {
            return readString((MemorySegment) h.invoke());
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    /** Read a NUL-terminated UTF-8 C string from a returned pointer. */
    private static String readString(MemorySegment ptr) {
        if (ptr == null || ptr.address() == 0) {
            return null;
        }
        return ptr.reinterpret(Long.MAX_VALUE).getString(0);
    }

    private static RuntimeException sneaky(Throwable t) {
        if (t instanceof RuntimeException re) {
            return re;
        }
        return new RuntimeException(t);
    }

    /** Holds a Java sink and adapts the native (char*, void*) callback to it. */
    private static final class Trampoline {
        private final Consumer<String> sink;

        Trampoline(Consumer<String> sink) {
            this.sink = sink;
        }

        @SuppressWarnings("unused") // bound via MethodHandle
        void onEvent(MemorySegment eventJson, MemorySegment userData) {
            String s = readString(eventJson);
            if (s != null) {
                sink.accept(s);
            }
        }
    }
}
