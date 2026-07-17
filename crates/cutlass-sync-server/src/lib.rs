//! Cutlass collab relay. Rooms keyed by URL path; the server holds an
//! Automerge doc per room and speaks the Automerge sync protocol with
//! each peer over websocket binary frames. Clients never talk to each
//! other directly — the room doc is the hub, so late joiners get full
//! history and offline edits merge on reconnect.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use automerge::sync::{Message as SyncMessage, State as SyncState, SyncDoc};
use automerge::Automerge;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::Message as WsMessage;

struct Peer {
    state: SyncState,
    tx: UnboundedSender<WsMessage>,
}

#[derive(Default)]
struct Room {
    doc: Automerge,
    peers: HashMap<u64, Peer>,
}

type Rooms = Arc<Mutex<HashMap<String, Room>>>;

pub async fn run(addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    println!("cutlass-sync-server listening on {addr}");
    let rooms: Rooms = Rooms::default();
    let mut next_id = 0u64;
    loop {
        let (stream, from) = listener.accept().await?;
        next_id += 1;
        let rooms = rooms.clone();
        let id = next_id;
        tokio::spawn(async move {
            if let Err(e) = handle(stream, rooms, id).await {
                println!("peer {id} ({from}): closed ({e})");
            }
        });
    }
}

async fn handle(stream: TcpStream, rooms: Rooms, peer_id: u64) -> anyhow::Result<()> {
    let mut room_name = String::from("default");
    let ws = tokio_tungstenite::accept_hdr_async(stream, |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
        room_name = req.uri().path().trim_start_matches('/').to_string();
        if room_name.is_empty() {
            room_name = "default".into();
        }
        Ok(resp)
    })
    .await?;
    println!("peer {peer_id} joined room '{room_name}'");
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = unbounded_channel::<WsMessage>();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // register the peer and offer the room's current state
    {
        let mut rooms = rooms.lock().unwrap();
        let room = rooms.entry(room_name.clone()).or_default();
        let mut state = SyncState::new();
        while let Some(m) = room.doc.generate_sync_message(&mut state) {
            let _ = tx.send(WsMessage::Binary(m.encode().into()));
        }
        room.peers.insert(peer_id, Peer { state, tx: tx.clone() });
    }

    while let Some(msg) = stream.next().await {
        match msg? {
            // presence and other ephemera: text frames, relayed to the
            // rest of the room without touching the document
            WsMessage::Text(text) => {
                let rooms = rooms.lock().unwrap();
                if let Some(room) = rooms.get(&room_name) {
                    for (id, peer) in room.peers.iter() {
                        if *id != peer_id {
                            let _ = peer.tx.send(WsMessage::Text(text.clone()));
                        }
                    }
                }
            }
            WsMessage::Binary(bytes) => {
                let mut rooms = rooms.lock().unwrap();
                let Some(room) = rooms.get_mut(&room_name) else { break };
                if let Some(peer) = room.peers.get_mut(&peer_id) {
                    match SyncMessage::decode(&bytes) {
                        Ok(m) => {
                            if let Err(e) = room.doc.receive_sync_message(&mut peer.state, m) {
                                println!("peer {peer_id}: bad sync message: {e}");
                                continue;
                            }
                        }
                        Err(e) => {
                            println!("peer {peer_id}: undecodable frame: {e}");
                            continue;
                        }
                    }
                }
                // fan out whatever each peer is missing now
                for (id, peer) in room.peers.iter_mut() {
                    let mut n = 0;
                    while let Some(m) = room.doc.generate_sync_message(&mut peer.state) {
                        let _ = peer.tx.send(WsMessage::Binary(m.encode().into()));
                        n += 1;
                    }
                    if n > 0 {
                        println!("room '{room_name}': -> peer {id} ({n} msg)");
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(room) = rooms.lock().unwrap().get_mut(&room_name) {
        room.peers.remove(&peer_id);
    }
    println!("peer {peer_id} left room '{room_name}'");
    Ok(())
}
