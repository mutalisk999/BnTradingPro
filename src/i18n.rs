//! 简易国际化：以中文原文为键，英文表提供翻译。
//!
//! 用法：`i18n::tr("下单")`。语言通过 `set_lang` 全局设置（App 启动/配置变更时同步），
//! 后台引擎线程的 toast 同样生效。缺键时回退中文原文。

use std::sync::atomic::{AtomicU8, Ordering};

pub const ZH: u8 = 0;
pub const EN: u8 = 1;

static LANG: AtomicU8 = AtomicU8::new(ZH);

pub fn set_lang(lang: u8) {
    LANG.store(lang, Ordering::Relaxed);
}

pub fn lang() -> u8 {
    LANG.load(Ordering::Relaxed)
}

/// 翻译：中文原文 → 当前语言文本
pub fn tr(zh: &'static str) -> &'static str {
    if lang() == ZH {
        return zh;
    }
    en_lookup(zh)
}

fn en_lookup(zh: &'static str) -> &'static str {
    match zh {
        // ---- 顶层导航 ----
        "现货交易" => "Spot",
        "期货交易" => "Futures",
        "Portfolio" => "Portfolio",
        "设置" => "Settings",
        "使用说明" => "Help",
        // ---- 顶栏/状态 ----
        "BnTradingPro" => "BnTradingPro",
        "实盘 · 真实资金" => "LIVE · Real Funds",
        "TESTNET" => "TESTNET",
        "REST" => "REST",
        "行情" => "Market",
        "账户" => "Account",
        "已连接" => "Connected",
        "连接中" => "Connecting",
        "未连接" => "Disconnected",
        "未配置密钥" => "No API Key",
        "获取行情中…" => "Loading market…",
        "错误: " => "Error: ",
        // ---- 页头 ----
        "搜索交易对（如 BTC）…" => "Search symbols (e.g. BTC)…",
        "无匹配交易对" => "No matching symbols",
        // ---- 盘口/成交 ----
        "盘口 20 档" => "Order Book (20)",
        "成交明细" => "Recent Trades",
        "加载中…" => "Loading…",
        "等待深度数据…" => "Waiting for depth…",
        "价差" => "Spread",
        "价格" => "Price",
        "数量" => "Qty",
        "时间" => "Time",
        // ---- 下单面板 ----
        "下单 · " => "Order · ",
        "未配置 API 密钥，仅可查看行情" => "No API key: view-only mode",
        "买入 / 开多" => "Buy / Long",
        "卖出 / 开空" => "Sell / Short",
        "买入" => "Buy",
        "卖出" => "Sell",
        "类型" => "Type",
        "限价" => "Limit",
        "市价" => "Market",
        "止损限价" => "Stop Limit",
        "止盈限价" => "Take-Profit Limit",
        "止损市价" => "Stop Market",
        "止盈市价" => "Take-Profit Market",
        "持仓方向" => "Position Side",
        "开多 (LONG)" => "Long (LONG)",
        "开空 (SHORT)" => "Short (SHORT)",
        "只减仓 (ReduceOnly)" => "Reduce-Only",
        "杠杆" => "Leverage",
                "触发价" => "Trigger",
        "步进" => "Step",
        "比例" => "Size",
        "按可用余额估算" => "Estimate from available balance",
        "预估金额" => "Est. Value",
        "价格将量化为" => "Price will be quantized to",
        "数量将量化为" => "Qty will be quantized to",
        "价格超出可表示范围" => "Price out of range",
        "数量超出可表示范围" => "Qty out of range",
        "小于最小下单数量" => "Below min order qty",
        "低于最小名义金额" => "Below min notional",
        "（限价单）" => " (limit order)",
        "提交中…" => "Submitting…",
        "买入开多" => "Buy / Long",
        "卖出开空" => "Sell / Short",
        // ---- 订单表格 ----
        "当前挂单" => "Open Orders",
        "持仓" => "Positions",
        "订单历史" => "Order History",
        "成交历史" => "Trade History",
        "笔挂单" => " open orders",
        "交易对" => "Symbol",
        "方向" => "Side",
        "已成交" => "Filled",
        "状态" => "Status",
        "操作" => "Action",
        "刷新" => "Refresh",
        "撤销" => "Cancel",
        "待成交" => "New",
        "部分成交" => "Partially Filled",
        "已撤销" => "Canceled",
        "撤销中" => "Canceling",
        "已拒绝" => "Rejected",
        "已过期" => "Expired",
        "匹配过期" => "Expired in Match",
        "其他" => "Other",
        "只减" => " RO",
        "触发" => "Trigger",
        "金额" => "Value",
        "手续费" => "Fee",
        "角色" => "Role",
        "maker" => "maker",
        "taker" => "taker",
        "市价平仓" => "Close",
        "平仓" => "Close",
        "全杠杆" => "Max Lev",
        "多" => "Long",
        "空" => "Short",
        "标记价" => "Mark",
        "开仓价" => "Entry",
        "未实现盈亏" => "Unrealized PnL",
        "强平价" => "Liq. Price",
        "最近 100 笔" => "Last 100 orders",
        "切换交易对后自动加载" => "Auto-loads after selecting a symbol",
        "暂无挂单" => "No open orders",
        "暂无持仓" => "No positions",
        "暂无订单历史" => "No order history",
        "暂无成交" => "No trades",
        "未配置 API 密钥" => "No API key configured",
        "最近" => "Last",
        "笔成交" => " trades",
        // ---- Portfolio ----
        "合约钱包余额" => "Futures Wallet",
        "保证金余额" => "Margin Balance",
        "可用余额" => "Available",
        "持仓模式：双向" => "Position Mode: Hedge",
        "持仓模式：单向" => "Position Mode: One-way",
        "切换" => "Toggle",
        "刷新账户" => "Refresh Account",
        "现货资产" => "Spot Assets",
        "合约资产" => "Futures Assets",
        "合约持仓" => "Futures Positions",
        "资产" => "Asset",
        "可用" => "Available",
        "冻结" => "Locked",
        "钱包余额" => "Wallet",
        "暂无非零资产" => "No non-zero assets",
        "（该 Key 无交易权限）" => " (key has no trade permission)",
        "未配置 API 密钥，无法查看资产（可在设置页配置）" => {
            "No API key configured — set one in Settings to view assets"
        }
        "合约账户加载中…" => "Loading futures account…",
        // ---- 设置 ----
        "交易环境" => "Environment",
        "测试网 (Testnet)" => "Testnet",
        "实盘" => "Live",
        "测试网密钥在 testnet.binance.vision / testnet.binancefuture.com 申请" => {
            "Get testnet keys at testnet.binance.vision / testnet.binancefuture.com"
        }
        "⚠ 实盘模式下下单将使用真实资金，请确认密钥与环境正确！" => {
            "⚠ Live mode places orders with REAL funds — verify your key and environment!"
        }
        "网络代理（REST 与 WebSocket 均生效）" => "Proxy (applies to REST & WebSocket)",
        "启用代理" => "Enable proxy",
        "地址" => "Address",
        "请求参数" => "Request Parameters",
        "HTTP 超时" => "HTTP Timeout",
        "日志级别" => "Log Level",
        "默认交易对" => "Default Symbol",
        "默认K线周期" => "Default Interval",
        "端点覆盖（留空使用内置默认）" => "Endpoint Overrides (blank = built-in defaults)",
        "现货 REST" => "Spot REST",
        "现货 WS" => "Spot WS",
        "合约 REST" => "Futures REST",
        "合约 WS" => "Futures WS",
        "保存并重连" => "Save & Reconnect",
        "API 密钥" => "API Key",
        "已配置" => "Configured",
        "未配置（仅公开行情）" => "Not configured (public data only)",
        "更换密钥" => "Change Key",
        "配置密钥" => "Configure Key",
        "删除凭据库" => "Delete Vault",
        "关于" => "About",
        "语言" => "Language",
        "主题" => "Theme",
        "深色" => "Dark",
        "浅色" => "Light",
        "中文" => "中文",
        "English" => "English",
        "设置已保存，正在重连…" => "Settings saved, reconnecting…",
        "已删除本机凭据库" => "Local vault deleted",
        // ---- 登录页 ----
        "Binance 现货 / U本位合约 交易终端" => "Binance Spot / USDT-M Futures Trading Terminal",
        "解锁" => "Unlock",
        "设置主密码" => "Set Master Password",
        "配置 API 密钥" => "Configure API Key",
        "第" => "Step",
        "/2 步" => "/2",
        "主密码用于加密本机保存的 API Key/Secret（Argon2id + AES-256-GCM）。忘记将无法恢复，需删除凭据库重新配置。" => {
            "The master password encrypts your API Key/Secret locally (Argon2id + AES-256-GCM). It cannot be recovered if lost — you would need to reset the vault."
        }
        "主密码（至少 8 位）" => "Master password (min 8 chars)",
        "再输一次" => "Confirm password",
        "继续" => "Continue",
        "主密码" => "Master password",
        "主密码（用于加密保存）" => "Master password (for encryption)",
        "输入主密码以解密本机保存的 API 密钥" => "Enter the master password to decrypt stored API keys",
        "重置凭据库" => "Reset Vault",
        "删除本机加密凭据库，重新开始设置" => "Delete the local vault and start over",
        "仅看行情，暂不解锁" => "Browse without unlocking",
        "当前环境：测试网（testnet.binance.vision / testnet.binancefuture.com 申请）" => {
            "Environment: Testnet (keys from testnet.binance.vision / testnet.binancefuture.com)"
        }
        "当前环境：实盘（binance.com → API 管理，建议绑定 IP 白名单并只开启现货/合约交易权限）" => {
            "Environment: LIVE (binance.com → API Management; bind IP whitelist, enable trading only)"
        }
        "API Key" => "API Key",
        "API Secret" => "API Secret",
        "保存并连接" => "Save & Connect",
        "跳过，仅看行情" => "Skip (view-only)",
        "跳过后仍可查看实时行情与 K 线，随时可在设置页配置密钥" => {
            "You can browse live market data without a key; configure one anytime in Settings"
        }
        "请输入主密码" => "Please enter the master password",
        "主密码错误" => "Wrong master password",
        "主密码错误，或凭据文件损坏" => "Wrong master password or corrupted vault",
        "主密码至少 8 位" => "Master password must be at least 8 characters",
        "两次输入不一致" => "Passwords do not match",
        "已解锁，开始同步账户数据" => "Unlocked — syncing account data",
        "密钥已加密保存并连接" => "Key saved (encrypted) and connected",
        // ---- 确认弹窗 ----
        "确认下单" => "Confirm Order",
        "【测试网】" => "[Testnet] ",
        "【实盘】" => "[LIVE] ",
        "确认 · 真实资金！" => " confirm · REAL FUNDS!",
        "确认提交" => "Confirm",
        "取消" => "Cancel",
        "只减仓" => "Reduce-Only",
        "委托价" => "Limit Price",
        "是" => "Yes",
        // ---- 图表 ----
        "加载行情中…" => "Loading market data…",
        "暂无K线数据（等待加载或检查网络/代理）" => "No candles yet (waiting or check network/proxy)",
        "24h 高" => "24h High",
        "24h 低" => "24h Low",
        "· 额" => " · Vol",
        "资金费率" => "Funding",
        "挂单 " => "Order ",
        "触发 " => "Trigger ",
        "持仓开仓 " => "Position entry ",
        "买" => "Buy",
        "卖" => "Sell",
        "滚轮缩放 · 拖动平移 · 双击回最新" => "Scroll to zoom · Drag to pan · Double-click to reset",
        // ---- 引擎 toast ----
        "请求超时，请检查网络/代理" => "Request timed out — check network/proxy",
        "未配置 API 密钥，请在设置中配置" => "No API key — configure one in Settings",
        "网络错误: " => "Network error: ",
        "响应解析失败: " => "Failed to parse response: ",
        "触发限速保护，稍后自动恢复" => "Rate-limit protection engaged",
        "代理配置无效: " => "Invalid proxy config: ",
        "已撤销委托 #" => "Order canceled #",
        "已撤销 " => "Canceled all orders for ",
        " 全部挂单" => "",
        "委托已提交 (#" => " order submitted (#",
        "已设为" => " set to ",
        "杠杆已设为" => "leverage set to ",
        "持仓模式已切换" => "Position mode switched",
        "连接 Binance 失败: " => "Binance connection failed: ",
        "获取现货交易规则失败: " => "Failed to load spot rules: ",
        "获取合约交易规则失败: " => "Failed to load futures rules: ",
        "现货账户: " => "Spot account: ",
        "合约账户: " => "Futures account: ",
        "加载K线失败: " => "Failed to load candles: ",
        "下单被拒绝" => "Order rejected",
        "价格已按最小变动量化" => "price quantized",
        "数量已按步进量化" => "qty quantized",
        "价格/数量超出可表示范围，请检查输入" => "Price/qty out of representable range",
        "数量过小（步进 " => "Qty too small (step ",
        "缺少 " => "Missing rules for ",
        " 交易规则（等待行情初始化）" => " (waiting for market init)",
        "已实现盈亏: " => "Realized PnL: ",
        // ---- 交易对属性 ----
        "交易对属性" => "Symbol Info",
        "基础资产" => "Base Asset",
        "计价资产" => "Quote Asset",
        "合约类型" => "Contract Type",
        "保证金资产" => "Margin Asset",
        "价格精度" => "Price Precision",
        "数量精度" => "Qty Precision",
        "最小变动" => "Tick Size",
        "最小数量" => "Min Qty",
        "最大数量" => "Max Qty",
        "最小名义金额" => "Min Notional",
        "支持订单类型" => "Order Types",
        "权限" => "Permissions",
        "上架时间" => "Listed",
        "标的类型" => "Underlying",
        _ => return_en_fallback(zh),
    }
}

/// 带动态后缀的兜底（如 "96,000.12" 之类不会走到这里；数字拼接串直接原样返回）
fn return_en_fallback(zh: &'static str) -> &'static str {
    zh
}
