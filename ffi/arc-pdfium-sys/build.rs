use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir.parent().unwrap().parent().unwrap();

    println!("cargo:rerun-if-env-changed=ARCPDF_PDFIUM_TARGET");
    println!("cargo:rerun-if-changed={}", repo_root.join("pdfium/pdfium-revision.toml").display());

    let target = env::var("ARCPDF_PDFIUM_TARGET").unwrap_or_else(|_| "macos_arm64".to_string());
    let artifact_dir = repo_root.join("artifacts/pdfium").join(&target);
    let include_dir = artifact_dir.join("include");
    let lib_path = artifact_dir.join("libpdfium.a");
    let manifest_path = artifact_dir.join("manifest.json");

    if !lib_path.exists() || !include_dir.exists() || !manifest_path.exists() {
        panic!(
            "PDFium artifacts for {} are missing. Run: scripts/build-pdfium --target {} (Phase 1-B)",
            target, target
        );
    }

    let lib_dir = lib_path.parent().unwrap();
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=pdfium");
    println!("cargo:rustc-link-lib=c++");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");

    let header = include_dir.join("fpdfview.h");
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", include_dir.display()))
        .allowlist_function("FPDF.*")
        .allowlist_type("FPDF.*")
        .allowlist_type("FS_.*")
        .allowlist_var("FPDF.*")
        .derive_debug(false)
        .layout_tests(false)
        .generate()
        .expect("failed to generate PDFium bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings.write_to_file(out_path).expect("failed to write bindings");
}
