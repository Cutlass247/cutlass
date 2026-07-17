//! One-shot collab client: joins a room, syncs, drops a clip on V2, and
//! leaves. Any Cutlass window in the room watches it appear live.
//! `cargo run -p cutlass-sync-server --example phantom_edit -- [room]`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cutlass_core::project::{Clip, Project};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let room = std::env::args().nth(1).unwrap_or_else(|| "demo".into());
    let url = format!("ws://127.0.0.1:9720/{room}");
    let (ws, _) = tokio_tungstenite::connect_async(&url).await?;
    let (mut sink, mut stream) = ws.split();
    let proj = Arc::new(Mutex::new(Project::new("Phantom")));
    let mut sync = automerge::sync::State::new();

    let mut send_all = |p: &Arc<Mutex<Project>>, sync: &mut automerge::sync::State| {
        let mut out = Vec::new();
        let mut p = p.lock().unwrap();
        while let Some(m) = p.generate_sync_message(sync) {
            out.push(m);
        }
        out
    };

    for m in send_all(&proj, &mut sync) {
        sink.send(WsMessage::Binary(m.into())).await?;
    }

    // sync down the room state for ~2s, then drop the ghost clip
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(Ok(WsMessage::Binary(b))) = msg else { break };
                proj.lock().unwrap().receive_sync_message(&mut sync, &b).ok();
                for m in send_all(&proj, &mut sync) {
                    sink.send(WsMessage::Binary(m.into())).await?;
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    let end = proj.lock().unwrap().track_end("V2");
    proj.lock().unwrap().add_clip(&Clip {
        id: format!("ghost{}", std::process::id()),
        name: "👻 phantom.mp4".into(),
        media: "m-ghost".into(),
        track: "V2".into(),
        start: end,
        len: 3.0,
        src_in: 0.0,
    })?;
    for m in send_all(&proj, &mut sync) {
        sink.send(WsMessage::Binary(m.into())).await?;
    }
    // give the relay a beat to fan out
    tokio::time::sleep(Duration::from_millis(800)).await;
    println!("phantom clip pushed to room '{room}'");
    Ok(())
}
