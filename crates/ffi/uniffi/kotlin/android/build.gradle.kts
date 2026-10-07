plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

val repo = rootDir.resolve("../../../..")

android {
    namespace = "org.ekiben.kaisatsu"
    compileSdk = 36
    defaultConfig {
        minSdk = 24
    }
    sourceSets["main"].kotlin.srcDir(repo.resolve("target/uniffi/kotlin"))
    // `cargo xtask uniffi android` puts one libkaisatsu_uniffi.so per ABI here.
    sourceSets["main"].jniLibs.srcDir(repo.resolve("target/uniffi/android-jniLibs"))
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    // The generated code calls the library through JNA; Android needs the AAR flavour.
    api("net.java.dev.jna:jna:5.19.1@aar")
}
