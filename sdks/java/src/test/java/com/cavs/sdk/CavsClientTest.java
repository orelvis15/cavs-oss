package com.cavs.sdk;

import com.cavs.sdk.model.*;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.ExecutionException;

import static org.junit.jupiter.api.Assertions.*;

class CavsClientTest {

    private CavsClient client;

    @BeforeEach
    void setUp() {
        client = CavsClient.create();
    }

    @AfterEach
    void tearDown() {
        if (client != null) {
            client.close();
        }
    }

    @Test
    void versionIsSemver() {
        assertEquals(3, client.version().split("\\.").length);
        assertEquals("1.0.0", client.abiVersion());
    }

    @Test
    void estimateSavings() {
        SavingsReport r = client.estimateSavings(SavingsRequest.builder()
                .pricePerGb(0.08)
                .monthlyDownloads(500_000)
                .averageFullDownloadBytes(65_011_712)
                .averageCavsDownloadBytes(2_631_921)
                .build());
        assertTrue(r.savingsPercent() > 90.0, "savingsPercent=" + r.savingsPercent());
        assertTrue(r.estimatedMonthlySavings() > r.cavsMonthlyCost());
    }

    @Test
    void fullPipeline(@TempDir Path tmp) throws IOException {
        Builds b = makeBuilds(tmp);
        Path work = Files.createDirectory(tmp.resolve("work"));

        AnalyzeReport an = client.analyze(AnalyzeRequest.builder()
                .oldPath(b.oldDir.toString()).newPath(b.newDir.toString()).build());
        assertTrue(an.summary().newSizeBytes() > 0);

        Path packOut = work.resolve("v2.cavs");
        PackResult pk = client.packDirectory(PackDirectoryRequest.builder()
                .inputDir(b.newDir.toString()).outputCavs(packOut.toString()).build());
        assertTrue(pk.filesPacked() >= 3);
        assertTrue(Files.isRegularFile(packOut));

        Path planPath = work.resolve("update.cavsplan");
        PlanResult pl = client.createPlan(CreatePlanRequest.builder()
                .oldPath(b.oldDir.toString()).newPath(b.newDir.toString())
                .outputPlan(planPath.toString()).build());
        assertTrue(pl.reusedBytes() > 0, "plan found no reuse");

        Path outDir = work.resolve("out");
        ApplyResult ap = client.applyPlan(ApplyPlanRequest.builder()
                .oldPath(b.oldDir.toString()).planPath(planPath.toString())
                .outputPath(outDir.toString()).build());
        assertTrue(ap.verified());
        assertTreesEqual(b.newDir, outDir);

        PreviewReport pv = client.preview(PreviewRequest.builder()
                .oldPath(b.oldDir.toString()).newPath(b.newDir.toString())
                .policy("balanced").build());
        assertFalse(pv.routes().isEmpty());
        assertFalse(pv.recommendedRoute().isBlank());

        BenchmarkReport bm = client.benchmark(BenchmarkRequest.builder()
                .oldPath(b.oldDir.toString()).newPath(b.newDir.toString())
                .measureApply(false).build());
        assertEquals(4, bm.routes().size());
    }

    @Test
    void errorMapping() {
        CavsException ex = assertThrows(CavsException.class, () ->
                client.analyze(AnalyzeRequest.builder()
                        .oldPath("/no/such/old").newPath("/no/such/new").build()));
        assertEquals(CavsErrorCode.PATH_NOT_FOUND, ex.code());
        assertFalse(ex.getMessage().isBlank());
    }

    @Test
    void progressCallbackReceivesEvents(@TempDir Path tmp) throws IOException {
        Builds b = makeBuilds(tmp);
        List<ProgressEvent> events = new CopyOnWriteArrayList<>();
        client.createPlan(CreatePlanRequest.builder()
                        .oldPath(b.oldDir.toString()).newPath(b.newDir.toString())
                        .outputPlan(tmp.resolve("p.cavsplan").toString()).build(),
                events::add);
        assertTrue(events.size() >= 2, "expected >=2 events, got " + events.size());
        assertTrue(events.stream().anyMatch(e -> "started".equals(e.type())));
    }

    @Test
    void asyncPreviewCompletes(@TempDir Path tmp) throws Exception {
        Builds b = makeBuilds(tmp);
        PreviewReport pv = client.previewAsync(PreviewRequest.builder()
                .oldPath(b.oldDir.toString()).newPath(b.newDir.toString()).build()).get();
        assertFalse(pv.recommendedRoute().isBlank());
    }

    @Test
    void asyncFailurePropagates() {
        var future = client.previewAsync(PreviewRequest.builder()
                .oldPath("/no/such/old").newPath("/no/such/new").build());
        ExecutionException ee = assertThrows(ExecutionException.class, future::get);
        assertInstanceOf(CavsException.class, ee.getCause());
        assertEquals(CavsErrorCode.PATH_NOT_FOUND, ((CavsException) ee.getCause()).code());
    }

    // ---- fixtures ----

    private record Builds(Path oldDir, Path newDir) {
    }

    private static Builds makeBuilds(Path root) throws IOException {
        Path old = Files.createDirectories(root.resolve("Build_v1/data"));
        Path neu = Files.createDirectories(root.resolve("Build_v2/data"));
        old = old.getParent();
        neu = neu.getParent();

        byte[] base = new byte[512 * 1024];
        for (int i = 0; i < base.length; i++) {
            base[i] = (byte) (i % 251);
        }
        Files.write(old.resolve("data/asset.bin"), base);
        byte[] changed = base.clone();
        for (int i = 300_000; i < 304_096; i++) {
            changed[i] ^= 0xFF;
        }
        Files.write(neu.resolve("data/asset.bin"), changed);
        Files.writeString(old.resolve("readme.txt"), "cavs sdk fixture\n");
        Files.writeString(neu.resolve("readme.txt"), "cavs sdk fixture\n");
        Files.write(neu.resolve("data/new_only.bin"), new byte[64 * 1024]);
        return new Builds(old, neu);
    }

    private static void assertTreesEqual(Path a, Path b) throws IOException {
        List<Path> files = new ArrayList<>();
        try (var walk = Files.walk(a)) {
            walk.filter(Files::isRegularFile).forEach(files::add);
        }
        Collections.sort(files);
        assertFalse(files.isEmpty(), "no files to compare");
        for (Path f : files) {
            Path rel = a.relativize(f);
            Path other = b.resolve(rel);
            assertTrue(Files.isRegularFile(other), "missing " + rel);
            assertArrayEquals(Files.readAllBytes(f), Files.readAllBytes(other), "content differs for " + rel);
        }
    }
}
