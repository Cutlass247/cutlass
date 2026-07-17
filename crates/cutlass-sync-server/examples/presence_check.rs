//! Verify the presence side-channel: text frames relay to other room
//! peers (and only to them), without touching the doc.
//! `cargo run -p cutlass-sync-server --example presence_check`

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = "127.0.0.1:9978".parse()?;
    tokio::spawn(cutlass_sync_server::run(addr));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let (ws_a, _) = tokio_tungstenite::connect_async("ws://127.0.0.1:9978/p").await?;
    let (ws_b, _) = tokio_tungstenite::connect_async("ws://127.0.0.1:9978/p").await?;
    let (mut sink_a, mut stream_a) = ws_a.split();
    let (_sink_b, mut stream_b) = ws_b.split();

    let payload = r#"{"id":"a1","name":"editor-a","color":"hsl(200,75%,60%)","playhead":3.25}"#;
    sink_a.send(WsMessage::Text(payload.into())).await?;

    // B must receive it…
    let got = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(Ok(msg)) = stream_b.next().await {
            if let WsMessage::Text(t) = msg {
                return Some(t.to_string());
            }
        }
        None
    })
    .await?
    .ok_or_else(|| anyhow::anyhow!("B never received presence"))?;
    assert!(got.contains(r#""id":"a1""#) && got.contains("3.25"), "bad payload: {got}");
    println!("B received: {got}");

    // …and A must NOT hear its own echo
    let echo = tokio::time::timeout(Duration::from_millis(800), async {
        while let Some(Ok(msg)) = stream_a.next().await {
            if matches!(msg, WsMessage::Text(_)) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(!echo, "A received its own presence back");
    println!("no echo to sender. OK");
    std::process::exit(0);
}
