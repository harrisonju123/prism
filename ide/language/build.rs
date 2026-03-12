fn main() {
    if let Ok(bundled) = std::env::var("PRISM_BUNDLE") {
        println!("cargo:rustc-env=PRISM_BUNDLE={}", bundled);
    }
}
