use std::env;
use std::path::PathBuf;

fn main() {
    // SDK root — set LIVE2D_SDK_ROOT or use default
    let sdk_root = match env::var("LIVE2D_SDK_ROOT") {
        Ok(val) => PathBuf::from(val),
        Err(_) => {
            let home = env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join("Downloads").join("CubismSdkForNative-5-r.5")
        }
    };

    let static_link = cfg!(feature = "static-link");

    let (lib_subdir, link_kind) = if static_link {
        ("lib", "static")
    } else {
        ("dll", "dylib")
    };

    let core_lib_dir = sdk_root
        .join("Core")
        .join(lib_subdir)
        .join("linux")
        .join("x86_64");

    let core_include = sdk_root.join("Core").join("include");

    println!("cargo:rustc-link-search=native={}", core_lib_dir.display());
    println!("cargo:rustc-link-lib={}=Live2DCubismCore", link_kind);

    if static_link {
        println!("cargo:rustc-link-lib=dylib=m");
    }

    println!("cargo:rerun-if-changed={}", core_include.join("Live2DCubismCore.h").display());

    // Generate bindings
    let bindings = bindgen::Builder::default()
        .header(core_include.join("Live2DCubismCore.h").to_string_lossy())
        .allowlist_function("csm.*")
        .allowlist_type("csm.*")
        .allowlist_var("csm.*")
        .opaque_type("csmMoc")
        .opaque_type("csmModel")
        .derive_default(true)
        .derive_debug(true)
        .use_core()
        .generate()
        .expect("bindgen failed to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Could not write bindings");
}
