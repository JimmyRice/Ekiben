// The Kotlin sources are generated into target/uniffi/kotlin by `cargo xtask uniffi`, and the
// native libraries by `cargo xtask uniffi jvm|android`; neither is checked in.
plugins {
    id("com.android.library") version "8.13.2" apply false
    id("org.jetbrains.kotlin.android") version "2.3.21" apply false
    id("org.jetbrains.kotlin.jvm") version "2.3.21" apply false
}

allprojects {
    group = "org.ekiben"
    version = "0.1.0"
}
