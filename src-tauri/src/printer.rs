use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

pub async fn query(ip: &str, keys: &[&str]) -> anyhow::Result<serde_json::Value> {
    let mut s = timeout(Duration::from_secs(5), TcpStream::connect((ip, 9200))).await??;
    let req: serde_json::Map<String, serde_json::Value> = keys
        .iter()
        .map(|k| (k.to_string(), serde_json::Value::Null))
        .collect();
    s.write_all(format!("{{}}{}", serde_json::Value::Object(req)).as_bytes())
        .await?;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = timeout(Duration::from_secs(3), s.read(&mut chunk)).await??;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.iter().position(|&b| b == b'{') {
            if let Ok(v) = serde_json::from_slice(&buf[i..]) {
                return Ok(v);
            }
        }
    }
    anyhow::bail!("no valid response from {ip}")
}

pub async fn send(ip: &str, data: &[u8]) -> anyhow::Result<()> {
    let mut s = timeout(Duration::from_secs(5), TcpStream::connect((ip, 9100))).await??;
    s.write_all(data).await?;
    s.shutdown().await?;
    Ok(())
}
