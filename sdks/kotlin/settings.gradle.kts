plugins {
    // Lets Gradle auto-provision the Java 22 toolchain when the runner
    // doesn't already have one.
    id("org.gradle.toolchains.foojay-resolver-convention") version "0.8.0"
}

rootProject.name = "cavs-sdk"
