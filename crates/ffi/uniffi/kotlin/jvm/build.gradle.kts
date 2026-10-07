plugins {
    id("org.jetbrains.kotlin.jvm")
    `java-library`
}

val repo = rootDir.resolve("../../../..")

kotlin {
    jvmToolchain(17)
    sourceSets.main {
        kotlin.srcDir(repo.resolve("target/uniffi/kotlin"))
    }
}

// JNA finds libraries bundled in a jar under `<os>-<arch>/`, e.g. `darwin-aarch64/`.
// `cargo xtask uniffi jvm` fills this directory with the host's library.
sourceSets.main {
    resources.srcDir(repo.resolve("target/uniffi/jvm-natives"))
}

dependencies {
    api("net.java.dev.jna:jna:5.19.1")
    testImplementation(kotlin("test"))
    testImplementation("org.json:json:20250517")
}

tasks.test {
    useJUnitPlatform()
    systemProperty("vectors", repo.resolve("spec/test-vectors/v1/vectors.json").path)
}
