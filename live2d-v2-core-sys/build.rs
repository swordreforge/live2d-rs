use std::env;
use std::path::PathBuf;

fn main() {
    // Path to the live2d-py build artifacts
    // Use V2_PY_BUILD_DIR env var, or derive relative to this source tree
    let py_build_dir = match env::var("V2_PY_BUILD_DIR") {
        Ok(val) => PathBuf::from(val),
        Err(_) => {
            // Default: assume live2d-py lives next to the Rust workspace root
            // CARGO_MANIFEST_DIR = .../live2d-rs/live2d-v2-core-sys
            // parent = .../live2d-rs  (workspace root, sibling to live2d-py/)
            PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
                .parent()
                .unwrap()
                .join("live2d-py")
                .join("build")
        }
    };

    // Library search path
    let glad_dir = py_build_dir.join("Live2D").join("Glad");
    let v2_dir = py_build_dir.join("Live2D").join("V2");
    let c_api_dir = py_build_dir.join("v2_c_api");

    println!("cargo:rustc-link-search=native={}", glad_dir.display());
    println!("cargo:rustc-link-search=native={}", v2_dir.display());
    println!("cargo:rustc-link-search=native={}", c_api_dir.display());

    // Link libraries
    println!("cargo:rustc-link-lib=static=v2_c_api");
    println!("cargo:rustc-link-lib=static=V2");
    println!("cargo:rustc-link-lib=static=glad");

    // System libraries needed on Linux
    if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-lib=dylib=GL");
        println!("cargo:rustc-link-lib=dylib=stdc++fs");
        println!("cargo:rustc-link-lib=dylib=stdc++");
        println!("cargo:rustc-link-lib=dylib=m");
    }
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=dylib=c++");
        println!("cargo:rustc-link-lib=dylib=framework=OpenGL");
    }

    // Header path for bindgen
    let header = py_build_dir
        .parent()
        .unwrap()
        .join("v2_c_api")
        .join("v2_c_api.h");

    println!("cargo:rerun-if-changed={}", header.display());

    // Generate bindings
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .allowlist_function("v2_.*")
        .allowlist_type("V2Model")
        .opaque_type("V2Model")
        .derive_default(true)
        .derive_debug(true)
        .use_core()
        .generate()
        .expect("bindgen failed to generate V2 C API bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Could not write V2 bindings");
}
