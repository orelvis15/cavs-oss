plugins {
    kotlin("jvm") version "2.0.21"
    kotlin("plugin.serialization") version "2.0.21"
    `java-library`
    `maven-publish`
}

group = "io.github.orelvis15"
version = "1.2.0"

kotlin {
    // The FFM native bridge (JEP 454) is finalized in Java 22.
    jvmToolchain(22)
}

java {
    withSourcesJar()
}

repositories {
    mavenCentral()
}

dependencies {
    api("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")

    testImplementation(kotlin("test"))
    testImplementation(platform("org.junit:junit-bom:5.10.3"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.test {
    useJUnitPlatform()
    // FFM downcalls/upcalls require native access to be enabled.
    jvmArgs("--enable-native-access=ALL-UNNAMED")
    // Point the loader at a locally built native library via CAVS_SDK_LIBRARY.
    System.getenv("CAVS_SDK_LIBRARY")?.let { systemProperty("cavs.sdk.library", it) }
}

publishing {
    publications {
        create<MavenPublication>("maven") {
            from(components["java"])
            artifactId = "cavs-sdk"
            pom {
                name = "CAVS SDK"
                description = "Kotlin/JVM SDK for CAVS, over the native Rust core via the C ABI"
                url = "https://github.com/orelvis15/cavs-oss"
                licenses {
                    license {
                        name = "Apache-2.0"
                        url = "https://www.apache.org/licenses/LICENSE-2.0"
                    }
                }
            }
        }
    }
    repositories {
        // Target repository is supplied by the release workflow (Maven
        // Central portal, GitHub Packages, or any Maven repo). When unset,
        // only `publishToMavenLocal` is usable.
        val repoUrl = providers.gradleProperty("mavenRepoUrl")
            .orElse(providers.environmentVariable("MAVEN_REPO_URL"))
        if (repoUrl.isPresent && repoUrl.get().isNotBlank()) {
            maven {
                url = uri(repoUrl.get())
                credentials {
                    username = providers.gradleProperty("mavenRepoUser")
                        .orElse(providers.environmentVariable("MAVEN_REPO_USER")).orNull
                    password = providers.gradleProperty("mavenRepoPassword")
                        .orElse(providers.environmentVariable("MAVEN_REPO_PASSWORD")).orNull
                }
            }
        }
    }
}
