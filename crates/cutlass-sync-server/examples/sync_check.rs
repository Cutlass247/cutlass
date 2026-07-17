//! End-to-end collab proof over real websockets: server + two clients
//! (each a real cutlass-core Project, same loop the desktop app runs).
//! A and B edit concurrently; both must converge.
//! `cargo run -p cutlass-sync-server --example sync_check`

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cutlass_core::project::{Clip, Project};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::Message as WsMessage;

fn demo_clip(id: &str, track: &str, start: f64) -> Clip {
    Clip {
        id: id.into(),
        name: format!("{id}.mp4"),
        media: format!("m-{id}"),
        track: track.into(),
        start,
        len: 4.0,
        src_in: 0.0,
    }
}

/// Same loop the desktop runs: ws in → receive+generate; local ping →
/// generate. Sends happen outside the project lock.
async fn spawn_client(url: String, project: Project) -> (Arc<Mutex<Project>>, UnboundedSender<()>) {
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");
    let (mut sink, mut stream) = ws.split();
    let proj = Arc::new(Mutex::new(project));
    let (ping_tx, mut ping_rx) = unbounded_channel::<()>();
    let p = Arc::clone(&proj);
    tokio::spawn(async move {
        let mut sync = automerge::sync::State::new();
        let mut out: Vec<Vec<u8>> = Vec::new();
        {
            let mut p = p.lock().unwrap();
            while let Some(m) = p.generate_sync_message(&mut sync) {
                out.push(m);
            }
        }
        for m in out.drain(..) {
            let _ = sink.send(WsMessage::Binary(m.into())).await;
        }
        loop {
            tokio::select! {
                msg = stream.next() => {
                    let Some(Ok(WsMessage::Binary(bytes))) = msg else { break };
                    {
                        let mut p = p.lock().unwrap();
                        let _ = p.receive_sync_message(&mut sync, &bytes);
                        while let Some(m) = p.generate_sync_message(&mut sync) {
                            out.push(m);
                        }
                    }
                    for m in out.drain(..) {
                        let _ = sink.send(WsMessage::Binary(m.into())).await;
                    }
                }
                ping = ping_rx.recv() => {
                    if ping.is_none() { break }
                    {
                        let mut p = p.lock().unwrap();
                        while let Some(m) = p.generate_sync_message(&mut sync) {
                            out.push(m);
                        }
                    }
                    for m in out.drain(..) {
                        let _ = sink.send(WsMessage::Binary(m.into())).await;
                    }
                }
            }
        }
    });
    (proj, ping_tx)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = "127.0.0.1:9977".parse()?;
    tokio::spawn(cutlass_sync_server::run(addr));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let url = "ws://127.0.0.1:9977/demo".to_string();
    let (pa, ping_a) = spawn_client(url.clone(), Project::new("Collab")).await;
    let (pb, ping_b) = spawn_client(url.clone(), Project::new("Collab")).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // concurrent edits from both sides
    pa.lock().unwrap().add_clip(&demo_clip("from-a", "V1", 0.0))?;
    pb.lock().unwrap().add_clip(&demo_clip("from-b", "V2", 2.0))?;
    let _ = ping_a.send(());
    let _ = ping_b.send(());

    // wait for convergence
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let sa = pa.lock().unwrap().snapshot();
        let sb = pb.lock().unwrap().snapshot();
        let na = sa["clips"].as_array().map(|c| c.len()).unwrap_or(0);
        if sa == sb && na == 2 {
            println!("converged: both replicas see {na} clips, snapshots identical");
            break;
        }
        if Instant::now() > deadline {
            eprintln!("A: {sa}");
            eprintln!("B: {sb}");
            anyhow::bail!("no convergence within 5s");
        }
    }

    // late joiner gets full history from the room doc
    let (pc, _ping_c) = spawn_client(url, Project::new("Late")).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let sc = pc.lock().unwrap().snapshot();
        if sc["clips"].as_array().map(|c| c.len()).unwrap_or(0) == 2 {
            println!("late joiner caught up: 2 clips");
            break;
        }
        if Instant::now() > deadline {
            anyhow::bail!("late joiner never caught up");
        }
    }
    println!("OK");
    std::process::exit(0);
}
