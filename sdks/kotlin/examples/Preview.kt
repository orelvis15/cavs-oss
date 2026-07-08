// Runs a CAVS update preview between two build directories you already have.
//
//   gradle -q runPreview --args="--old Build_v1 --new Build_v2"
package com.cavs.examples

import com.cavs.sdk.CavsClient
import com.cavs.sdk.model.PreviewRequest
import kotlin.system.exitProcess

fun main(args: Array<String>) {
    val oldPath = valueOf(args, "--old")
    val newPath = valueOf(args, "--new")
    if (oldPath == null || newPath == null) {
        System.err.println("usage: preview --old <dir> --new <dir>")
        exitProcess(2)
    }

    CavsClient.create().use { cavs ->
        val report = cavs.preview(
            PreviewRequest(oldPath = oldPath, newPath = newPath, policy = "balanced"),
        ) { e -> e.phase?.let { System.err.println("  [${e.type}] $it") } }

        println("Recommended route: ${report.recommendedRoute}")
        for (r in report.routes) {
            println("  ${r.name.padEnd(16)} ${r.networkBytes} bytes")
        }
    }
}

private fun valueOf(args: Array<String>, flag: String): String? {
    val i = args.indexOf(flag)
    return if (i >= 0 && i + 1 < args.size) args[i + 1] else null
}
