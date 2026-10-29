// build.rs - embed Windows icon and version info
fn main() {
    // Tell cargo to re-run if the icon changes
    println!("cargo:rerun-if-changed=assets/icons/icon.ico");
    println!("cargo:rerun-if-changed=resource.rc");

    // Only embed resources on Windows
    #[cfg(target_os = "windows")]
    {
        use winres::WindowsResource;

        let mut res = WindowsResource::new();
        let file_version = format!("{}.0", env!("CARGO_PKG_VERSION"));
        res.set_icon("assets/icons/icon.ico");
        res.set("FileDescription", "DM Ultra Download Manager");
        res.set("FileVersion", &file_version);
        res.set("ProductName", "DM Ultra");
        res.set("ProductVersion", &file_version);
        res.set("CompanyName", "DM Ultra");
        res.set("LegalCopyright", "Copyright (C) 2026");
        res.set("OriginalFilename", "dmultra.exe");
        res.set("InternalName", "dmultra");
        res.compile().unwrap();
    }
}