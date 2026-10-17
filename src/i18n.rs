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
        "资产总览" => "Portfolio",
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
        "错误: " => "Error: ",
        "引擎已断开，命令未提交" => "Engine disconnected — command not sent",
        "配置保存失败: " => "Failed to save config: ",
        // ---- 页头 ----
        "搜索交易对…" => "Search symbols…",
        "无匹配交易对" => "No matching symbols",
        "自选" => "Favorites",
        "全部" => "All",
        // 订单历史筛选：基础单来自 allOrders，条件单来自 allAlgoOrders
        "基础单" => "Regular",
        "条件单" => "Conditional",
        "当前筛选无订单" => "No orders match this filter",
        "切换筛选查看其它订单" => "Try another filter to see other orders",
        // 预警条目的市场后缀（侧栏/预警弹窗共用）
        "· 合约" => " · Futures",
        "加入自选" => "Add to favorites",
        "取消自选" => "Remove from favorites",
        "REST 接口正常" => "REST OK",
        "REST 接口异常" => "REST Error",
        "REST 客户端初始化失败，请检查网络/代理设置" => "REST client init failed — check network/proxy settings",
        "后台任务异常退出: " => "Background task exited: ",
        "命令处理任务已停止，请重启应用" => "Command loop stopped — please restart the app",
        "任务反复退出，请重启应用: " => "Task keeps exiting, please restart the app: ",
        "挂单获取失败" => "Failed to load open orders",
        "读超时（连接无数据）" => "Read timeout (no data)",
        "点击填入下单价格" => "Click to fill the order price",
        "确认" => "Confirm",
        "确认删除凭据库？" => "Delete the vault?",
        "删除凭据库失败" => "Failed to delete vault",
        "自定义端点已启用" => "Custom endpoints active",
        "账户数据加载中" => "Account data loading…",
        "可用余额为 0" => "Available balance is 0",
        "数量过小" => "Qty too small",
        "主密码用于加密本机保存的 API Key/Secret（Argon2id + AES-256-GCM）。忘记将无法恢复。" => {
            "The master password encrypts your API Key/Secret locally (Argon2id + AES-256-GCM). It cannot be recovered if lost."
        }
        // ---- 盘口/成交 ----
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
        "设定" => "Set",
        "杠杆倍数" => "Leverage",
        "杠杆倍数需为 1-125 的整数" => "Leverage must be an integer from 1 to 125",
        "交易规则尚未加载" => "Trading rules not loaded yet",
        "请输入数量" => "Enter quantity",
        "请输入价格" => "Enter price",
        "请输入触发价" => "Enter trigger price",
        "价格将量化为" => "Price will be quantized to",
        "数量将量化为" => "Qty will be quantized to",
        "价格超出可表示范围" => "Price out of range",
        "数量超出可表示范围" => "Qty out of range",
        "小于最小下单数量" => "Below min order qty",
        "低于最小名义金额" => "Below min notional",
        "（限价单）" => " (limit order)",
        "提交中…" => "Submitting…",
        "本地时间与服务器偏差过大，已自动校时并重试，请重试" => {
            "Clock drift too large — time resynced and request retried, please try again"
        }
        "IP 已被限频封禁（418），请降低请求频率后重试" => {
            "IP banned by rate limiting (418) — reduce request frequency and retry later"
        }
        "请求频率超限（429），请稍后重试" => "Rate limit exceeded (429) — retry later",
        "API Key 无效或权限不足（检查 Key、IP 白名单、是否用对测试网/实盘）" => {
            "API key invalid or unauthorized (check the key, IP whitelist, testnet vs live)"
        }
        "API Key 格式错误" => "Malformed API key",
        "签名错误（Secret 不匹配）" => "Signature error (wrong secret)",
        "下单参数不满足过滤器（价格/数量/最小名义金额）" => "Order fails filters (price/qty/min notional)",
        "下单被拒绝: " => "Order rejected: ",
        "保证金不足" => "Insufficient margin",
        "订单会立即触发（止盈止损价过于贴近市价）" => "Order would trigger immediately (stop too close to market)",
        "订单方向与持仓方向冲突（ReduceOnly）" => "Side conflicts with position (ReduceOnly)",
        "持仓方向与订单冲突（检查双向/单向持仓模式）" => "Position side conflict (check hedge/one-way mode)",
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
        "撤销全部" => "Cancel All",
        // 撤销的两步确认标签（点击一次进入待确认态后显示）
        "确认撤销" => "Confirm Cancel",
        "再按一次 Ctrl+Shift+C 确认撤销: " => "Press Ctrl+Shift+C again to confirm: ",
        "U本位合约" => "USDT-M Futures",
        "现货" => "Spot",
        "个持仓" => " positions",
        "切换单向持仓" => "Switch to One-way",
        "切换双向持仓" => "Switch to Hedge",
        "待成交" => "New",
        "已触发" => "Triggered",
        "部分成交" => "Partially Filled",
        "已撤销" => "Canceled",
        "撤销中" => "Canceling",
        "已拒绝" => "Rejected",
        "已过期" => "Expired",
        "匹配过期" => "Expired in Match",
        "其他" => "Other",
        "只减" => " RO",
        "触发" => "Trigger",
        "整仓" => "Close-All",
        "金额" => "Value",
        "手续费" => "Fee",
        "角色" => "Role",
        "maker" => "maker",
        "taker" => "taker",
        "市价平仓" => "Close",
        "平仓" => "Close",
        "多" => "Long",
        "空" => "Short",
        "标记价" => "Mark",
        "开仓价" => "Entry",
        "未实现盈亏" => "Unrealized PnL",
        "强平价" => "Liq. Price",
        "最近 100 笔" => "Last 100 orders",
        "最近 30 天" => "Last 30 days",
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
        "合约资产 · U本位" => "Futures Assets (USDT-M)",
        "合约持仓" => "Futures Positions",
        "资产" => "Asset",
        "可用" => "Available",
        "冻结" => "Locked",
        "钱包余额" => "Wallet",
        "暂无非零资产" => "No non-zero assets",
        "个非零资产" => "non-zero assets",
        "（该 Key 无交易权限）" => " (key has no trade permission)",
        "未配置 API 密钥，无法查看资产（可在设置页配置）" => {
            "No API key configured — set one in Settings to view assets"
        }
        "合约账户加载中…" => "Loading futures account…",
        // ---- 设置 ----
        "交易环境" => "Environment",
        "语言与主题" => "Language & Theme",
        "保存后生效；顶栏右侧 深色/浅色 与 中/EN 按钮可即时切换" => {
            "Takes effect on save; the Dark/Light and 中/EN buttons in the top bar switch instantly"
        }
        "数据目录" => "Data dir:",
        "本工具仅供个人交易辅助与研究，不构成任何投资建议；使用实盘交易产生的一切后果由使用者自行承担。" => {
            "For personal trading assistance and research only — not investment advice. You bear all consequences of live trading."
        }
        "测试网 (Testnet)" => "Testnet",
        "实盘" => "Live",
        "测试网密钥在 testnet.binance.vision / testnet.binancefuture.com 申请" => {
            "Get testnet keys at testnet.binance.vision / testnet.binancefuture.com"
        }
        "测试网与实盘的 API 密钥不通用，切换环境后需重新配置密钥" => {
            "Testnet and live API keys are not interchangeable — reconfigure keys after switching"
        }
        "⚠ 实盘模式下下单将使用真实资金，请确认密钥与环境正确！" => {
            "⚠ Live mode places orders with REAL funds — verify your key and environment!"
        }
        "网络代理（REST 与 WebSocket 均生效）" => "Proxy (applies to REST & WebSocket)",
        "启用代理" => "Enable proxy",
        "地址" => "Address",
        "请求参数" => "Request Parameters",
        "接收窗口" => "recvWindow",
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
        "设置已应用，正在重连…" => "Settings applied, reconnecting…",
        "已删除本机凭据库" => "Local vault deleted",
        "已清空端点覆盖：测试网与实盘端点不通用" => {
            "Endpoint overrides cleared: testnet and live endpoints are not interchangeable"
        }
        // ---- 登录页 ----
        "Binance 现货 / U本位合约 交易终端" => "Binance Spot / USDT-M Futures Trading Terminal",
        "解锁" => "Unlock",
        "配置 API 密钥" => "Configure API Key",
        "主密码（至少 8 位）" => "Master password (min 8 chars)",
        "再输一次" => "Confirm password",
        "主密码" => "Master password",
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
        "确认买入" => "Confirm Buy",
        "确认卖出" => "Confirm Sell",
        "取消" => "Cancel",
        "上一笔请求处理中…" => "Previous request in progress…",
        "请先修正表单中标红的错误" => "Fix the highlighted form errors first",
        // MA 均线开关状态（tooltip 与可访问名字共用）
        "已开" => "On",
        "已关" => "Off",
        "请填写 API Key 与 Secret" => "Enter the API Key and Secret",
        "请输入主密码与 API Key/Secret" => "Enter the master password and API Key/Secret",
        "点按切换均线，图上叠加 MA5/10/20/60" => "Toggle moving averages (MA5/10/20/60 overlay)",
        "在设置页配置密钥后查看" => "Configure keys on the Settings page to view",
        "只减仓" => "Reduce-Only",
        "委托价" => "Limit Price",
        "是" => "Yes",
        // ---- 图表 ----
        "加载行情中…" => "Loading market data…",
        "暂无K线数据（等待加载或检查网络/代理）" => "No candles yet (waiting or check network/proxy)",
        "24h 高" => "24h High",
        "24h 开" => "24h Open",
        "24h 收" => "24h Close",
        "24h 量" => "Vol",
        "24h 额" => "Turnover",
        "24h 低" => "24h Low",
        "资金费率" => "Funding",
        "挂单 " => "Order ",
        "触发 " => "Trigger ",
        "持仓开仓 " => "Position entry ",
        "买" => "Buy",
        "卖" => "Sell",
        "滚轮缩放 · 拖动平移 · 双击回最新" => "Scroll to zoom · Drag to pan · Double-click to reset",
        // ---- 使用说明页 ----
        "快速开始" => "Quick Start",
        "1. 在 testnet.binance.vision（现货）或 testnet.binancefuture.com（合约）用 GitHub 账号申请免费测试网 API Key" => {
            "1. Get a free testnet API key with your GitHub account at testnet.binance.vision (spot) or testnet.binancefuture.com (futures)"
        }
        "2. 启动本程序，在配置页输入 API Key / Secret 并设置主密码（用于加密保存到本机）" => {
            "2. Launch the app, enter your API Key / Secret on the setup screen and set a master password (encrypts them locally)"
        }
        "3. 默认即测试网模式（顶部金色 TESTNET 徽标），可查看行情、下单、撤单，放心演练" => {
            "3. Testnet is the default (gold TESTNET badge on top) — browse markets, place and cancel orders freely"
        }
        "4. 切换实盘前请再三确认：实盘密钥来自 binance.com 的 API 管理，建议绑定 IP 白名单并只开启读取+交易权限" => {
            "4. Before going live, double-check: live keys come from binance.com API Management; bind an IP whitelist and enable read + trade permissions only"
        }
        "页面导览" => "Pages",
        "现货交易 / 期货交易" => "Spot / Futures",
        "左右结构：左侧交易对列表（支持搜索与⭐自选置顶），中间 K 线与订单区，右侧盘口、成交明细与下单面板。两个市场各自记忆交易对、周期与表单。" => {
            "Left to right: symbol list (search + star favorites pinned), chart and order tabs, then order book, recent trades and the order panel. Each market remembers its own symbol, interval and form."
        }
        "K 线图" => "Chart",
        "滚轮缩放、按住拖动平移、双击回到最新。MA5/10/20/60 均线可在图上方开关；图上叠加当前挂单价、止损触发价与合约持仓开仓价。悬停显示十字光标与 OHLC 详情。K 线缓存于本地磁盘，切换交易对或周期先秒开缓存，再增量刷新最新数据。" => {
            "Scroll to zoom, drag to pan, double-click to snap to latest. Toggle MA5/10/20/60 above the chart; open orders, stop triggers and futures entry prices are overlaid. Hover for crosshair and OHLC details. Candles are cached on disk: switching symbol or interval opens instantly from cache, then refreshes with live data."
        }
        "下单面板" => "Order Panel",
        "限价/市价/止损止盈；合约另有跟踪委托（回调幅度 0.1-10%），现货另有 OCO（限价 + 止损限价成组，一边成交另一边自动撤销）；限价单可选 GTC/IOC/FOK（合约另有 GTX Post-Only）；合约下单可附带止盈/止损一次提交。比例按钮按可用余额估算数量；提交前有确认弹窗（实盘红色警示）。价格与数量会按交易所过滤器自动量化。" => {
            "Limit / market / stop-loss / take-profit; futures adds trailing stop (callback 0.1-10%), spot adds OCO (a limit + stop-limit pair — filling one side cancels the other). Limit orders take GTC/IOC/FOK (futures also GTX post-only); futures orders can attach TP/SL submitted in one go. Size buttons estimate from your available balance; every submit goes through a confirm dialog (red warning when live). Price and qty are auto-quantized to exchange filters."
        }
        "订单区" => "Orders",
        "汇总现货资产、合约资金概况与当前持仓。" => "Aggregates spot balances, futures funds overview and current positions.",
        "环境切换（测试网/实盘）、代理、端点覆盖、语言与主题、密钥管理。" => "Environment (testnet/live), proxy, endpoint overrides, language & theme, key management.",
        "交易对与自选" => "Symbols & Favorites",
        "左侧列表顶部搜索框实时过滤；点击行尾的星标可将交易对加入自选，自选排在列表最顶部" => {
            "The search box filters the list live; click the star at the end of a row to favorite it — favorites pin to the top"
        }
        "自选按市场分别保存（现货与合约互不影响），持久化到本机配置" => {
            "Favorites are saved per market (spot and futures independent) and persisted to the local config"
        }
        // ---- 帮助页：阶段 2/3 功能导览 ----
        "盘口与成交明细" => "Order Book & Trades",
        "盘口聚合档位" => "Depth Grouping",
        "盘口上方的档位胶囊按 1/10/100/1000 倍最小变动单位聚合挂单量，便于观察大单密集区" => {
            "The step pills above the book merge orders by 1/10/100/1000 × tick size, making large-order clusters easy to spot"
        }
        "历史订单与成交历史可一键导出 CSV（UTF-8 带 BOM，Excel 可直接打开），文件保存在数据目录 exports/ 下" => {
            "Order and trade history export to CSV in one click (UTF-8 with BOM, opens directly in Excel); files land in the exports/ folder of the data directory"
        }
        "交易页顶部的「预警」按钮打开当前交易对的预警面板：输入目标价即可添加，上破/下破方向按当前价自动判断，同价位不会重复添加" => {
            "Open the alert panel for the current symbol via the \"Alert\" button at the top of the trading page: type a target price to add one; direction (above/below) is inferred from the last price and duplicates are rejected"
        }
        "预警在本地对最新价实时评估，到价弹出提示并自动移除（一次性触发）；列表随配置持久化，重启后仍生效" => {
            "Alerts are evaluated locally against the latest price; on trigger a toast pops and the alert is removed (one-shot). The list persists with your config and survives restarts"
        }
        "设置页「WS 下单」开启后，合约订单经 WebSocket 通道提交（延迟更低）；任何失败会自动回退 REST" => {
            "With \"WS orders\" enabled in Settings, futures orders are submitted over the WebSocket channel (lower latency); any failure falls back to REST automatically"
        }
        "网络与代理" => "Network & Proxy",
        "程序直连 Binance；如需代理，在设置页启用并填写 http:// 或 socks5:// 地址（支持用户名密码），REST 与 WebSocket 全部生效" => {
            "The app connects to Binance directly; enable a proxy in Settings with an http:// or socks5:// address (user:pass supported) — applies to both REST and WebSocket"
        }
        "有自建网关或反代时，可在设置页覆盖四个端点（现货/合约 × REST/WS）" => {
            "With a self-hosted gateway or reverse proxy, override the four endpoints in Settings (spot/futures × REST/WS)"
        }
        "常见问题" => "Troubleshooting",
        "提示 -2015 / 签名错误" => "Error -2015 / signature",
        "检查 Key 是否用对环境（测试网与实盘互不通用）、是否绑定 IP 白名单、Secret 是否复制完整。" => {
            "Check the key matches the environment (testnet and live keys are not interchangeable), the IP whitelist, and that the secret was copied in full."
        }
        "提示 -1021 时间戳偏差" => "Error -1021 timestamp",
        "程序会自动与服务器校时并重试；若反复出现请检查系统时钟。" => {
            "The app auto-syncs time with the server and retries; if it persists, check your system clock."
        }
        "行情显示未连接" => "Market shows disconnected",
        "检查网络与代理设置；状态灯为红色时鼠标悬停可查看错误详情。" => {
            "Check network and proxy settings; hover a red status dot for error details."
        }
        "忘了主密码" => "Forgot master password",
        "在解锁页点「重置凭据库」删除本机加密文件后重新配置（密钥需重新输入）。" => {
            "On the unlock screen click \"Reset Vault\" to delete the local encrypted file and start over (keys must be re-entered)."
        }
        "风险提示" => "Risk Disclosure",
        "本工具仅供个人交易辅助与研究学习，不构成任何投资建议。加密货币交易风险极高，使用实盘交易产生的一切后果由使用者自行承担。请务必先在测试网完整演练后再考虑实盘。" => {
            "This tool is for personal trading assistance and research only — not investment advice. Crypto trading is extremely risky; you bear all consequences of live trading. Fully rehearse on testnet before going live."
        }
        // ---- 引擎 toast ----
        "请求超时，请检查网络/代理" => "Request timed out — check network/proxy",
        "未配置 API 密钥，请在设置中配置" => "No API key — configure one in Settings",
        "网络错误: " => "Network error: ",
        "响应解析失败: " => "Failed to parse response: ",
        "代理配置无效: " => "Invalid proxy config: ",
        "已撤销委托 #" => "Order canceled #",
        "已撤销 " => "Canceled all orders for ",
        "委托已提交 (#" => " order submitted (#",
        "杠杆已设为" => "leverage set to ",
        "持仓模式已切换" => "Position mode switched",
        "连接 Binance 失败: " => "Binance connection failed: ",
        "获取现货交易规则失败: " => "Failed to load spot rules: ",
        "获取合约交易规则失败: " => "Failed to load futures rules: ",
        "现货账户: " => "Spot account: ",
        "合约账户: " => "Futures account: ",
        "加载K线失败: " => "Failed to load candles: ",
        "价格/数量超出可表示范围，请检查输入" => "Price/qty out of representable range",
        " 交易规则（等待行情初始化）" => " (waiting for market init)",
        "已实现盈亏: " => "Realized PnL: ",
        // ---- 补充（评审缺口）----
        "盘口 10 档" => "Order Book (10)",
        "盘口" => "Order Book",
        "切换主题" => "Toggle theme",
        "切换语言" => "Toggle language",
        "实盘环境 · 真实资金" => "LIVE · Real Funds",
        "保存失败: " => "Failed to save: ",
        "该交易对无挂单" => "No open orders for this symbol",
        "请先在交易页选择交易对" => "Select a symbol on the trading page first",
        "主密码（验证现有主密码）" => "Master password (verify current)",
        "开" => "Open",
        "高" => "High",
        "低" => "Low",
        "收" => "Close",
        "量" => "Vol",
        "单向" => "One-way",
        "做多" => "Long",
        "做空" => "Short",
        "缺少" => "Missing",
        "加载失败: " => "Failed to load: ",
        "该交易对同时存在多空双向持仓，无法确定止盈止损的平仓方向，请先手动平掉一侧" => {
            "Symbol has both long and short positions — cannot determine the stop order's closing side. Close one side first"
        }
        "点击关闭" => "Click to dismiss",
        "清空搜索" => "Clear search",
        // ---- UI 重构补充（侧栏折叠 / 图表最大化与副图指标 / 空状态 / 步进提示）----
        "收起侧边栏" => "Collapse sidebar",
        "展开侧边栏" => "Expand sidebar",
        "收起下单面板" => "Collapse order panel",
        "展开下单面板" => "Expand order panel",
        "全屏" => "Fullscreen",
        "还原" => "Restore",
        "数量已按步进调整" => "Quantity adjusted to step",
        "下单后将显示在这里" => "Placed orders will appear here",
        "尝试其他关键词" => "Try another keyword",
        "余额到账后自动显示" => "Balances appear once credited",
        "交易规则加载中，请检查网络与代理" => "Loading symbol rules — check network & proxy",
        "错误" => "Error",
        // ---- Phase 1：TIF / 附加止盈止损 / 保证金模式 / 账户条 ----
        "有效期" => "Time in Force",
        "成交为止" => "GTC — rests until filled",
        "立即成交，剩余撤销" => "IOC — fill what's possible, cancel the rest",
        "全部成交或立即撤销" => "FOK — fill entirely or cancel",
        "GTX 只做maker" => "GTX post-only",
        "只做 maker，不会吃单" => "Post-only — never takes liquidity",
        "止盈/止损" => "TP/SL",
        "止盈 / 止损" => "Take-Profit / Stop-Loss",
        "TP/SL" => "TP/SL",
        "止盈止损" => "TP/SL",
        "附加止盈" => "Attach TP",
        "附加止损" => "Attach SL",
        "附加止盈/止损（触发后市价全平）" => "Attach TP/SL (market-close all on trigger)",
        "仓位止盈止损" => "Position TP/SL",
        "该止盈止损针对整个仓位（无论加仓或者减仓）。平仓后该止盈止损将被自动取消。当价格到达设定的触发价格时，将执行市价平仓委托。如果仓位数量超过市价委托单笔最大数量限制，委托将被拒绝。" => "This TP/SL applies to the whole position (regardless of later adds or reduces). It is cancelled automatically once the position closes. When price reaches the trigger, a market-close order is placed; if the position size exceeds the max quantity for a single market order, the order will be rejected.",
        "下单同时挂出触发即全平的止盈/止损单" => {
            "Also place trigger-to-close TP/SL orders with this order"
        }
        "仅单向持仓模式可用（双向模式请用持仓页设置）" => {
            "One-way mode only (use the Positions tab in hedge mode)"
        }
        "止盈触发" => "TP Trigger",
        "止损触发" => "SL Trigger",
        "至少填写一个触发价" => "Enter at least one trigger price",
        "已启用止盈/止损：至少填写一个触发价" => "TP/SL enabled: enter at least one trigger price",
        "已启用止盈/止损：先填写委托价或等待行情" => {
            "TP/SL enabled: enter a limit price or wait for market data"
        }
        "触发价超出可表示范围" => "Trigger price out of range",
        "止盈触发价方向不对：多头应高于现价，空头应低于现价" => {
            "TP trigger on the wrong side: above price for longs, below for shorts"
        }
        "止损触发价方向不对：多头应低于现价，空头应高于现价" => {
            "SL trigger on the wrong side: below price for longs, above for shorts"
        }
        "止盈触发价方向不对：多头应高于标记价，空头应低于标记价" => {
            "TP trigger on the wrong side: above mark price for longs, below for shorts"
        }
        "止损触发价方向不对：多头应低于标记价，空头应高于标记价" => {
            "SL trigger on the wrong side: below mark price for longs, above for shorts"
        }
        "触发后市价全平该持仓，提交前需确认" => {
            "Market-closes the whole position when triggered; a confirm dialog follows"
        }
        "提交" => "Submit",
        "关闭" => "Close",
        "全仓" => "Cross",
        "逐仓" => "Isolated",
        "点击切换保证金模式（全仓/逐仓）" => "Click to switch margin mode (cross/isolated)",
        "保证金模式已切为 " => "Margin mode switched to ",
        "保证金模式无需修改" => "Margin mode already set",
        "钱包" => "Wallet",
        "未实现" => "uPnL",
        "USDT 可用" => "USDT available",
        "最新" => "Last",
        "买一" => "Best bid",
        "卖一" => "Best ask",
        // ---- Phase 1：全撤 / 一键平仓 ----
        "撤销本市场全部" => "Cancel Entire Market",
        "确认撤销本市场全部" => "Confirm: Cancel Entire Market",
        "撤销该市场所有交易对的全部挂单" => "Cancel every open order in this market",
        "已撤销本市场全部挂单" => "Canceled every open order in the market",
        "全部平仓" => "Close All",
        "市价平仓所有持仓（聚合确认）" => "Market-close every position (aggregated confirm)",
        "市价平仓（有确认弹窗）" => "Market close (with confirm dialog)",
        "已按步进调整" => "adjusted to step",
        // ---- Phase 1：使用说明快捷键 ----
        "快捷键（交易页）" => "Hotkeys (Trading)",
        "B / S" => "B / S",
        "买入 / 卖出方向切换，并聚焦数量输入框（输入框获得焦点时不生效）" => {
            "Switch buy/sell side and focus the qty box (inactive while a text field has focus)"
        }
        "Ctrl+Shift+C" => "Ctrl+Shift+C",
        "撤销当前交易对的全部挂单" => "Cancel all open orders for the current symbol",
        "连按两次撤销当前交易对的全部挂单" => "Press twice to cancel all open orders for the current symbol",
        "当前挂单可单笔撤销、一键撤销该交易对全部挂单或本市场全部挂单；合约持仓支持行内 TP/SL、平仓、反手（市价平仓后反向开同等数量）与全部平仓（均走确认弹窗）。" => {
            "Open orders: cancel one by one, per symbol, or wipe the whole market; futures positions: inline TP/SL, close, reverse (market-close then reopen the same size on the other side) or close all (all with confirm dialogs)."
        }
        // ---- Phase 2：盘口聚合 / 价格预警 / CSV 导出 ----
        "档位" => "Step",
        "导出 CSV" => "Export CSV",
        "导出失败: " => "Export failed: ",
        "已导出 " => "Exported: ",
        "价格预警" => "Price Alerts",
        "价格预警（本地触发，到价弹提示）" => {
            "Price alerts (evaluated locally, toast on trigger)"
        }
        "预警" => "Alert",
        "目标价" => "Target price",
        "添加" => "Add",
        "删除" => "Delete",
        "请输入有效的目标价" => "Enter a valid target price",
        "行情加载中，请稍后再试" => "Market data loading — try again shortly",
        "暂无预警，输入目标价添加" => "No alerts yet — enter a target price to add one",
        "已添加预警 " => "Alert added: ",
        "已删除预警 " => "Alert removed: ",
        // ---- Phase 3：跟踪委托 / 反手 / OCO / WS 下单 ----
        "跟踪委托" => "Trailing Stop",
        "回调幅度" => "Callback",
        "回调幅度需为 0.1-10 的百分数" => "Callback rate must be a percentage from 0.1 to 10",
        "请输入回调幅度（0.1-10 的百分数）" => "Enter callback rate (0.1-10 %)",
        "百分比" => "percent",
        "%（0.1-10）" => "% (0.1-10)",
        "反手" => "Reverse",
        "反手交易" => "Reverse",
        "市价平仓并反向开同等数量（有确认弹窗）" => {
            "Market-close and reopen the same size on the opposite side (with confirm dialog)"
        }
        "开仓" => "Open",
        "OCO" => "OCO",
        "止损委托价" => "Stop Limit",
        "请输入止损委托价" => "Enter the stop-limit price",
        "卖出 OCO：限价腿应高于止损触发价" => {
            "Sell OCO: the limit leg must be above the stop trigger"
        }
        "买入 OCO：限价腿应低于止损触发价" => {
            "Buy OCO: the limit leg must be below the stop trigger"
        }
        "WS 下单" => "WS Orders",
        "合约订单经 WebSocket 通道提交（延迟更低）；任何失败自动回退 REST" => {
            "Submit futures orders over the WebSocket API (lower latency); any failure falls back to REST automatically"
        }
        "WS 下单失败，已回退 REST: " => "WS order failed, fell back to REST: ",
        // ---- 资金流水 ----
        "资金流水" => "Fund Flow",
        "资金流水获取失败" => "Fund flow fetch failed ",
        "暂无资金流水" => "No fund flow",
        "划转、资金费、充值与提现将显示在这里" => {
            "Transfers, funding fees, deposits and withdrawals will appear here"
        }
        "进入页签后自动加载" => "Loads automatically on tab open",
        "备注" => "Note",
        "最近 100 条" => "Last 100 entries",
        "订单历史获取失败" => "Order history fetch failed ",
        "成交历史获取失败" => "Trade history fetch failed ",
        "充值" => "Deposit",
        "提现" => "Withdraw",
        "划转" => "Transfer",
        "资金费" => "Funding Fee",
        "已实现盈亏" => "Realized PnL",
        "体验金" => "Bonus",
        "成功" => "Success",
        "处理中" => "Pending",
        "已入账" => "Credited",
        "审核中" => "Under Review",
        "已取消" => "Cancelled",
        "待审核" => "Awaiting Review",
        "失败" => "Failed",
        "完成" => "Completed",
        "转入" => "In",
        "转出" => "Out",
        "网络 " => "Network ",
        "手续费 " => "Fee ",
        _ => return_en_fallback(zh),
    }
}

/// 带动态后缀的兜底（如 "96,000.12" 之类不会走到这里；数字拼接串直接原样返回）
fn return_en_fallback(zh: &'static str) -> &'static str {
    zh
}
