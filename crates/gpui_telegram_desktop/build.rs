fn main() {
    println!("cargo:rerun-if-changed=assets/AppIcon.ico");
    println!("cargo:rerun-if-changed=native/macos_permission_hud.m");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        #[cfg(target_os = "macos")]
        {
            cc::Build::new()
                .file("native/macos_permission_hud.m")
                .flag("-fobjc-arc")
                .flag("-Wno-deprecated-declarations")
                .compile("fabushi_macos_permission_hud");

            println!("cargo:rustc-link-lib=framework=AppKit");
            println!("cargo:rustc-link-lib=framework=Foundation");
            println!("cargo:rustc-link-lib=framework=CoreGraphics");
            println!("cargo:rustc-link-lib=framework=QuartzCore");
        }
    }

    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            let icon_path = std::path::Path::new("assets/AppIcon.ico");
            if icon_path.exists() {
                res.set_icon(icon_path.to_str().unwrap_or("assets/AppIcon.ico"));
            }
            res.set("ProductName", "Fabushi");
            res.set("FileDescription", "Fabushi Desktop");
            res.set("CompanyName", "Fabushi Contributors");
            res.set("LegalCopyright", "Copyright (c) 2026 Fabushi Contributors");
            res.set("OriginalFilename", "Fabushi.exe");
            res.set("InternalName", "Fabushi");
            if let Err(err) = res.compile() {
                panic!("Failed to compile Windows resources: {err}");
            }
        }
    }
}
