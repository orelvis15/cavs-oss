package com.cavs.sdk.nativebridge;

import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.util.HexFormat;

/**
 * Locates and loads the native CAVS library. The library is first sought on
 * disk via {@code -Dcavs.sdk.library=/path/to/lib} or the {@code CAVS_SDK_LIBRARY}
 * environment variable (used by tests and custom deployments); otherwise it
 * is extracted from the jar resource {@code /native/<os>-<arch>/<libname>} to
 * a private temp directory and, when a {@code .sha256} sidecar resource is
 * present, verified before loading.
 */
final class NativeLibraryLoader {

    private static boolean loaded;

    private NativeLibraryLoader() {
    }

    static synchronized void ensureLoaded() {
        if (loaded) {
            return;
        }
        String override = System.getProperty("cavs.sdk.library");
        if (override == null || override.isBlank()) {
            override = System.getenv("CAVS_SDK_LIBRARY");
        }
        if (override != null && !override.isBlank()) {
            System.load(Path.of(override).toAbsolutePath().toString());
            loaded = true;
            return;
        }
        loadFromResources();
        loaded = true;
    }

    private static void loadFromResources() {
        String resourceDir = "/native/" + osArch();
        String libName = libraryFileName();
        String resource = resourceDir + "/" + libName;
        try (InputStream in = NativeLibraryLoader.class.getResourceAsStream(resource)) {
            if (in == null) {
                throw new UnsatisfiedLinkError(
                        "cavs: native library not bundled at " + resource
                                + "; set -Dcavs.sdk.library or CAVS_SDK_LIBRARY to a local build");
            }
            Path dir = Files.createTempDirectory("cavs-sdk-native");
            dir.toFile().deleteOnExit();
            Path target = dir.resolve(libName);
            long size;
            try (OutputStream out = Files.newOutputStream(target)) {
                size = in.transferTo(out);
            }
            target.toFile().deleteOnExit();
            verifyChecksum(resource, target);
            if (size == 0) {
                throw new UnsatisfiedLinkError("cavs: extracted native library is empty");
            }
            System.load(target.toAbsolutePath().toString());
        } catch (java.io.IOException e) {
            throw new UnsatisfiedLinkError("cavs: failed to stage native library: " + e.getMessage());
        }
    }

    private static void verifyChecksum(String resource, Path target) throws java.io.IOException {
        try (InputStream sums = NativeLibraryLoader.class.getResourceAsStream(resource + ".sha256")) {
            if (sums == null) {
                return; // optional; release builds ship it
            }
            String expected = new String(sums.readAllBytes()).trim().split("\\s+")[0].toLowerCase();
            String actual = sha256Hex(Files.readAllBytes(target));
            if (!expected.equals(actual)) {
                throw new UnsatisfiedLinkError(
                        "cavs: native library checksum mismatch (expected " + expected + ", got " + actual + ")");
            }
        }
    }

    private static String sha256Hex(byte[] bytes) {
        try {
            return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes));
        } catch (java.security.NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }

    /** e.g. linux-x86_64, macos-aarch64, windows-x86_64. */
    static String osArch() {
        String os = System.getProperty("os.name", "").toLowerCase();
        String arch = System.getProperty("os.arch", "").toLowerCase();
        String osTag;
        if (os.contains("mac") || os.contains("darwin")) {
            osTag = "macos";
        } else if (os.contains("win")) {
            osTag = "windows";
        } else {
            osTag = "linux";
        }
        String archTag = switch (arch) {
            case "amd64", "x86_64" -> "x86_64";
            case "aarch64", "arm64" -> "aarch64";
            default -> arch;
        };
        return osTag + "-" + archTag;
    }

    static String libraryFileName() {
        String os = System.getProperty("os.name", "").toLowerCase();
        if (os.contains("mac") || os.contains("darwin")) {
            return "libcavs_sdk.dylib";
        }
        if (os.contains("win")) {
            return "cavs_sdk.dll";
        }
        return "libcavs_sdk.so";
    }
}
