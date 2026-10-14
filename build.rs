//! 构建脚本：Windows 下把应用图标（PNG → ico）嵌入 exe 资源，
//! 资源管理器/任务栏/标题栏显示该图标；窗口内图标由 main.rs 运行时
//! 解码同一份 PNG（egui ViewportBuilder::with_icon）。

fn main() {
    println!("cargo:rerun-if-changed=assets/binance-icon.png");

    #[cfg(windows)]
    {
        let png =
            std::fs::read("assets/binance-icon.png").expect("读取 assets/binance-icon.png 失败");
        let img = image::load_from_memory(&png).expect("解析图标 PNG 失败");
        // ico 资源用 256×256 单尺寸：Windows 会按需缩放小尺寸
        let ico = img.resize_exact(256, 256, image::imageops::FilterType::Lanczos3);
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR 未设置");
        let ico_path = std::path::Path::new(&out_dir).join("app.ico");
        ico.save_with_format(&ico_path, image::ImageFormat::Ico)
            .expect("生成 app.ico 失败");
        winresource::WindowsResource::new()
            .set_icon(ico_path.to_str().expect("ico 路径非 UTF-8"))
            .compile()
            .expect("嵌入图标资源失败（需要 MSVC 工具链的 rc.exe）");
    }
}
