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

    tracing::info!("BnTradingPro 启动，数据目录: {}", dir.display());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("BnTradingPro")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1100.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "BnTradingPro",
        options,
        Box::new(move |cc| Ok(Box::new(app::BnApp::new(cc, cfg)))),
    )
}
