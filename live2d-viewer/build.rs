use std::env;
use std::path::PathBuf;

fn main() {
    let static_link = cfg!(feature = "static-link");
    if !static_link {
        let sdk_root = match env::var("LIVE2D_SDK_ROOT") {
            Ok(val) => PathBuf::from(val),
            Err(_) => {
                let home = env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                PathBuf::from(home).join("Downloads").join("CubismSdkForNative-5-r.5")
            }
        };
        let core_lib_dir = sdk_root.join("Core").join("dll").join("linux").join("x86_64");
        println!("cargo:rustc-link-arg-bin=live2d-viewer=-Wl,-rpath,{}", core_lib_dir.display());
    }
}
