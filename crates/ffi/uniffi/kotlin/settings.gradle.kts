rootProject.name = "kaisatsu"

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

// `jvm`: a jar for desktop and server JVMs. `android`: an AAR, only where an Android SDK is.
include(":jvm")
if (System.getenv("ANDROID_HOME") != null || file("local.properties").exists()) {
    include(":android")
}
