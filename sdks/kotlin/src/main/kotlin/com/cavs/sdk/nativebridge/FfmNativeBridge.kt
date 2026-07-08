package com.cavs.sdk.nativebridge

import java.lang.foreign.Arena
import java.lang.foreign.FunctionDescriptor
import java.lang.foreign.Linker
import java.lang.foreign.MemorySegment
import java.lang.foreign.SymbolLookup
import java.lang.foreign.ValueLayout
import java.lang.invoke.MethodHandle
import java.lang.invoke.MethodHandles
import java.lang.invoke.MethodType
import java.util.concurrent.ConcurrentHashMap

/**
 * [NativeBridge] backed by the Java Foreign Function & Memory API (JEP 454,
 * finalized in Java 22). It needs no third-party dependency: the whole
 * binding is JDK-native.
 *
 * Downcalls use [MethodHandle.invokeWithArguments] rather than the
 * signature-polymorphic `invoke`/`invokeExact` (which Kotlin cannot emit);
 * for CAVS's coarse, file-system-heavy operations the reflective dispatch
 * overhead is negligible.
 */
class FfmNativeBridge : NativeBridge {

    // Must run before the downcall-handle initializers below resolve symbols.
    init {
        NativeLibraryLoader.ensureLoaded()
    }

    private val version = down("cavs_sdk_version", FunctionDescriptor.of(ADDR))
    private val abiVersion = down("cavs_sdk_abi_version", FunctionDescriptor.of(ADDR))
    private val capabilities = down("cavs_sdk_capabilities_json", FunctionDescriptor.of(ADDR))
    private val ctxNew = down("cavs_context_new", FunctionDescriptor.of(ADDR, ADDR))
    private val ctxFree = down("cavs_context_free", FunctionDescriptor.ofVoid(ADDR))
    private val setProgress =
        down("cavs_context_set_progress_callback", FunctionDescriptor.of(INT, ADDR, ADDR, ADDR))
    private val execute = down("cavs_execute_json", FunctionDescriptor.of(ADDR, ADDR, ADDR, ADDR))
    private val startJson = down("cavs_start_json", FunctionDescriptor.of(ADDR, ADDR, ADDR, ADDR))
    private val jobPoll = down("cavs_job_poll", FunctionDescriptor.of(ADDR, ADDR))
    private val jobCancel = down("cavs_job_cancel", FunctionDescriptor.of(INT, ADDR))
    private val jobFree = down("cavs_job_free", FunctionDescriptor.ofVoid(ADDR))
    private val resultJson = down("cavs_result_json", FunctionDescriptor.of(ADDR, ADDR))
    private val resultFree = down("cavs_result_free", FunctionDescriptor.ofVoid(ADDR))
    private val stringFree = down("cavs_string_free", FunctionDescriptor.ofVoid(ADDR))

    /** Per-context upcall state, kept alive for the context's lifetime. */
    private class CtxState(val arena: Arena, @Suppress("unused") val stub: MemorySegment)

    private val contexts = ConcurrentHashMap<Long, CtxState>()

    override fun version(): String = readString(version.call() as MemorySegment)!!

    override fun abiVersion(): String = readString(abiVersion.call() as MemorySegment)!!

    override fun capabilitiesJson(): String {
        val ptr = capabilities.call() as MemorySegment
        val s = readString(ptr) ?: ""
        stringFree.call(ptr)
        return s
    }

    override fun contextNew(): Long = (ctxNew.call(MemorySegment.NULL) as MemorySegment).address()

    override fun contextFree(ctx: Long) {
        val state = contexts.remove(ctx)
        try {
            ctxFree.call(MemorySegment.ofAddress(ctx))
        } finally {
            state?.arena?.close()
        }
    }

    override fun setProgressCallback(ctx: Long, sink: ((String) -> Unit)?) {
        contexts.remove(ctx)?.arena?.close()
        if (sink == null) {
            setProgress.call(MemorySegment.ofAddress(ctx), MemorySegment.NULL, MemorySegment.NULL)
            return
        }
        val arena = Arena.ofShared()
        val handler = MethodHandles.lookup().bind(
            Trampoline(sink),
            "onEvent",
            MethodType.methodType(Void.TYPE, MemorySegment::class.java, MemorySegment::class.java),
        )
        val stub = LINKER.upcallStub(
            handler,
            FunctionDescriptor.ofVoid(ValueLayout.ADDRESS, ValueLayout.ADDRESS),
            arena,
        )
        contexts[ctx] = CtxState(arena, stub)
        setProgress.call(MemorySegment.ofAddress(ctx), stub, MemorySegment.NULL)
    }

    override fun executeSync(ctx: Long, operation: String, requestJson: String): String =
        Arena.ofConfined().use { arena ->
            val op = arena.allocateFrom(operation)
            val req = arena.allocateFrom(requestJson)
            drainResult(execute.call(MemorySegment.ofAddress(ctx), op, req) as MemorySegment)
        }

    override fun startJob(ctx: Long, operation: String, requestJson: String): Long =
        Arena.ofConfined().use { arena ->
            val op = arena.allocateFrom(operation)
            val req = arena.allocateFrom(requestJson)
            (startJson.call(MemorySegment.ofAddress(ctx), op, req) as MemorySegment).address()
        }

    override fun pollJob(job: Long): String? {
        val result = jobPoll.call(MemorySegment.ofAddress(job)) as MemorySegment
        return if (result.address() == 0L) null else drainResult(result)
    }

    override fun cancelJob(job: Long) {
        jobCancel.call(MemorySegment.ofAddress(job))
    }

    override fun freeJob(job: Long) {
        jobFree.call(MemorySegment.ofAddress(job))
    }

    /** Read the result envelope JSON and free the native result. */
    private fun drainResult(result: MemorySegment): String {
        if (result.address() == 0L) {
            return """{"ok":false,"error":{"code":"CAVS-E-INTERNAL",""" +
                """"message":"native returned null result","recoverable":false}}"""
        }
        val envelope = readString(resultJson.call(result) as MemorySegment) ?: ""
        resultFree.call(result)
        return envelope
    }

    /** Holds a Kotlin sink and adapts the native (char*, void*) callback. */
    private class Trampoline(private val sink: (String) -> Unit) {
        @Suppress("unused") // bound via MethodHandle
        fun onEvent(eventJson: MemorySegment, @Suppress("UNUSED_PARAMETER") userData: MemorySegment) {
            readString(eventJson)?.let(sink)
        }
    }

    private companion object {
        val LINKER: Linker = Linker.nativeLinker()
        val ADDR: ValueLayout = ValueLayout.ADDRESS
        val INT: ValueLayout = ValueLayout.JAVA_INT

        fun down(name: String, fd: FunctionDescriptor): MethodHandle {
            val symbol = SymbolLookup.loaderLookup().find(name)
                .orElseThrow { UnsatisfiedLinkError("cavs: missing native symbol $name") }
            return LINKER.downcallHandle(symbol, fd)
        }

        /** Invoke a downcall handle without signature polymorphism. */
        fun MethodHandle.call(vararg args: Any?): Any? =
            try {
                invokeWithArguments(*args)
            } catch (t: Throwable) {
                throw (t as? RuntimeException ?: RuntimeException(t))
            }

        /** Read a NUL-terminated UTF-8 C string from a returned pointer. */
        fun readString(ptr: MemorySegment?): String? =
            if (ptr == null || ptr.address() == 0L) null
            else ptr.reinterpret(Long.MAX_VALUE).getString(0)
    }
}
