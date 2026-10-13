# BnTradingPro

README.md (English) | [README_CN.md (中文)](README_CN.md)

A professional trading terminal for Binance Spot / USDⓈ-M Futures — a native desktop application (Windows) built with **Rust + egui**.

![tech](https://img.shields.io/badge/Rust-1.99+-orange) ![gui](https://img.shields.io/badge/GUI-egui%2Feframe-blue) ![license](https://img.shields.io/badge/status-beta-yellow)

## Features

- **Dual markets**: one-click switching between Spot and USDⓈ-M Futures
- **Real-time market data**: WebSocket push of 24h ticker, 20-level order book (100ms), trades, and candlesticks; futures additionally include mark price and funding rate
- **Custom-rendered candlestick chart**: candles + volume, scroll to zoom / drag to pan / double-click to return to the latest / crosshair details; with overlays for open-order prices and futures entry-price reference lines
- **Manual order entry**: Limit / Market / Stop-Limit / Take-Profit Limit (Spot); Stop-Market / Take-Profit Market (Futures); Hedge Mode supported with LONG/SHORT
- **Order protection**: prices and quantities are automatically quantized per symbol filters (tickSize / stepSize), with validation of minimum notional value / minimum quantity; a second confirmation dialog before submission (red warning in live-trading mode)
- **Account**: Spot balances (available / locked), futures account overview (wallet balance / unrealized PnL / margin / available), positions table (entry price / mark price / liquidation price / one-click close), leverage adjustment, one-way ↔ hedge mode switching
- **Order management**: real-time open-orders list (pushed via the user data stream; cancel a single order or all orders for the symbol), order history, trade fills
- **Security**: master password (Argon2id-derived) + AES-256-GCM encrypted storage of the API Key/Secret, keys zeroized on exit, fully sanitized logs
- **Networking**: HTTP / SOCKS5 proxy (effective for both REST and WebSocket), endpoint overrides, one-click Testnet switching (with a prominent badge)

## Quick Start

### 1. Build

```powershell
cargo build --release
# Output: target\release\bn-trading-pro.exe
```

Requires the MSVC toolchain (`rustup default stable` + Visual Studio Build Tools with the C++ workload).

### 2. Prepare API Keys (testnet strongly recommended first)

| Environment | Where to apply | Notes |
|---|---|---|
| Spot Testnet | https://testnet.binance.vision | Free, just sign in with GitHub |
| Futures Testnet | https://testnet.binancefuture.com | Sign in with GitHub |
| Live | binance.com → User Center → API Management | An IP whitelist is recommended; enable only "Read + Spot/Futures Trading", **never enable withdrawal permission** |

> Note: API keys are **not interchangeable** between the testnet and live trading; after switching environments, reconfigure your keys in Settings.

### 3. First Launch

1. Create a master password (used to encrypt the local key vault; it cannot be recovered if forgotten)
2. Enter the API Key / Secret → Save and connect
3. Defaults to Testnet mode (yellow `TESTNET` badge at the top); double-check before switching to live trading

## Configuration

Config file: `%APPDATA%\BnTradingPro\config.toml`; key vault: `vault.bin` in the same directory; logs: `logs\` in the same directory.

| Setting | Default | Description |
|---|---|---|
| `testnet` | `true` | Testnet toggle |
| `default_symbol` / `default_interval` | BTCUSDT / 15m | Default trading pair and candlestick interval at startup |
| `recv_window_ms` | 5000 | Validity window for signed requests |
| `proxy.enabled` / `proxy.url` | off | Proxy, e.g. `http://127.0.0.1:7890`, `socks5://user:pass@127.0.0.1:1080` |
| `override_spot_rest` and 3 more | empty | Endpoint overrides (self-hosted reverse proxy / debugging) |

Built-in default endpoints:

- Spot live `api.binance.com` / `stream.binance.com:9443`
- Spot testnet `testnet.binance.vision` / `stream.testnet.binance.vision:9443`
- Futures live `fapi.binance.com` / `fstream.binance.com`
- Futures testnet `testnet.binancefuture.com` / `stream.binancefuture.com`

## Architecture

```
UI thread (eframe, immediate mode)           Background engine (dedicated tokio runtime)
┌────────────────────────────────┐   Command   ┌────────────────────────────────────────────┐
│  read-only snapshot → render   │ ──────────→ │  cmd_loop    REST command execution        │
│  user actions → send command   │             │  market_ws   market stream + auto-reconnect│
└────────────────────────────────┘             │  user_ws     user data stream              │
        ▲                                      │               (listenKey keep-alive)       │
        └────── Arc<SharedState> ──────────────┴────────────────────────────────────────────┘
              (market / account / order snapshots + Toast + config version)
```

| Module | Responsibility |
|---|---|
| `binance/client.rs` | HMAC-SHA256 signing, time sync, rate limiting (X-MBX-USED-WEIGHT), Chinese mapping of error codes |
| `binance/ws.rs` | WebSocket connections: direct / HTTP CONNECT tunnel / SOCKS5 |
| `binance/symbols.rs` | exchangeInfo parsing and tick/step/minNotional quantization |
| `engine/` | Command loop, market data WebSocket (exponential backoff reconnect), user data stream (real-time reconciliation via executionReport / ORDER_TRADE_UPDATE) |
| `security/` | Argon2id KDF + AES-256-GCM vault |
| `ui/` | Themes (CJK fonts), custom-rendered candlestick chart, order book / order entry / orders / assets / settings |

## Testing

```powershell
cargo test          # 21 unit tests: signature vectors, quantization boundaries, vault round-trip, WS message parsing, etc.
cargo clippy -- -D warnings
```

## Disclaimer

- This tool is for personal trading assistance and research/learning purposes only, and does not constitute any investment advice
- You bear all consequences of live trading yourself; always validate on the testnet first
- Binance services are restricted in some countries/regions; please verify network reachability and compliance on your own
