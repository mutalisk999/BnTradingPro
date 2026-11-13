//! BnTradingPro — Binance 现货/U本位合约交易终端
//!
//! 入口：日志初始化、配置加载、GPUI Application 启动
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
use gpui::{
    px, point, size, Application, Bounds, TitlebarOptions, WindowBounds, WindowKind,
    WindowOptions,
};
use tracing_subscriber::EnvFilter;

fn main() {
    let dir = config::data_dir();
    let _ = std::fs::create_dir_all(dir);
    let cfg = AppConfig::load(dir);

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
    // log→tracing 桥接：依赖库用 log crate 报错（如平台层异常），
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

    Application::new().run(move |cx: &mut gpui::App| {
        // 字体：Windows 系统字体注入（雅黑正文 / Cascadia 等宽数字）；
        // 主题模式/涨跌方向由静态原子承载，取色经 theme::c() 现算
        ui::theme::apply_fonts(cx);
        ui::theme::set_mode_str(&cfg.theme_mode);
        ui::theme::set_price_dir_str(&cfg.price_dir);

        // 窗口参数：初始 1200×600（= 最小尺寸，默认即最大信息密度布局）
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(
                px(ui::theme::START_W as f32),
                px(ui::theme::START_H as f32),
            ),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("BnTradingPro".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(
                px(ui::theme::MIN_W as f32),
                px(ui::theme::MIN_H as f32),
            )),
            kind: WindowKind::Normal,
            ..Default::default()
        };

        // 窗口图标：GPUI Windows 后端自动从 exe 资源 ID 1 加载
        // （build.rs winresource 已嵌入，无需代码）
        let result = cx.open_window(options, |window, cx| {
            app::BnApp::new(window, cx, cfg.clone())
        });
        if let Err(e) = result {
            tracing::error!("打开主窗口失败: {e:#}");
        }
    });
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
