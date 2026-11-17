// Windows 上给 exe 嵌入图标和版本信息。图标和安卓启动图标同源，见 assets/icons/manifest.json。
// 其他平台、wasm32 目标什么都不做。
fn main() {
    println!("cargo:rerun-if-changed=assets/icons/spectrum.ico");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    embed_windows_resources();
}

#[cfg(windows)]
fn embed_windows_resources() {
    // 目标不是 Windows（例如在 Windows 上编 wasm32）就不嵌。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icons/spectrum.ico");
    res.set("FileDescription", "Spectrum 3D");
    res.set("ProductName", "Spectrum 3D");
    if let Err(e) = res.compile() {
        // 图标嵌不进去不该挡住构建，但要大声说出来。
        println!("cargo:warning=嵌入 Windows 图标失败：{e}");
    }
}
