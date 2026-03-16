fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_manifest_file("app.manifest");
        res.compile().expect("Failed to compile Windows resources");
    }
    tauri_build::build()
}
