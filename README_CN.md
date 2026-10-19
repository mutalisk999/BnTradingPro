# BnTradingPro

[README.md (English)](README.md) | README_CN.md（中文）

Binance 现货 / U 本位合约专业交易终端，使用 **Rust + egui** 构建的原生桌面应用（Windows）。

![tech](https://img.shields.io/badge/Rust-1.99+-orange) ![gui](https://img.shields.io/badge/GUI-egui%2Feframe-blue) ![license](https://img.shields.io/badge/status-beta-yellow)

## 界面预览

主界面 —— 现货交易页（暗色主题，实盘 BTCUSDT 实时行情：盘口 / K 线图 / 下单面板）：

![主界面](docs/images/main-en.png)

## 功能

- **双市场**：现货（Spot）与 U 本位合约（USDT-M Futures）一键切换
- **实时行情**：WebSocket 推送 24h 行情、20 档盘口（100ms）、逐笔成交、K 线；合约附标记价格与资金费率
- **自绘 K 线图**：蜡烛 + 成交量，滚轮缩放 / 拖动平移 / 双击回最新 / 十字光标详情；叠加挂单价与合约开仓价参考线
- **手动下单**：限价 / 市价 / 止损限价 / 止盈限价（现货），止损市价 / 止盈市价（合约），双向持仓模式支持 LONG/SHORT
- **下单保护**：按交易对过滤器自动量化价格（tickSize）与数量（stepSize），校验最小名义金额 / 最小数量；提交前二次确认弹窗（实盘红色警示）
- **账户**：现货余额（可用/冻结）、合约账户概况（钱包余额 / 未实现盈亏 / 保证金 / 可用）、持仓表（开仓价 / 标记价 / 强平价 / 一键平仓）、杠杆调整、单向↔双向持仓切换
- **订单管理**：实时挂单列表（用户数据流推送，可单笔撤销 / 撤销该对全部）、订单历史、成交明细
- **安全**：主密码（Argon2id 派生）+ AES-256-GCM 加密保存 API Key/Secret，密钥退出清零，日志全脱敏
- **网络**：HTTP / SOCKS5 代理（REST 与 WebSocket 均生效）、端点覆盖、Testnet 一键切换（带醒目角标）

## 快速开始

### 1. 构建

```powershell
cargo build --release
# 产物：target\release\bn-trading-pro.exe
```

需要 MSVC 工具链（`rustup default stable` + Visual Studio Build Tools C++ 工作负载）。

### 2. 准备 API Key（强烈建议先用测试网）

| 环境 | 申请地址 | 说明 |
|---|---|---|
| 现货测试网 | https://testnet.binance.vision | GitHub 登录即得，免费 |
| 合约测试网 | https://testnet.binancefuture.com | GitHub 登录即得 |
| 实盘 | binance.com → 用户中心 → API 管理 | 建议绑定 IP 白名单，仅开启"读取+现货/合约交易"，**禁止提币权限** |

> 注意：测试网与实盘的 API Key **互不通用**，切换环境后需在设置中重新配置密钥。

### 3. 首次启动

1. 创建主密码（用于加密本机密钥库，忘记无法恢复）
2. 输入 API Key / Secret → 保存并连接
3. 默认即测试网模式（顶部黄色 `TESTNET` 角标）；切换实盘前请再三确认

## 配置

配置文件：`%APPDATA%\BnTradingPro\config.toml`，密钥库：同目录 `vault.bin`，日志：同目录 `logs\`。

| 配置项 | 默认 | 说明 |
|---|---|---|
| `testnet` | `true` | 测试网开关 |
| `default_symbol` / `default_interval` | BTCUSDT / 15m | 启动默认交易对与 K 线周期 |
| `recv_window_ms` | 5000 | 签名请求有效期 |
| `proxy.enabled` / `proxy.url` | 关闭 | 代理，如 `http://127.0.0.1:7890`、`socks5://user:pass@127.0.0.1:1080` |
| `override_spot_rest` 等 4 项 | 空 | 端点覆盖（自建反代 / 调试） |

内置默认端点：

- 现货实盘 `api.binance.com` / `stream.binance.com:9443`
- 现货测试网 `testnet.binance.vision` / `stream.testnet.binance.vision:9443`
- 合约实盘 `fapi.binance.com` / `fstream.binance.com`
- 合约测试网 `testnet.binancefuture.com` / `stream.binancefuture.com`

## 架构

```
UI 线程（eframe，立即模式）           后台引擎（独立 tokio 运行时）
┌──────────────────────┐   Command    ┌───────────────────────────┐
│  只读快照 → 渲染      │ ──────────→ │  cmd_loop    REST 命令执行  │
│  用户操作 → 下发命令  │              │  market_ws  行情流+自动重连 │
└──────────────────────┘              │  user_ws    用户数据流      │
        ▲                             │             (listenKey 保活)│
        └────── Arc<SharedState> ─────┴───────────────────────────┘
              (行情/账户/订单快照 + Toast + 配置版本号)
```

| 模块 | 职责 |
|---|---|
| `binance/client.rs` | HMAC-SHA256 签名、时间同步、限速（X-MBX-USED-WEIGHT）、错误码中文映射 |
| `binance/ws.rs` | WS 连接：直连 / HTTP CONNECT 隧道 / SOCKS5 |
| `binance/symbols.rs` | exchangeInfo 解析与 tick/step/minNotional 量化 |
| `engine/` | 命令循环、行情 WS（指数退避重连）、用户数据流（executionReport / ORDER_TRADE_UPDATE 实时对账） |
| `security/` | Argon2id KDF + AES-256-GCM vault |
| `ui/` | 主题（CJK 字体）、自绘 K 线、盘口/下单/订单/资产/设置 |

## 测试

```powershell
cargo test          # 64 个单元测试：签名向量、量化边界、vault 回环、WS 消息解析等
cargo clippy -- -D warnings
```

## 声明

- 本工具仅供个人交易辅助与研究学习，不构成任何投资建议
- 使用实盘交易产生的一切后果由使用者自行承担，请务必先在测试网验证
- Binance 在部分国家/地区存在服务限制，请自行确认网络可达性与合规性
