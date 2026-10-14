//! 图表副图技术指标计算：输入 K 线（Decimal），输出 f64 序列。
//!
//! 全部为纯函数；结果由 chart.rs 按 (市场, 交易对, 周期, kline_version, 指标) 缓存，
//! 仅在数据/指标变化时重算一次。暖机期（窗口不足）的值为 NaN，绘制时跳过。

use crate::binance::model::Candle;
use crate::ui::widgets::f64_decimal;

/// 副图指标（标签中英文一致，纯 ASCII，不需要 i18n）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubIndicator {
    Vol,
    Macd,
    Rsi,
    Kdj,
    Boll,
    Cci,
    Wr,
    Atr,
    Adx,
    Sar,
    Obv,
    Mfi,
    Stoch,
}

impl SubIndicator {
    pub const ALL: [SubIndicator; 13] = [
        SubIndicator::Vol,
        SubIndicator::Macd,
        SubIndicator::Rsi,
        SubIndicator::Kdj,
        SubIndicator::Boll,
        SubIndicator::Cci,
        SubIndicator::Wr,
        SubIndicator::Atr,
        SubIndicator::Adx,
        SubIndicator::Sar,
        SubIndicator::Obv,
        SubIndicator::Mfi,
        SubIndicator::Stoch,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            SubIndicator::Vol => "VOL",
            SubIndicator::Macd => "MACD",
            SubIndicator::Rsi => "RSI",
            SubIndicator::Kdj => "KDJ",
            SubIndicator::Boll => "BOLL",
            SubIndicator::Cci => "CCI",
            SubIndicator::Wr => "WR",
            SubIndicator::Atr => "ATR",
            SubIndicator::Adx => "ADX",
            SubIndicator::Sar => "SAR",
            SubIndicator::Obv => "OBV",
            SubIndicator::Mfi => "MFI",
            SubIndicator::Stoch => "STOCH",
        }
    }
}

/// 序列颜色角色：缓存里只存角色，绘制时经 theme 解析为当前主题色
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Accent,
    Warn,
    Up,
    Down,
    Dim,
}

/// 副图序列：若干条线 + 可选柱（MACD HIST）+ 可选点（SAR）+ 参考线（RSI 30/70）。
/// 所有序列与 K 线等长对齐（下标即 K 线下标），暖机期为 NaN。
#[derive(Debug, Clone, Default)]
pub struct SubSeries {
    pub lines: Vec<(&'static str, Role, Vec<f64>)>,
    /// 按正负着色的柱（MACD HIST）
    pub bars: Option<Vec<f64>>,
    /// 上升段的 SAR 点（NaN 表示该处无点）
    pub dots_up: Option<Vec<f64>>,
    /// 下降段的 SAR 点
    pub dots_down: Option<Vec<f64>>,
    /// 水平虚线参考值（RSI 30/70）
    pub guides: Vec<f64>,
    /// y 范围是否强制包含 0（MACD/OBV 等围绕 0 摆动的序列）
    pub zero_based: bool,
}

/// 计算副图指标（Vol 由 chart.rs 走原生成交量路径，不进入这里）
pub fn compute(ind: SubIndicator, candles: &[Candle]) -> SubSeries {
    let n = candles.len();
    let mut o = Vec::with_capacity(n);
    let mut h = Vec::with_capacity(n);
    let mut l = Vec::with_capacity(n);
    let mut c = Vec::with_capacity(n);
    let mut v = Vec::with_capacity(n);
    for k in candles {
        o.push(f64_decimal(&k.o));
        h.push(f64_decimal(&k.h));
        l.push(f64_decimal(&k.l));
        c.push(f64_decimal(&k.c));
        v.push(f64_decimal(&k.v));
    }
    match ind {
        SubIndicator::Vol => SubSeries::default(),
        SubIndicator::Macd => macd(&c),
        SubIndicator::Rsi => rsi(&c, 14),
        SubIndicator::Kdj => kdj(&h, &l, &c),
        SubIndicator::Boll => boll(&c, 20, 2.0),
        SubIndicator::Cci => cci(&h, &l, &c, 20),
        SubIndicator::Wr => wr(&h, &l, &c, 14),
        SubIndicator::Atr => atr(&h, &l, &c, 14),
        SubIndicator::Adx => adx(&h, &l, &c, 14),
        SubIndicator::Sar => sar(&h, &l),
        SubIndicator::Obv => obv(&c, &v),
        SubIndicator::Mfi => mfi(&h, &l, &c, &v, 14),
        SubIndicator::Stoch => stoch(&h, &l, &c, 14, 3),
    }
}

// ---------- 基础序列 ----------

/// EMA：种子 = 首值，ema[i] = v[i]*k + ema[i-1]*(1-k)，k = 2/(n+1)
fn ema(v: &[f64], n: usize) -> Vec<f64> {
    let k = 2.0 / (n as f64 + 1.0);
    let mut out = Vec::with_capacity(v.len());
    let mut prev = match v.first() {
        Some(x) => *x,
        None => return out,
    };
    out.push(prev);
    for &x in &v[1..] {
        prev = x * k + prev * (1.0 - k);
        out.push(prev);
    }
    out
}

/// 滑动窗口 SMA：窗口不足为 NaN
fn sma(v: &[f64], n: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; v.len()];
    if v.len() < n || n == 0 {
        return out;
    }
    let mut sum: f64 = v[..n].iter().sum();
    out[n - 1] = sum / n as f64;
    for i in n..v.len() {
        sum += v[i] - v[i - n];
        out[i] = sum / n as f64;
    }
    out
}

/// Wilder 平滑（RSI/ATR/ADX 用）：种子 = 前 n 个的均值，
/// 之后 out[i] = out[i-1] + (v[i] - out[i-1]) / n
fn wilder(v: &[f64], n: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; v.len()];
    if v.len() < n || n == 0 {
        return out;
    }
    let mut s: f64 = v[..n].iter().sum();
    s /= n as f64;
    out[n - 1] = s;
    let nf = n as f64;
    for i in n..v.len() {
        s += (v[i] - s) / nf;
        out[i] = s;
    }
    out
}

/// 窗口内最高/最低（含 i，窗口 n），i < n-1 时为 NaN
fn rolling_max(v: &[f64], n: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; v.len()];
    for i in n.saturating_sub(1)..v.len() {
        out[i] = v[i + 1 - n..=i].iter().cloned().fold(f64::MIN, f64::max);
    }
    out
}

fn rolling_min(v: &[f64], n: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; v.len()];
    for i in n.saturating_sub(1)..v.len() {
        out[i] = v[i + 1 - n..=i].iter().cloned().fold(f64::MAX, f64::min);
    }
    out
}

// ---------- 各指标 ----------

/// MACD(12,26,9)：DIF=EMA12-EMA26；DEA=EMA9(DIF)；HIST=2*(DIF-DEA)
fn macd(c: &[f64]) -> SubSeries {
    let e12 = ema(c, 12);
    let e26 = ema(c, 26);
    let dif: Vec<f64> = e12.iter().zip(&e26).map(|(&a, &b)| a - b).collect();
    let dea = ema(&dif, 9);
    let hist: Vec<f64> = dif.iter().zip(&dea).map(|(&a, &b)| 2.0 * (a - b)).collect();
    SubSeries {
        lines: vec![("DIF", Role::Accent, dif), ("DEA", Role::Warn, dea)],
        bars: Some(hist),
        zero_based: true,
        ..Default::default()
    }
}

/// RSI(n)：Wilder 平滑；单边行情（无亏损）按惯例取 100
fn rsi(c: &[f64], n: usize) -> SubSeries {
    let mut out = vec![f64::NAN; c.len()];
    if c.len() > n {
        let (mut ag, mut al) = (0.0f64, 0.0f64);
        for i in 1..=n {
            let d = c[i] - c[i - 1];
            if d > 0.0 {
                ag += d;
            } else {
                al -= d;
            }
        }
        ag /= n as f64;
        al /= n as f64;
        out[n] = if al == 0.0 {
            100.0
        } else {
            100.0 - 100.0 / (1.0 + ag / al)
        };
        let nf = n as f64;
        for i in n + 1..c.len() {
            let d = c[i] - c[i - 1];
            ag = (ag * (nf - 1.0) + d.max(0.0)) / nf;
            al = (al * (nf - 1.0) + (-d).max(0.0)) / nf;
            out[i] = if al == 0.0 {
                100.0
            } else {
                100.0 - 100.0 / (1.0 + ag / al)
            };
        }
    }
    SubSeries {
        lines: vec![("RSI", Role::Accent, out)],
        guides: vec![30.0, 70.0],
        ..Default::default()
    }
}

/// KDJ(9,3,3)：RSV=(C-L9)/(H9-L9)*100（H9==L9 → 50）；K=2K'/3+RSV/3；D=2D'/3+K/3；J=3K-2D
fn kdj(h: &[f64], l: &[f64], c: &[f64]) -> SubSeries {
    let n = 9;
    let (hh, ll) = (rolling_max(h, n), rolling_min(l, n));
    let mut k = vec![f64::NAN; c.len()];
    let mut d = vec![f64::NAN; c.len()];
    let mut j = vec![f64::NAN; c.len()];
    let (mut kv, mut dv) = (50.0f64, 50.0f64);
    for i in 0..c.len() {
        if hh[i].is_nan() {
            continue;
        }
        let range = hh[i] - ll[i];
        let rsv = if range == 0.0 {
            50.0
        } else {
            (c[i] - ll[i]) / range * 100.0
        };
        kv = kv * 2.0 / 3.0 + rsv / 3.0;
        dv = dv * 2.0 / 3.0 + kv / 3.0;
        k[i] = kv;
        d[i] = dv;
        j[i] = 3.0 * kv - 2.0 * dv;
    }
    SubSeries {
        lines: vec![
            ("K", Role::Accent, k),
            ("D", Role::Warn, d),
            ("J", Role::Up, j),
        ],
        ..Default::default()
    }
}

/// BOLL(20,2)：MID=SMA20；MD=窗口内总体标准差；UP=MID+2MD；LOW=MID-2MD
fn boll(c: &[f64], n: usize, mult: f64) -> SubSeries {
    let mid = sma(c, n);
    let mut up = vec![f64::NAN; c.len()];
    let mut low = vec![f64::NAN; c.len()];
    for i in n.saturating_sub(1)..c.len() {
        let m = mid[i];
        if m.is_nan() {
            continue;
        }
        let mut var = 0.0;
        for &x in &c[i + 1 - n..=i] {
            var += (x - m) * (x - m);
        }
        let md = (var / n as f64).sqrt();
        up[i] = m + mult * md;
        low[i] = m - mult * md;
    }
    SubSeries {
        lines: vec![
            ("MID", Role::Dim, mid),
            ("UP", Role::Up, up),
            ("LOW", Role::Down, low),
        ],
        ..Default::default()
    }
}

/// CCI(20)：TP=(H+L+C)/3；CCI=(TP-SMA20(TP))/(0.015*MeanDev20(TP))
fn cci(h: &[f64], l: &[f64], c: &[f64], n: usize) -> SubSeries {
    let tp: Vec<f64> = (0..c.len()).map(|i| (h[i] + l[i] + c[i]) / 3.0).collect();
    let avg = sma(&tp, n);
    let mut out = vec![f64::NAN; c.len()];
    for i in n.saturating_sub(1)..c.len() {
        let m = avg[i];
        if m.is_nan() {
            continue;
        }
        let mut md = 0.0;
        for &x in &tp[i + 1 - n..=i] {
            md += (x - m).abs();
        }
        md /= n as f64;
        out[i] = if md == 0.0 {
            0.0
        } else {
            (tp[i] - m) / (0.015 * md)
        };
    }
    SubSeries {
        lines: vec![("CCI", Role::Accent, out)],
        ..Default::default()
    }
}

/// WR(14)：WR=(H14-C)/(H14-L14)*100（区间为 0 → 50）
fn wr(h: &[f64], l: &[f64], c: &[f64], n: usize) -> SubSeries {
    let (hh, ll) = (rolling_max(h, n), rolling_min(l, n));
    let out: Vec<f64> = (0..c.len())
        .map(|i| {
            if hh[i].is_nan() {
                f64::NAN
            } else {
                let range = hh[i] - ll[i];
                if range == 0.0 {
                    50.0
                } else {
                    (hh[i] - c[i]) / range * 100.0
                }
            }
        })
        .collect();
    SubSeries {
        lines: vec![("WR", Role::Warn, out)],
        ..Default::default()
    }
}

/// ATR(14)：TR=max(H-L, |H-prevC|, |L-prevC|)（首根 H-L），Wilder 平滑
fn atr(h: &[f64], l: &[f64], c: &[f64], n: usize) -> SubSeries {
    let mut tr = Vec::with_capacity(c.len());
    for i in 0..c.len() {
        if i == 0 {
            tr.push(h[i] - l[i]);
        } else {
            let pc = c[i - 1];
            tr.push((h[i] - l[i]).max((h[i] - pc).abs()).max((l[i] - pc).abs()));
        }
    }
    SubSeries {
        lines: vec![("ATR", Role::Accent, wilder(&tr, n))],
        ..Default::default()
    }
}

/// ADX(14)：+DM/-DM/TR 做 Wilder 平滑 → +DI/-DI → DX → ADX（再平滑 14）
fn adx(h: &[f64], l: &[f64], c: &[f64], n: usize) -> SubSeries {
    let len = c.len();
    let mut pdm = vec![0.0f64; len];
    let mut mdm = vec![0.0f64; len];
    let mut tr = vec![0.0f64; len];
    for i in 1..len {
        let up = h[i] - h[i - 1];
        let dn = l[i - 1] - l[i];
        pdm[i] = if up > dn && up > 0.0 { up } else { 0.0 };
        mdm[i] = if dn > up && dn > 0.0 { dn } else { 0.0 };
        let pc = c[i - 1];
        tr[i] = (h[i] - l[i]).max((h[i] - pc).abs()).max((l[i] - pc).abs());
    }
    let sp = wilder(&pdm, n);
    let sm = wilder(&mdm, n);
    let st = wilder(&tr, n);
    let mut pdi = vec![f64::NAN; len];
    let mut mdi = vec![f64::NAN; len];
    let mut dx = vec![f64::NAN; len];
    for i in 0..len {
        if sp[i].is_nan() || st[i].is_nan() || st[i] == 0.0 {
            continue;
        }
        pdi[i] = 100.0 * sp[i] / st[i];
        mdi[i] = 100.0 * sm[i] / st[i];
        let denom = pdi[i] + mdi[i];
        dx[i] = if denom == 0.0 {
            0.0
        } else {
            100.0 * (pdi[i] - mdi[i]).abs() / denom
        };
    }
    // ADX：对 DX 再做一次 Wilder 平滑（wilder 需要 n 个有效值，DX 从 n-1 起有效）
    let adx_v = wilder(&dx, n);
    SubSeries {
        lines: vec![
            ("+DI", Role::Up, pdi),
            ("-DI", Role::Down, mdi),
            ("ADX", Role::Accent, adx_v),
        ],
        ..Default::default()
    }
}

/// SAR：标准抛物线（af 步进 0.02，上限 0.2），趋势反转时重置；按趋势拆成两组点。
/// 初始趋势默认上升（sar=首根最低价，ep=首根最高价），由穿透逻辑自行翻转。
fn sar(h: &[f64], l: &[f64]) -> SubSeries {
    let len = h.len();
    let mut up_arr = vec![f64::NAN; len];
    let mut dn_arr = vec![f64::NAN; len];
    if len == 0 {
        return SubSeries {
            dots_up: Some(up_arr),
            dots_down: Some(dn_arr),
            ..Default::default()
        };
    }
    let mut is_up = true;
    let mut sar_v = if is_up { l[0] } else { h[0] };
    let mut ep = if is_up { h[0] } else { l[0] };
    let mut af = 0.02f64;
    for i in 0..len {
        if i > 0 {
            sar_v += af * (ep - sar_v);
            if is_up {
                // SAR 不得高于前两根最低价
                sar_v = sar_v.min(l[i - 1]);
                if i >= 2 {
                    sar_v = sar_v.min(l[i - 2]);
                }
                if h[i] > ep {
                    ep = h[i];
                    af = (af + 0.02).min(0.2);
                }
                if l[i] < sar_v {
                    is_up = false;
                    sar_v = ep;
                    ep = l[i];
                    af = 0.02;
                }
            } else {
                sar_v = sar_v.max(h[i - 1]);
                if i >= 2 {
                    sar_v = sar_v.max(h[i - 2]);
                }
                if l[i] < ep {
                    ep = l[i];
                    af = (af + 0.02).min(0.2);
                }
                if h[i] > sar_v {
                    is_up = true;
                    sar_v = ep;
                    ep = h[i];
                    af = 0.02;
                }
            }
        }
        if is_up {
            up_arr[i] = sar_v;
        } else {
            dn_arr[i] = sar_v;
        }
    }
    SubSeries {
        dots_up: Some(up_arr),
        dots_down: Some(dn_arr),
        ..Default::default()
    }
}

/// OBV：cumulative Σ v * sign(c - prevC)（首根计 0）
fn obv(c: &[f64], v: &[f64]) -> SubSeries {
    let mut out = Vec::with_capacity(c.len());
    let mut acc = 0.0f64;
    for i in 0..c.len() {
        if i > 0 {
            let d = c[i] - c[i - 1];
            if d > 0.0 {
                acc += v[i];
            } else if d < 0.0 {
                acc -= v[i];
            }
        }
        out.push(acc);
    }
    SubSeries {
        lines: vec![("OBV", Role::Accent, out)],
        zero_based: true,
        ..Default::default()
    }
}

/// MFI(14)：TP 资金流；neg==0 → 100
fn mfi(h: &[f64], l: &[f64], c: &[f64], v: &[f64], n: usize) -> SubSeries {
    let tp: Vec<f64> = (0..c.len()).map(|i| (h[i] + l[i] + c[i]) / 3.0).collect();
    let mut out = vec![f64::NAN; c.len()];
    if c.len() <= n {
        return SubSeries {
            lines: vec![("MFI", Role::Accent, out)],
            ..Default::default()
        };
    }
    for (i, _) in c.iter().enumerate().take(c.len()).skip(n) {
        let (mut pos, mut neg) = (0.0f64, 0.0f64);
        for j in i + 1 - n..=i {
            let d = tp[j] - tp[j - 1];
            if d > 0.0 {
                pos += tp[j] * v[j];
            } else if d < 0.0 {
                neg += tp[j] * v[j];
            }
        }
        out[i] = if neg == 0.0 {
            100.0
        } else {
            100.0 - 100.0 / (1.0 + pos / neg)
        };
    }
    SubSeries {
        lines: vec![("MFI", Role::Accent, out)],
        ..Default::default()
    }
}

/// Stoch(14,3)：rawK=(C-L14)/(H14-L14)*100；K=SMA3(rawK)；D=SMA3(K)
fn stoch(h: &[f64], l: &[f64], c: &[f64], n: usize, m: usize) -> SubSeries {
    let (hh, ll) = (rolling_max(h, n), rolling_min(l, n));
    let mut raw = vec![f64::NAN; c.len()];
    for i in 0..c.len() {
        if hh[i].is_nan() {
            continue;
        }
        let range = hh[i] - ll[i];
        raw[i] = if range == 0.0 {
            50.0
        } else {
            (c[i] - ll[i]) / range * 100.0
        };
    }
    let k = sma_skip_nan(&raw, m);
    let d = sma_skip_nan(&k, m);
    SubSeries {
        lines: vec![("K", Role::Accent, k), ("D", Role::Warn, d)],
        ..Default::default()
    }
}

/// 对可能含前缀 NaN 的序列做 SMA：任一窗口成员为 NaN → 结果 NaN
fn sma_skip_nan(v: &[f64], n: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; v.len()];
    if n == 0 {
        return out;
    }
    for i in n.saturating_sub(1)..v.len() {
        let w = &v[i + 1 - n..=i];
        if w.iter().any(|x| x.is_nan()) {
            continue;
        }
        out[i] = w.iter().sum::<f64>() / n as f64;
    }
    out
}
