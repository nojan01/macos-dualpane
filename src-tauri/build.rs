fn main() {
    #[cfg(target_os = "macos")]
    {
        cc::Build::new()
            .file("src/promise_drag.m")
            .flag("-fobjc-arc")
            .flag("-fmodules")
            .compile("dualbeam_promise_drag");
        println!("cargo:rustc-link-lib=framework=Cocoa");
        println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");
        println!("cargo:rerun-if-changed=src/promise_drag.m");
        println!("cargo:rerun-if-changed=src/promise_drag.h");
    }
    tauri_build::build()
}
