//! Generates Swift and Kotlin sources: `cargo xtask uniffi` runs it.

fn main() {
    uniffi::uniffi_bindgen_main();
}
