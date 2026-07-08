package com.cavs.sdk.nativebridge

import java.nio.file.Files
import java.nio.file.Path
import java.security.MessageDigest
import java.util.HexFormat

/**
 * Locates and loads the native CAVS library. The library is first sought on
 * disk via `-Dcavs.sdk.library=/path/to/lib` or the `CAVS_SDK_LIBRARY`
 * environment variable (used by tests and custom deployments); otherwise it
 * is extracted from the jar resource `/native/<os>-<arch>/<libname>` to a
 * private temp directory and, when a `.sha256` sidecar resource is present,
 * verified before loading.
 */
internal object NativeLibraryLoader {

    @Volatile
    private var loaded = false

    @Synchronized
    fun ensureLoaded() {
        if (loaded) return

        val override = System.getProperty("cavs.sdk.library")?.takeIf { it.isNotBlank() }
            ?: System.getenv("CAVS_SDK_LIBRARY")?.takeIf { it.isNotBlank() }
        if (override != null) {
            System.load(Path.of(override).toAbsolutePath().toString())
            loaded = true
            return
        }
        loadFromResources()
        loaded = true
    }

    private fun loadFromResources() {
        val libName = libraryFileName()
        val resource = "/native/${osArch()}/$libName"
        val input = javaClass.getResourceAsStream(resource)
            ?: throw UnsatisfiedLinkError(
                "cavs: native library not bundled at $resource; " +
                    "set -Dcavs.sdk.library or CAVS_SDK_LIBRARY to a local build",
            )
        val dir = Files.createTempDirectory("cavs-sdk-native").also { it.toFile().deleteOnExit() }
        val target = dir.resolve(libName).also { it.toFile().deleteOnExit() }
        val size = input.use { Files.newOutputStream(target).use(it::transferTo) }
        verifyChecksum(resource, target)
        if (size == 0L) throw UnsatisfiedLinkError("cavs: extracted native library is empty")
        System.load(target.toAbsolutePath().toString())
    }

    private fun verifyChecksum(resource: String, target: Path) {
        val sums = javaClass.getResourceAsStream("$resource.sha256") ?: return
        val expected = sums.use { it.readBytes() }.decodeToString().trim()
            .split(Regex("\\s+")).first().lowercase()
        val actual = HexFormat.of().formatHex(
            MessageDigest.getInstance("SHA-256").digest(Files.readAllBytes(target)),
        )
        if (expected != actual) {
            throw UnsatisfiedLinkError(
                "cavs: native library checksum mismatch (expected $expected, got $actual)",
            )
        }
    }

    /** e.g. linux-x86_64, macos-aarch64, windows-x86_64. */
    fun osArch(): String {
        val os = System.getProperty("os.name", "").lowercase()
        val arch = System.getProperty("os.arch", "").lowercase()
        val osTag = when {
            os.contains("mac") || os.contains("darwin") -> "macos"
            os.contains("win") -> "windows"
            else -> "linux"
        }
        val archTag = when (arch) {
            "amd64", "x86_64" -> "x86_64"
            "aarch64", "arm64" -> "aarch64"
            else -> arch
        }
        return "$osTag-$archTag"
    }

    fun libraryFileName(): String {
        val os = System.getProperty("os.name", "").lowercase()
        return when {
            os.contains("mac") || os.contains("darwin") -> "libcavs_sdk.dylib"
            os.contains("win") -> "cavs_sdk.dll"
            else -> "libcavs_sdk.so"
        }
    }
}
