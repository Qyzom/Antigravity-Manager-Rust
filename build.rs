fn main() {
    println!("cargo:rerun-if-changed=ui/app.slint");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/logo.ico");

    slint_build::compile("ui/app.slint").expect("Failed to compile Slint UI");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icons/logo.ico");
        res.set("ProductName", "Aroxy");
        res.set("FileDescription", "Aroxy AI Protocol Proxy & Quota Dispatcher");
        res.set("LegalCopyright", "Copyright (c) 2026 Qyzom");
        let _ = res.compile();
    }
}
