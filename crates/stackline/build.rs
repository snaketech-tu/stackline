// The UI is embedded from ui/dist. Create a placeholder when it has not been
// built yet, so `cargo build` and `cargo test` work on a fresh checkout.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    if !dist.join("index.html").exists() {
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(
            dist.join("index.html"),
            "<!doctype html><title>Stackline</title><p>The UI has not been built. Run <code>npm run build</code> in <code>ui/</code>.</p>",
        )
        .unwrap();
    }
    println!("cargo:rerun-if-changed=../../ui/dist/index.html");
    // Self-update downloads the release binary built for the same target.
    println!("cargo:rustc-env=STACKLINE_TARGET={}", std::env::var("TARGET").unwrap());
    println!("cargo:rerun-if-changed=updater.pub");
}
