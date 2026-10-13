//! BnTradingPro — Binance 现货/U本位合约交易终端
//!
//! 入口：日志初始化、配置加载、eframe 启动
// release 以 GUI 子系统启动（不带终端窗口）；debug 保留控制台便于看日志输出
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod binance;
mod config;
mod engine;
mod i18n;
mod security;
mod ui;

use config::AppConfig;
use eframe::egui;
use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result<()> {
    let dir = config::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let cfg = AppConfig::load(&dir);

    cleanup_old_logs(&dir.join("logs"));

    // 日志：滚动文件 + 控制台
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(cfg.log_level.clone()));
    let appender = tracing_appender::rolling::daily(dir.join("logs"), "app.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    // _log_guard 持有到进程结束，保证日志刷盘
    let _log_guard = guard;
    // log→tracing 桥接：eframe/egui/winit 用 log crate 报错（如事件循环异常），
    // 不桥接这些错误会静默丢失
    tracing_log::LogTracer::init().ok();

    // panic 钩子：panic 消息默认只走 stderr（release 无控制台即彻底丢失），
    // 这里补写一份到日志文件（含位置与 backtrace），便于事后定位
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let loc = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "<unknown>".into());
            let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "<non-string payload>".into()
            };
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            tracing::error!("PANIC at {loc}: {msg}\nbacktrace:\n{bt}");
            default_hook(info);
        }));
    }

    tracing::info!("BnTradingPro 启动，数据目录: {}", dir.display());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("BnTradingPro")
            .with_icon(app_icon())
            .with_inner_size([960.0, 600.0])
            .with_min_inner_size([733.0, 453.0]),
        ..Default::default()
    };

    eframe::run_native(
        "BnTradingPro",
        options,
        Box::new(move |cc| Ok(Box::new(app::BnApp::new(cc, cfg)))),
    )
}

/// 窗口/任务栏图标：解码内嵌 PNG（与 build.rs 嵌入 exe 资源的 ico 同一素材）
fn app_icon() -> egui::IconData {
    const PNG: &[u8] = include_bytes!("../assets/binance-icon.png");
    let img = image::load_from_memory(PNG).expect("解析内置图标失败");
    let rgba = img.to_rgba8();
    egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    }
}

/// 启动时清理 7 天前的滚动日志（app.log / app.log.YYYY-MM-DD）
fn cleanup_old_logs(logs_dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(7 * 24 * 3600);
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_log = name == "app.log" || name.starts_with("app.log.");
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if is_log && meta.is_file() && modified < cutoff {
            if let Err(e) = std::fs::remove_file(&path) {
                // 日志初始化尚未完成，只能静默忽略失败
                let _ = e;
            }
        }
    }
}
