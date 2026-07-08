package com.cavs.sdk

import com.cavs.sdk.model.AnalyzeRequest
import com.cavs.sdk.model.ApplyPlanRequest
import com.cavs.sdk.model.BenchmarkRequest
import com.cavs.sdk.model.CreatePlanRequest
import com.cavs.sdk.model.PackDirectoryRequest
import com.cavs.sdk.model.PreviewRequest
import com.cavs.sdk.model.ProgressEvent
import com.cavs.sdk.model.SavingsRequest
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.io.path.readBytes
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class CavsClientTest {

    private lateinit var client: CavsClient

    @BeforeTest
    fun setUp() {
        client = CavsClient.create()
    }

    @AfterTest
    fun tearDown() {
        if (::client.isInitialized) client.close()
    }

    @Test
    fun versionIsSemver() {
        assertEquals(3, client.version().split(".").size)
        assertEquals("1.0.0", client.abiVersion())
    }

    @Test
    fun estimateSavings() {
        val r = client.estimateSavings(
            SavingsRequest(
                pricePerGb = 0.08,
                monthlyDownloads = 500_000.0,
                averageFullDownloadBytes = 65_011_712.0,
                averageCavsDownloadBytes = 2_631_921.0,
            ),
        )
        assertTrue(r.savingsPercent > 90.0, "savingsPercent=${r.savingsPercent}")
        assertTrue(r.estimatedMonthlySavings > r.cavsMonthlyCost)
    }

    @Test
    fun fullPipeline() {
        val (old, new) = makeBuilds()
        val work = Files.createTempDirectory("cavs-work")

        val an = client.analyze(AnalyzeRequest(oldPath = old.toString(), newPath = new.toString()))
        assertTrue(an.summary.newSizeBytes > 0)

        val packOut = work.resolve("v2.cavs")
        val pk = client.packDirectory(
            PackDirectoryRequest(inputDir = new.toString(), outputCavs = packOut.toString()),
        )
        assertTrue(pk.filesPacked >= 3)
        assertTrue(Files.isRegularFile(packOut))

        val planPath = work.resolve("update.cavsplan")
        val pl = client.createPlan(
            CreatePlanRequest(oldPath = old.toString(), newPath = new.toString(), outputPlan = planPath.toString()),
        )
        assertTrue(pl.reusedBytes > 0, "plan found no reuse")

        val outDir = work.resolve("out")
        val ap = client.applyPlan(
            ApplyPlanRequest(oldPath = old.toString(), planPath = planPath.toString(), outputPath = outDir.toString()),
        )
        assertTrue(ap.verified)
        assertTreesEqual(new, outDir)

        val pv = client.preview(
            PreviewRequest(oldPath = old.toString(), newPath = new.toString(), policy = "balanced"),
        )
        assertTrue(pv.routes.isNotEmpty())
        assertTrue(pv.recommendedRoute.isNotBlank())

        val bm = client.benchmark(
            BenchmarkRequest(oldPath = old.toString(), newPath = new.toString(), measureApply = false),
        )
        assertEquals(4, bm.routes.size)
    }

    @Test
    fun errorMapping() {
        val ex = assertFailsWith<CavsException> {
            client.analyze(AnalyzeRequest(oldPath = "/no/such/old", newPath = "/no/such/new"))
        }
        assertEquals(CavsErrorCode.PATH_NOT_FOUND, ex.code)
        assertTrue(ex.message!!.isNotBlank())
    }

    @Test
    fun progressCallbackReceivesEvents() {
        val (old, new) = makeBuilds()
        val work = Files.createTempDirectory("cavs-work")
        val events = CopyOnWriteArrayList<ProgressEvent>()
        client.createPlan(
            CreatePlanRequest(oldPath = old.toString(), newPath = new.toString(), outputPlan = work.resolve("p.cavsplan").toString()),
        ) { events.add(it) }
        assertTrue(events.size >= 2, "expected >= 2 events, got ${events.size}")
        assertTrue(events.any { it.type == "started" })
    }

    @Test
    fun asyncPreviewCompletes() {
        val (old, new) = makeBuilds()
        val pv = client.previewAsync(
            PreviewRequest(oldPath = old.toString(), newPath = new.toString()),
        ).get()
        assertTrue(pv.recommendedRoute.isNotBlank())
    }

    // ---- fixtures ----

    private fun makeBuilds(): Pair<Path, Path> {
        val root = Files.createTempDirectory("cavs-builds")
        val old = Files.createDirectories(root.resolve("Build_v1/data")).parent
        val new = Files.createDirectories(root.resolve("Build_v2/data")).parent

        val base = ByteArray(512 * 1024) { (it % 251).toByte() }
        Files.write(old.resolve("data/asset.bin"), base)
        val changed = base.copyOf()
        for (i in 300_000 until 304_096) changed[i] = (changed[i].toInt() xor 0xFF).toByte()
        Files.write(new.resolve("data/asset.bin"), changed)
        Files.writeString(old.resolve("readme.txt"), "cavs sdk fixture\n")
        Files.writeString(new.resolve("readme.txt"), "cavs sdk fixture\n")
        Files.write(new.resolve("data/new_only.bin"), ByteArray(64 * 1024))
        return old to new
    }

    private fun assertTreesEqual(a: Path, b: Path) {
        var checked = 0
        Files.walk(a).use { stream ->
            stream.filter { Files.isRegularFile(it) }.forEach { file ->
                val rel = a.relativize(file)
                assertContentEquals(file.readBytes(), b.resolve(rel).readBytes(), "differs: $rel")
                checked++
            }
        }
        assertTrue(checked > 0, "no files compared")
    }
}
