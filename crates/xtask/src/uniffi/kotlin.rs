//! Kotlin: runs the shared vectors on the JVM, builds the jar, and (when the Android NDK and
//! `cargo-ndk` are there) the AAR. The Gradle project is `crates/ffi/uniffi/kotlin`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Result, run_command};

use super::generate::{Generated, PROFILE};

const ABIS: [&str; 4] = ["arm64-v8a", "armeabi-v7a", "x86_64", "x86"];

pub(crate) fn run(generated: &Generated) -> Result {
    let project = generated.root.join("crates/ffi/uniffi/kotlin");
    let sdk = android_sdk();

    // JNA finds a library bundled in the jar by its `<os>-<arch>` directory.
    let natives = generated.out_dir.join("jvm-natives");
    let _ = std::fs::remove_dir_all(&natives);
    let platform = natives.join(jna_platform()?);
    std::fs::create_dir_all(&platform)?;
    std::fs::copy(
        &generated.library,
        platform.join(generated.library.file_name().ok_or("no library name")?),
    )?;
    gradle(&project, sdk.as_deref(), &[":jvm:test", ":jvm:jar"])?;
    println!(
        "jar: {}",
        project.join("jvm/build/libs/jvm-0.1.0.jar").display()
    );

    let Some(sdk) = sdk else {
        println!("(no Android SDK found: set ANDROID_HOME to build the AAR)");
        return Ok(());
    };
    let Some(ndk) = newest_ndk(&sdk) else {
        println!(
            "(no Android NDK in {}/ndk: install one to build the AAR)",
            sdk.display()
        );
        return Ok(());
    };
    if Command::new("cargo-ndk").arg("--version").output().is_err() {
        println!("(install `cargo install cargo-ndk` to build the AAR)");
        return Ok(());
    }

    let jni_libs = generated.out_dir.join("android-jniLibs");
    let _ = std::fs::remove_dir_all(&jni_libs);
    let mut ndk_build = Command::new(env!("CARGO"));
    ndk_build
        .current_dir(&generated.root)
        .env("ANDROID_NDK_HOME", &ndk)
        .arg("ndk")
        .arg("-o")
        .arg(&jni_libs);
    for abi in ABIS {
        ndk_build.args(["-t", abi]);
    }
    run_command(ndk_build.args([
        "build",
        "--package",
        "kaisatsu-uniffi",
        "--profile",
        PROFILE,
    ]))?;
    gradle(&project, Some(&sdk), &[":android:assembleRelease"])?;
    println!(
        "aar: {}",
        project
            .join("android/build/outputs/aar/android-release.aar")
            .display()
    );
    Ok(())
}

fn gradle(project: &Path, sdk: Option<&Path>, tasks: &[&str]) -> Result {
    let mut command = Command::new(project.join("gradlew"));
    command
        .current_dir(project)
        .arg("--console=plain")
        .args(tasks);
    if let Some(sdk) = sdk {
        command.env("ANDROID_HOME", sdk);
    }
    run_command(&mut command)
}

fn jna_platform() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-aarch64",
        ("macos", "x86_64") => "darwin-x86-64",
        ("linux", "x86_64") => "linux-x86-64",
        ("linux", "aarch64") => "linux-aarch64",
        ("windows", "x86_64") => "win32-x86-64",
        (os, arch) => return Err(format!("no JNA platform name for {os}/{arch}").into()),
    })
}

fn android_sdk() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    std::env::var_os("ANDROID_HOME")
        .or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
        .map(PathBuf::from)
        .into_iter()
        .chain(
            home.iter()
                .flat_map(|home| [home.join("Library/Android/sdk"), home.join("Android/Sdk")]),
        )
        .find(|path| path.is_dir())
}

fn newest_ndk(sdk: &Path) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = std::fs::read_dir(sdk.join("ndk"))
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.is_dir())
        .collect();
    versions.sort();
    versions.pop()
}
