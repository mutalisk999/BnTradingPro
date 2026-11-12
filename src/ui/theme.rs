//! 主题与字体：明暗双主题 + Windows 系统字体（微软雅黑正文 / Cascadia 等宽数字）
//!
//! 颜色通过 `UiTheme` 全局（gpui::Global）运行时取值，主题切换即时生效。
//! 色板沿用 egui 版的 RGB 值（经 rgb()/rgba() 转 GPUI Rgba），保证视觉零回归。

use gpui::{px, App, Hsla, Pixels, Rgba};
use std::sync::atomic::{AtomicU8, Ordering};

/// 常量版 RGB 转换：gpui 的 `rgb()` 是普通函数不能在 `const` 里调用，
/// 而色板 DARK/LIGHT 是 const 项。语义与 gpui::rgb 一致（u32 hex → Rgba）
const fn rgb_const(hex: u32) -> Rgba {
    Rgba {
        r: (((hex >> 16) & 0xff) as f32) / 255.0,
        g: (((hex >> 8) & 0xff) as f32) / 255.0,
        b: ((hex & 0xff) as f32) / 255.0,
        a: 1.0,
    }
}

// ---------- 字号 / 控件高度 token ----------
/// 全应用仅 5 档字号：小字（辅助小字）→ 次要（表格/说明）→ 正文 → 页面标题 → 大数字。
/// 字号按"字体大小"设置整体缩放：全部取值为函数而非 const，读取时
/// 乘当前档位系数，设置变更后下一帧即生效（无需重装字体）
///
/// **缩放取舍**：行高/按钮高/面板宽等布局常量（BTN_H/ROW_H 等）不随字号
/// 档位变化，故档位幅度刻意保守（中 ×1.15、大 ×1.3），避免大字号顶死
/// 固定行高的纵向留白
static FONT_STEP: AtomicU8 = AtomicU8::new(0); // 0=小 1=中 2=大

/// 按配置字符串设档位（"small"/"medium"/"large"，其余按小处理——
/// 老配置反序列化的空串也落入"小"）
pub fn set_font_size_str(s: &str) {
    let step = match s {
        "medium" => 1,
        "large" => 2,
        _ => 0,
    };
    FONT_STEP.store(step, Ordering::Relaxed);
}

#[cfg(test)]
mod test_util {
    use super::set_font_size_str;
    use std::sync::Mutex;

    /// 仅测试用：FONT_STEP 是进程级全局量，cargo test 的并行线程会互相
    /// 踩——一个用例设 large 量几何的同时，另一个正按 small 断言。
    /// 改读档位或按 font_*() 量几何的用例统一先拿这把锁；
    /// guard 释放时把档位恢复 small，下个持锁者看到的是干净基线
    pub struct FontGuard(std::sync::MutexGuard<'static, ()>);

    impl Drop for FontGuard {
        fn drop(&mut self) {
            set_font_size_str("small");
        }
    }

    pub fn font_lock() -> FontGuard {
        static LOCK: Mutex<()> = Mutex::new(());
        FontGuard(LOCK.lock().unwrap())
    }
}

#[cfg(test)]
pub use test_util::{font_lock, FontGuard};

/// 档位系数：小 ×1.0（当前基准）、中 ×1.15、大 ×1.3
fn font_scale() -> f32 {
    match FONT_STEP.load(Ordering::Relaxed) {
        1 => 1.15,
        2 => 1.3,
        _ => 1.0,
    }
}

pub fn font_sm() -> Pixels {
    px(12.0 * font_scale())
}
pub fn font_md() -> Pixels {
    px(13.5 * font_scale())
}
pub fn font_body() -> Pixels {
    px(14.0 * font_scale())
}
/// 品牌名等需要强调的展示型文字
pub fn font_xl() -> Pixels {
    px(20.0 * font_scale())
}
pub fn font_lg() -> Pixels {
    px(16.5 * font_scale())
}
pub fn font_hero() -> Pixels {
    px(20.0 * font_scale())
}
/// 解锁/配钥页的品牌标题（页面级展示字，不属于常规字号阶梯）
pub fn font_brand() -> Pixels {
    px(34.0 * font_scale())
}
/// 图表水位印（装饰性大字，同样不属于常规字号阶梯，但纳入 token 体系
/// 统一管理，避免裸字面量散落在绘制代码里）
pub fn font_watermark() -> Pixels {
    px(58.0 * font_scale())
}

/// 常规按钮/页签高度
pub const BTN_H: f32 = 28.0;
/// 主操作按钮高度（买卖切换、下单提交）
pub const BTN_LG: f32 = 36.0;
/// 表单行内输入控件统一最小高
pub const CONTROL_H: f32 = 26.0;
/// 表单行标准高度（≥ 控件高，留少量呼吸空间）
pub const ROW_H: f32 = 30.0;

// ---------- 圆角半径 token ----------
/// 小徽标 / 行内幽灵按钮
pub const R_SM: f32 = 4.0;
/// 常规按钮 / 胶囊页签
pub const R_MD: f32 = 6.0;
/// 卡片 / 弹窗 / Toast
pub const R_LG: f32 = 8.0;

// ---------- 价格变动高亮（flash）token ----------
/// flash 高亮持续时长：价格变化后行底色在此时间内渐变回原色
pub const FLASH_SECS: f32 = 0.5;
/// flash 行底色不透明度（0-1）：叠在行背景之上的涨/跌色强度
pub const FLASH_ALPHA: f32 = 0.28;
/// flash 文字提亮幅度（0-1）：无法铺行底色的区域用文字向主题文字色插值代替
pub const FLASH_BRIGHTEN: f32 = 0.5;
/// 盘口消失档位残影的淡出时长（秒）：吃单后旧档位底色在此时间内渐隐
pub const GHOST_SECS: f64 = 0.4;

// ---------- 心跳重绘调度（UI 侧自适应心跳间隔，毫秒）----------
/// 图表/动画档：≈30fps，K 线最新价与闪烁衰减都在此刷新
pub const REPAINT_CHART: u64 = 33;
/// 行情档：盘口深度 20 档 @100ms 推送，盘口/页头/成交随之刷新
pub const REPAINT_QUOTE: u64 = 100;
/// 空闲档：无行情变动的页面（资产/设置/帮助）保持低频重绘
pub const REPAINT_IDLE: u64 = 400;

/// 窗口初始逻辑尺寸
pub const START_W: f64 = 1200.0;
pub const START_H: f64 = 600.0;
/// 窗口最小逻辑尺寸（初始即最小：默认打开就是最大信息密度布局）
pub const MIN_W: f64 = 1200.0;
pub const MIN_H: f64 = 600.0;

// ---------- 主题定义 ----------

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub up: Rgba,
    pub down: Rgba,
    pub bg: Rgba,
    pub panel: Rgba,
    pub panel2: Rgba,
    pub card: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub accent: Rgba,
    pub warn: Rgba,
    pub err: Rgba,
    /// 图表横向网格线
    pub grid: Rgba,
    /// 图表纵向时间网格线
    pub grid_time: Rgba,
    /// 十字光标
    pub crosshair: Rgba,
    /// 图表悬浮信息框背景
    pub tooltip_bg: Rgba,
    /// 轴标签/时间标签底色
    pub tag_bg: Rgba,
    /// 列表行悬停高亮
    pub row_hover: Rgba,
    /// 大按钮未选中态底色（买/卖切换、页签等）
    pub btn_idle: Rgba,
    /// 涨/跌等彩色填充（买卖按钮、最新价标签）上的前景文字
    pub on_fill: Rgba,
    /// 金色 accent 填充（主按钮）上的前景文字
    pub on_accent: Rgba,
}

pub const DARK: Theme = Theme {
    up: rgb_const(0x0ecb81),
    down: rgb_const(0xf6465d),
    bg: rgb_const(0x0a0d12),
    panel: rgb_const(0x12161e),
    panel2: rgb_const(0x191f2a),
    card: rgb_const(0x1f2632),
    border: rgb_const(0x2c3542),
    text: rgb_const(0xe8eaed),
    text_dim: rgb_const(0x8b95a5),
    accent: rgb_const(0xf0b90b),
    warn: rgb_const(0xf0a030),
    err: rgb_const(0xf6465d),
    grid: rgb_const(0x1c222c),
    grid_time: rgb_const(0x1a202a),
    crosshair: rgb_const(0x788294),
    tooltip_bg: rgb_const(0x181e29),
    tag_bg: rgb_const(0x2c3442),
    row_hover: rgb_const(0x262e3c),
    btn_idle: rgb_const(0x1e2530),
    // 涨/跌填充上的文字用深色：白色在 up=rgb_const(14,203,129) 上对比度仅 2.1:1，
    // 深色对 up 8.1:1、对 down 4.9:1
    on_fill: rgb_const(0x062016),
    on_accent: rgb_const(0x1c1602),
};

pub const LIGHT: Theme = Theme {
    up: rgb_const(0x069e62),
    down: rgb_const(0xd63a50),
    // 纸面暖底：冷蓝灰换成米黄"报纸"色——大面积纯白/冷灰的眩光降一档（护眼），
    // 层次仍是 bg 最暗 → panel/card 最亮，卡片浮在纸面上
    bg: rgb_const(0xf0ede5),
    panel: rgb_const(0xfcfaf3),
    panel2: rgb_const(0xf5f2e9),
    // 卡片近白（暖）：与纸面 bg 拉开层次，卡片"浮"在页面上
    card: rgb_const(0xfaf8f0),
    // 边框暖化并加深：原冷灰边框在浅底上对比偏弱，面板/卡片轮廓更实
    border: rgb_const(0xcfc8b8),
    // 暖墨色：报纸式深墨偏黄，在暖纸底上更融洽
    text: rgb_const(0x1c1b17),
    // 暖灰并加深：在 (250,248,240) 卡片底上对比度从 5.3:1 提到 6.1:1
    text_dim: rgb_const(0x615d52),
    // accent/warn 加深：原色在白底上对比度 2.7:1 / 3.5:1，不满足 4.5:1
    accent: rgb_const(0x8c6600),
    warn: rgb_const(0x926008),
    err: rgb_const(0xd63a50),
    grid: rgb_const(0xddd8cb),
    grid_time: rgb_const(0xe2ddd1),
    crosshair: rgb_const(0x747062),
    tooltip_bg: rgb_const(0xfdfaf3),
    tag_bg: rgb_const(0xdcd6c7),
    row_hover: rgb_const(0xeee9dd),
    btn_idle: rgb_const(0xe5dfd0),
    // 彩色填充（up/down）上的文字用黑色：白色对浅色 up 仅 3.5:1；
    // 黑色对 up 6.1:1、对 down 4.6:1
    on_fill: rgb_const(0x000000),
    // accent 加深后深色字对比度跌至 3.1:1，改白色（5.2:1）
    on_accent: rgb_const(0xffffff),
};

// ---------- 全局主题（gpui::Global） ----------

/// UI 全局主题：渲染时经 `cx.global::<UiTheme>()` 取当前色板。
/// 字段含 price_dir 翻转后的 cur() 语义（up/down 已按涨跌方向互换），
/// 组件侧无需再关心方向配置
pub struct UiTheme {
    /// 当前色板（up/down 已按红涨绿跌配置翻转）
    pub t: Theme,
    /// 浅色主题标记（MA 配色等按主题取变体的场合用）
    pub light: bool,
}

impl gpui::Global for UiTheme {}

impl UiTheme {
    /// 由配置字符串初始化全局主题（mode: "light"/"dark"；price_dir: "red_up"/...）
    pub fn install(cx: &mut App, mode_str: &str, price_dir_str: &str) {
        set_mode_str(mode_str);
        set_price_dir_str(price_dir_str);
        cx.set_global(UiTheme {
            t: cur(),
            light: is_light(),
        });
    }

    /// 主题或方向配置变更后调用：重算色板并写回全局
    pub fn refresh(&mut self) {
        self.t = cur();
        self.light = is_light();
    }
}

/// 组件取色入口：当前主题色板（up/down 已按方向翻转）
pub fn c(cx: &App) -> &Theme {
    // 全局在启动时 install，未安装属于编程错误
    &cx.global::<UiTheme>().t
}

// ---------- 静态模式/方向存储（配置写档用，渲染走 UiTheme 全局） ----------

static MODE: AtomicU8 = AtomicU8::new(0);
pub const DARK_MODE: u8 = 0;
pub const LIGHT_MODE: u8 = 1;

pub fn set_mode(mode: u8) {
    MODE.store(mode, Ordering::Relaxed);
}

pub fn mode() -> u8 {
    MODE.load(Ordering::Relaxed)
}

pub fn is_light() -> bool {
    mode() == LIGHT_MODE
}

/// 当前主题配色（up/down 字段已按涨跌方向翻转：
/// 红涨绿跌模式下全应用所有 up/down 取色点统一生效，含K线/盘口/统计）
pub fn cur() -> Theme {
    let base = if is_light() { LIGHT } else { DARK };
    if PRICE_DIR.load(Ordering::Relaxed) == DIR_RED_UP {
        Theme {
            up: base.down,
            down: base.up,
            ..base
        }
    } else {
        base
    }
}

/// 涨跌颜色方向：红涨绿跌（A 股/币安默认）或绿涨红跌（美股习惯）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceDir {
    /// 绿涨红跌（币安默认：up=#0ECB81 涨，down=#F6465D 跌）
    GreenUp,
    /// 红涨绿跌
    RedUp,
}

impl PriceDir {
    pub fn from_cfg(s: &str) -> Self {
        if s == "red_up" {
            PriceDir::RedUp
        } else {
            PriceDir::GreenUp
        }
    }

    pub fn as_cfg(self) -> &'static str {
        match self {
            PriceDir::GreenUp => "green_up",
            PriceDir::RedUp => "red_up",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PriceDir::GreenUp => crate::i18n::tr("绿涨红跌"),
            PriceDir::RedUp => crate::i18n::tr("红涨绿跌"),
        }
    }
}

static PRICE_DIR: AtomicU8 = AtomicU8::new(0);
pub const DIR_GREEN_UP: u8 = 0;
pub const DIR_RED_UP: u8 = 1;

/// 由配置字符串设置涨跌方向
pub fn set_price_dir_str(s: &str) {
    let v = if s == "red_up" {
        DIR_RED_UP
    } else {
        DIR_GREEN_UP
    };
    PRICE_DIR.store(v, Ordering::Relaxed);
}

/// 由配置字符串设置主题模式
pub fn set_mode_str(s: &str) {
    set_mode(if s == "light" { LIGHT_MODE } else { DARK_MODE });
}

/// 上涨色（cur() 已按 price_dir 翻转字段，直读即可）
pub fn up_color() -> Rgba {
    cur().up
}

/// 下跌色（cur() 已按 price_dir 翻转字段，直读即可）
pub fn down_color() -> Rgba {
    cur().down
}

/// 由涨跌幅取颜色：>0 上涨色，<0 下跌色，0 中性。
pub fn signed_color(d: f64) -> Rgba {
    if d > 0.0 {
        up_color()
    } else if d < 0.0 {
        down_color()
    } else {
        cur().text_dim
    }
}

/// 逐笔成交主动方向着色：`buyer_maker=true` 记为跌（红），
/// `false` 记为涨（绿），同样随 price_dir 翻转
pub fn trade_color(buyer_maker: bool) -> Rgba {
    if buyer_maker {
        down_color()
    } else {
        up_color()
    }
}

/// MA 均线配色：浅色主题用加深变体（原色在近白图表背景上对比度不足 3:1）。
/// 色域约束：琥珀 / 青 / 蓝 / 灰蓝四轴——不使用粉、紫（设计规范明确禁止），
/// 也不复用涨跌语义色，避免均线与 K 线方向暗示混淆
pub fn ma_colors() -> [Rgba; 4] {
    if is_light() {
        [
            rgb_const(0xb27a00), // 琥珀（加深）
            rgb_const(0x0074ac), // 青（加深）
            rgb_const(0x1a5fd6), // 蓝（加深）
            rgb_const(0x5a6982), // 灰蓝（加深）
        ]
    } else {
        [
            rgb_const(0xffc13d),
            rgb_const(0x50c8ff),
            rgb_const(0x6096ff),
            rgb_const(0x94a3b8),
        ]
    }
}

// ---------- 颜色工具 ----------

/// 同色减透明度：对应 egui gamma_multiply(α) 在本应用的用法
/// （半透明叠加层/淡填充）。GPUI 中 Hsla/Rgba 的 opacity() 同义
pub fn faded(c: Rgba, a: f32) -> Hsla {
    Hsla::from(c).opacity(a.clamp(0.0, 1.0))
}

/// 线性插值两色（flash 衰减/残影淡出用）：t=0 返回 from，t=1 返回 to
pub fn lerp_color(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    }
}

// ---------- 字体加载 ----------

/// 从 Windows 字体目录按优先级加载第一个存在的字体文件字节
fn load_font_bytes(candidates: &[&str]) -> Option<Vec<u8>> {
    for name in candidates {
        let path = std::path::PathBuf::from("C:\\Windows\\Fonts").join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            tracing::info!("load font: {}", path.display());
            return Some(bytes);
        }
    }
    None
}

/// 注入 Windows 系统字体到 GPUI 文本系统。
/// GPUI 无字体族栈机制，文字渲染时的中英混排回退由其内部按字形覆盖自动处理
/// （cosmic-text 后端按 fallback 链找字形）；这里把两套字体都注册进系统，
/// 组件侧正文用 "Microsoft YaHei"、代码/数字列用 "Cascadia Code" 显式指定族名。
/// 失败（如非 Windows 环境）仅记日志，GPUI 会退回内置字体
pub fn apply_fonts(cx: &App) {
    let ts = cx.text_system();
    let mut fonts = Vec::new();
    if let Some(b) = load_font_bytes(&[
        "CascadiaCode.ttf",
        "CascadiaMono.ttf",
        "consola.ttf",
        "lucon.ttf",
    ]) {
        fonts.push(std::borrow::Cow::Owned(b));
    }
    if let Some(b) = load_font_bytes(&[
        "msyh.ttc",
        "msyh.ttf",
        "msyhbd.ttc",
        "simhei.ttf",
        "Deng.ttf",
    ]) {
        fonts.push(std::borrow::Cow::Owned(b));
    }
    if fonts.is_empty() {
        tracing::warn!("未找到系统字体，回退 GPUI 内置字体");
        return;
    }
    if let Err(e) = ts.add_fonts(fonts) {
        tracing::error!("字体注册失败: {e}");
    }
}

/// 正文（中英混排）字体族名
pub const FONT_UI: &str = "Microsoft YaHei";
/// 等宽数字/代码字体族名（表格价格列、交易对代码等）
pub const FONT_MONO: &str = "Cascadia Code";

// ---------- 测试 ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_scale_tokens_track_font_step() {
        let _g = font_lock();
        set_font_size_str("small");
        assert_eq!(font_body(), px(14.0));
        set_font_size_str("large");
        assert_eq!(font_body(), px(14.0 * 1.3));
    }

    #[test]
    fn cur_flips_up_down_on_red_up() {
        let green = cur();
        set_price_dir_str("red_up");
        let red = cur();
        assert_eq!(green.up, red.down);
        assert_eq!(green.down, red.up);
        set_price_dir_str("green_up");
    }

    #[test]
    fn lerp_color_endpoints() {
        let a = rgb_const(0x000000);
        let b = rgb_const(0xffffff);
        assert_eq!(lerp_color(a, b, 0.0), a);
        assert_eq!(lerp_color(a, b, 1.0), b);
    }

    #[test]
    fn hsla_defaults_valid() {
        // faded() 经 Hsla 做透明度换算，保证该路径的构造可用（编译期 import 卫生）
        let _ = Hsla::from(rgb_const(0x808080));
        let f = faded(rgb_const(0x000000), 0.5);
        assert!((f.a - 0.5).abs() < 1e-3);
    }
}
