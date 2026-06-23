use std::env;
use std::path::PathBuf;

fn sdk_root_dir() -> PathBuf {
    if let Ok(val) = env::var("LIVE2D_SDK_ROOT") {
        return PathBuf::from(val);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("CubismSdkForNative-5-r.5")
}

fn main() {
    let static_link = cfg!(feature = "static-link");
    if !static_link {
        let sdk_root = sdk_root_dir();
        let core_lib_dir = sdk_root
            .join("Core")
            .join("dll")
            .join("linux")
            .join("x86_64");
        println!(
            "cargo:rustc-link-arg-bin=live2d-viewer=-Wl,-rpath,{}",
            core_lib_dir.display()
        );
    }
}
