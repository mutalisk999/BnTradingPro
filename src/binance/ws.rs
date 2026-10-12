//! WebSocket 连接建立：支持直连、HTTP CONNECT 代理与 SOCKS5 代理隧道
//!
//! 统一走"手动建立 TCP（可代理）→ TLS → tungstenite 握手"路径，保证单一返回类型。

use crate::config::ProxyConfig;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::Connector;
use url::Url;

pub type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 建立到 ws(s):// 的连接（自动应用代理配置；整体 15s 超时，防止半开连接卡死重连循环）
pub async fn connect(ws_url: &str, proxy: &ProxyConfig) -> Result<WsStream, String> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        connect_inner(ws_url, proxy),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => Err("连接超时（15s）".into()),
    }
}

async fn connect_inner(ws_url: &str, proxy: &ProxyConfig) -> Result<WsStream, String> {
    let url = Url::parse(ws_url).map_err(|e| format!("WS 地址无效: {e}"))?;
    let host = url.host_str().ok_or("WS 地址缺少主机")?.to_string();
    let port = url.port_or_known_default().ok_or("WS 地址缺少端口")?;

    // 1. TCP（直连或代理隧道）
    let tcp: TcpStream = match proxy_tcp(&host, port, proxy).await {
        Ok(s) => s,
        Err(e) => return Err(format!("连接 {host}:{port} 失败: {e}")),
    };

    // 2/3. TLS + tungstenite 握手
    let tls_connector = native_tls::TlsConnector::builder()
        .build()
        .map_err(|e| format!("TLS 初始化失败: {e}"))?;

    let request = ws_url
        .to_string()
        .into_client_request()
        .map_err(|e| format!("WS 握手请求构建失败: {e}"))?;
    let (stream, _resp) = tokio_tungstenite::client_async_tls_with_config(
        request,
        tcp,
        None,
        Some(Connector::NativeTls(tls_connector)),
    )
    .await
    .map_err(|e| format!("WS 握手失败: {e}"))?;
    Ok(stream)
}

/// TCP 连接（考虑代理）
async fn proxy_tcp(host: &str, port: u16, proxy: &ProxyConfig) -> Result<TcpStream, String> {
    if !(proxy.enabled && !proxy.url.trim().is_empty()) {
        return TcpStream::connect((host, port))
            .await
            .map_err(|e| format!("直连失败: {e}"));
    }
    let purl = Url::parse(proxy.url.trim()).map_err(|e| format!("代理地址无效: {e}"))?;
    let scheme = purl.scheme().to_ascii_lowercase();
    let phost = purl.host_str().ok_or("代理地址缺少主机")?.to_string();
    // 未写端口时按协议补默认端口（与 REST 侧 reqwest 行为一致）
    let pport = purl
        .port()
        .or(match scheme.as_str() {
            "http" => Some(80),
            "https" => Some(443),
            "socks5" | "socks5h" => Some(1080),
            _ => None,
        })
        .ok_or("代理地址缺少端口")?;
    let user = purl.username();
    let pass = purl.password().unwrap_or("");

    match scheme.as_str() {
        "socks5" | "socks5h" => {
            let addr = format!("{phost}:{pport}");
            // 目标用域名形式 → 远程解析（socks5h 语义）
            let target = tokio_socks::TargetAddr::Domain(host.to_string().into(), port);
            let stream = if user.is_empty() {
                tokio_socks::tcp::Socks5Stream::connect(addr.as_str(), target)
                    .await
                    .map_err(|e| format!("SOCKS5 连接失败: {e}"))?
            } else {
                tokio_socks::tcp::Socks5Stream::connect_with_password(
                    addr.as_str(),
                    target,
                    user,
                    pass,
                )
                .await
                .map_err(|e| format!("SOCKS5 连接失败: {e}"))?
            };
            Ok(stream.into_inner())
        }
        "http" | "https" => {
            // HTTP CONNECT 隧道
            let mut tcp = TcpStream::connect((phost.as_str(), pport))
                .await
                .map_err(|e| format!("连接代理失败: {e}"))?;
            let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
            if !user.is_empty() {
                use std::io::Write as _;
                let mut cred = Vec::new();
                let _ = write!(cred, "{user}:{pass}");
                let b64 = {
                    const T: &[u8; 64] =
                        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
                    let mut out = String::new();
                    for chunk in cred.chunks(3) {
                        let b = [
                            chunk[0],
                            *chunk.get(1).unwrap_or(&0),
                            *chunk.get(2).unwrap_or(&0),
                        ];
                        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
                        out.push(T[(n >> 18) as usize & 63] as char);
                        out.push(T[(n >> 12) as usize & 63] as char);
                        out.push(if chunk.len() > 1 {
                            T[(n >> 6) as usize & 63] as char
                        } else {
                            '='
                        });
                        out.push(if chunk.len() > 2 {
                            T[n as usize & 63] as char
                        } else {
                            '='
                        });
                    }
                    out
                };
                req.push_str(&format!("Proxy-Authorization: Basic {b64}\r\n"));
            }
            req.push_str("\r\n");
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            tcp.write_all(req.as_bytes())
                .await
                .map_err(|e| format!("发送 CONNECT 失败: {e}"))?;
            let mut buf = Vec::with_capacity(512);
            let mut one = [0u8; 1];
            // 逐字节读到头结束，避免污染后续 TLS 数据
            loop {
                let n = tcp
                    .read(&mut one)
                    .await
                    .map_err(|e| format!("代理响应读取失败: {e}"))?;
                if n == 0 {
                    return Err("代理连接中断".into());
                }
                buf.push(one[0]);
                if buf.ends_with(b"\r\n\r\n") || buf.len() > 8192 {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf);
            let ok = head
                .lines()
                .next()
                .map(|l| l.contains(" 200 ") || l.ends_with(" 200"))
                .unwrap_or(false);
            if !ok {
                return Err(format!(
                    "代理拒绝 CONNECT: {}",
                    head.lines().next().unwrap_or("")
                ));
            }
            Ok(tcp)
        }
        other => Err(format!("不支持的代理协议: {other}（支持 http/socks5）")),
    }
}
