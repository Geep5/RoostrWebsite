//! Stamps the build time into the Settings footer (the web app's `__BUILD_STAMP__`).

fn main() {
    let stamp = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC");
    println!("cargo:rustc-env=ROOSTR_BUILD_STAMP={stamp}");
    println!("cargo:rerun-if-changed=src");
}
