plugins {
    `java-library`
    `maven-publish`
}

group = "com.cavs"
version = "1.1.0"

java {
    // The FFM native bridge (JEP 454) is finalized in Java 22.
    toolchain {
        languageVersion = JavaLanguageVersion.of(22)
    }
    withSourcesJar()
    withJavadocJar()
}

repositories {
    mavenCentral()
}

dependencies {
    api("com.fasterxml.jackson.core:jackson-databind:2.17.2")

    testImplementation(platform("org.junit:junit-bom:5.10.3"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.test {
    useJUnitPlatform()
    // FFM downcalls/upcalls require native access to be enabled.
    jvmArgs("--enable-native-access=ALL-UNNAMED")
    // Point the loader at a locally built native library. Set the
    // CAVS_SDK_LIBRARY env var (see the Makefile / CI) to the built
    // libcavs_sdk.{so,dylib} / cavs_sdk.dll.
    System.getenv("CAVS_SDK_LIBRARY")?.let {
        systemProperty("cavs.sdk.library", it)
    }
}

publishing {
    publications {
        create<MavenPublication>("maven") {
            from(components["java"])
            artifactId = "cavs-sdk"
            pom {
                name = "CAVS SDK"
                description = "Java SDK for CAVS, over the native Rust core via the C ABI"
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
}
