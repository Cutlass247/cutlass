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

/// Which room a websocket path joins.
///
/// Everything after the leading slash, with the bare path `/` (and a client
/// that sends no path at all) landing in `default` rather than in a room whose
/// name is the empty string. Two people who both "just connect" should meet.
fn room_name_from_path(path: &str) -> String {
    let name = path.trim_start_matches('/');
    if name.is_empty() {
        "default".to_string()
    } else {
        name.to_string()
    }
}

impl Room {
    /// Register a peer and offer it everything the room already has.
    ///
    /// The sync state starts empty, so Automerge sends the whole history — this
    /// is what makes a late joiner see the edit somebody made an hour ago.
    fn join(&mut self, peer_id: u64, tx: UnboundedSender<WsMessage>) {
        let mut state = SyncState::new();
        while let Some(m) = self.doc.generate_sync_message(&mut state) {
            let _ = tx.send(WsMessage::Binary(m.encode().into()));
        }
        self.peers.insert(peer_id, Peer { state, tx });
    }

    /// Relay an ephemeral frame (presence, cursors) to the rest of the room.
    ///
    /// Never back to its sender: a client that saw its own cursor echoed would
    /// render a second one chasing it.
    fn relay(&self, from: u64, msg: WsMessage) {
        for (id, peer) in self.peers.iter() {
            if *id != from {
                let _ = peer.tx.send(msg.clone());
            }
        }
    }

    /// Apply one binary sync frame from a peer.
    ///
    /// An error here is the peer's problem, not the room's — a frame that
    /// won't decode, or that Automerge refuses, must not take down a session
    /// everyone else is working in. The caller logs and carries on.
    fn apply_sync(&mut self, peer_id: u64, bytes: &[u8]) -> Result<(), String> {
        let peer = self
            .peers
            .get_mut(&peer_id)
            .ok_or_else(|| "peer is not in this room".to_string())?;
        let m = SyncMessage::decode(bytes).map_err(|e| format!("undecodable frame: {e}"))?;
        self.doc
            .receive_sync_message(&mut peer.state, m)
            .map_err(|e| format!("bad sync message: {e}"))
    }

    /// Send every peer whatever it is missing now, and report how much each
    /// one was sent so the caller can log it.
    fn fan_out(&mut self) -> Vec<(u64, usize)> {
        let mut sent = Vec::new();
        for (id, peer) in self.peers.iter_mut() {
            let mut n = 0;
            while let Some(m) = self.doc.generate_sync_message(&mut peer.state) {
                let _ = peer.tx.send(WsMessage::Binary(m.encode().into()));
                n += 1;
            }
            if n > 0 {
                sent.push((*id, n));
            }
        }
        sent
    }
}

/// Lock the room table, ignoring poisoning.
///
/// One peer's connection task panicking while holding this would otherwise
/// poison it, and every other peer in every other room would then panic on
/// their next message — one bad frame disconnecting everybody. The same
/// reasoning as the desktop app and the licence server: what's guarded is a
/// document and a peer list, not an invariant a panic could leave unsafe.
trait LockExt<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

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
        room_name = room_name_from_path(req.uri().path());
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
    rooms
        .lock_ok()
        .entry(room_name.clone())
        .or_default()
        .join(peer_id, tx.clone());

    while let Some(msg) = stream.next().await {
        match msg? {
            // presence and other ephemera: text frames, relayed to the
            // rest of the room without touching the document
            WsMessage::Text(text) => {
                let rooms = rooms.lock_ok();
                if let Some(room) = rooms.get(&room_name) {
                    room.relay(peer_id, WsMessage::Text(text));
                }
            }
            WsMessage::Binary(bytes) => {
                let mut rooms = rooms.lock_ok();
                let Some(room) = rooms.get_mut(&room_name) else { break };
                // A frame this peer got wrong is logged and skipped. The room
                // keeps going: one client's bad frame is not everyone's problem.
                if let Err(e) = room.apply_sync(peer_id, &bytes) {
                    println!("peer {peer_id}: {e}");
                    continue;
                }
                for (id, n) in room.fan_out() {
                    println!("room '{room_name}': -> peer {id} ({n} msg)");
                }
            }
            _ => {}
        }
    }

    if let Some(room) = rooms.lock_ok().get_mut(&room_name) {
        room.peers.remove(&peer_id);
    }
    println!("peer {peer_id} left room '{room_name}'");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use automerge::transaction::Transactable;
    use automerge::{ObjType, ReadDoc, ROOT};
    use tokio::sync::mpsc::UnboundedReceiver;

    /// A client, standing in for the far end of a websocket. It keeps its own
    /// document and its own sync state, exactly as the desktop app does, so
    /// these tests exercise the real protocol rather than a mock of it.
    struct Client {
        id: u64,
        doc: Automerge,
        state: SyncState,
        rx: UnboundedReceiver<WsMessage>,
    }

    fn join(room: &mut Room, id: u64) -> Client {
        let (tx, rx) = unbounded_channel();
        room.join(id, tx);
        Client { id, doc: Automerge::new(), state: SyncState::new(), rx }
    }

    /// Pump messages between one client and the room until neither has
    /// anything left to say. The real thing is driven by socket reads; the
    /// convergence rule is the same.
    fn settle(room: &mut Room, c: &mut Client) {
        for _ in 0..50 {
            let mut moved = false;

            while let Ok(msg) = c.rx.try_recv() {
                if let WsMessage::Binary(b) = msg {
                    let m = SyncMessage::decode(&b).expect("server sent a decodable frame");
                    c.doc.receive_sync_message(&mut c.state, m).expect("client accepts it");
                    moved = true;
                }
            }

            while let Some(m) = c.doc.generate_sync_message(&mut c.state) {
                room.apply_sync(c.id, &m.encode()).expect("server accepts the client's frame");
                moved = true;
            }

            room.fan_out();
            if !moved && c.rx.is_empty() {
                return;
            }
        }
        panic!("sync never settled");
    }

    /// Put a named title on a document, the smallest edit that is visible from
    /// the other side.
    fn set_title(doc: &mut Automerge, title: &str) {
        let mut tx = doc.transaction();
        tx.put(ROOT, "title", title).unwrap();
        tx.commit();
    }

    fn title_of(doc: &Automerge) -> Option<String> {
        doc.get(ROOT, "title").ok().flatten().map(|(v, _)| v.into_string().unwrap())
    }

    #[test]
    fn a_path_picks_the_room_and_no_path_still_finds_one() {
        assert_eq!(room_name_from_path("/studio"), "studio");
        assert_eq!(room_name_from_path("/a/b"), "a/b");
        // The ones that matter: two people who "just connect" have to meet,
        // not land in separate rooms both named "".
        assert_eq!(room_name_from_path("/"), "default");
        assert_eq!(room_name_from_path(""), "default");
    }

    #[test]
    fn an_edit_from_one_peer_reaches_the_other() {
        let mut room = Room::default();
        let mut a = join(&mut room, 1);
        let mut b = join(&mut room, 2);
        settle(&mut room, &mut a);
        settle(&mut room, &mut b);

        set_title(&mut a.doc, "my film");
        settle(&mut room, &mut a); // a -> room
        settle(&mut room, &mut b); // room -> b

        assert_eq!(title_of(&b.doc).as_deref(), Some("my film"));
    }

    #[test]
    fn someone_who_joins_late_gets_what_they_missed() {
        let mut room = Room::default();
        let mut early = join(&mut room, 1);
        settle(&mut room, &mut early);
        set_title(&mut early.doc, "started without you");
        settle(&mut room, &mut early);

        // Joining now, after the edit was made and its author has gone quiet.
        let mut late = join(&mut room, 2);
        settle(&mut room, &mut late);

        assert_eq!(title_of(&late.doc).as_deref(), Some("started without you"));
    }

    #[test]
    fn two_peers_editing_at_once_both_survive() {
        let mut room = Room::default();
        let mut a = join(&mut room, 1);
        let mut b = join(&mut room, 2);
        settle(&mut room, &mut a);
        settle(&mut room, &mut b);

        // Neither has seen the other's change when they make their own, which
        // is the case a last-writer-wins server would lose one of.
        {
            let mut tx = a.doc.transaction();
            tx.put_object(ROOT, "from_a", ObjType::Map).unwrap();
            tx.commit();
        }
        {
            let mut tx = b.doc.transaction();
            tx.put_object(ROOT, "from_b", ObjType::Map).unwrap();
            tx.commit();
        }
        for _ in 0..3 {
            settle(&mut room, &mut a);
            settle(&mut room, &mut b);
        }

        for (who, doc) in [("a", &a.doc), ("b", &b.doc)] {
            assert!(doc.get(ROOT, "from_a").unwrap().is_some(), "{who} lost a's edit");
            assert!(doc.get(ROOT, "from_b").unwrap().is_some(), "{who} lost b's edit");
        }
    }

    #[test]
    fn a_frame_the_server_cannot_read_does_not_take_the_room_down() {
        let mut room = Room::default();
        let mut a = join(&mut room, 1);
        let mut b = join(&mut room, 2);
        settle(&mut room, &mut a);
        settle(&mut room, &mut b);
        set_title(&mut a.doc, "still here");
        settle(&mut room, &mut a);

        // Garbage from one peer. handle() logs this and continues; what must
        // not happen is the document changing or the room becoming unusable.
        assert!(room.apply_sync(1, b"not a sync message").is_err());
        assert!(room.apply_sync(99, b"also from nobody").is_err());

        settle(&mut room, &mut b);
        assert_eq!(title_of(&b.doc).as_deref(), Some("still here"));
    }

    #[test]
    fn presence_goes_to_everyone_but_the_sender() {
        let mut room = Room::default();
        let mut a = join(&mut room, 1);
        let mut b = join(&mut room, 2);
        let mut c = join(&mut room, 3);
        while a.rx.try_recv().is_ok() {}
        while b.rx.try_recv().is_ok() {}
        while c.rx.try_recv().is_ok() {}

        room.relay(a.id, WsMessage::Text("cursor at 12s".into()));

        // Echoing a cursor back to its owner draws a second one chasing it.
        assert!(a.rx.try_recv().is_err(), "sender got its own presence back");
        for (who, peer) in [("b", &mut b), ("c", &mut c)] {
            match peer.rx.try_recv() {
                Ok(WsMessage::Text(t)) => assert_eq!(t.as_str(), "cursor at 12s"),
                other => panic!("{who} should have got the presence frame, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_peer_that_left_stops_being_sent_to() {
        let mut room = Room::default();
        let mut a = join(&mut room, 1);
        let mut gone = join(&mut room, 2);
        settle(&mut room, &mut a);
        settle(&mut room, &mut gone);

        // What handle() does when the socket closes.
        room.peers.remove(&gone.id);

        set_title(&mut a.doc, "after you left");
        settle(&mut room, &mut a);

        assert!(gone.rx.try_recv().is_err(), "a departed peer was still sent frames");
        assert_eq!(room.peers.len(), 1);
    }

    #[test]
    fn a_poisoned_room_table_does_not_brick_the_server() {
        // One peer's task panicking mid-message would otherwise poison the
        // table and take down every other peer in every other room.
        let rooms: Rooms = Rooms::default();
        let r2 = rooms.clone();
        let _ = std::thread::spawn(move || {
            let _guard = r2.lock_ok();
            panic!("a peer task died holding the lock");
        })
        .join();

        assert!(rooms.lock().is_err(), "the mutex really is poisoned");
        rooms.lock_ok().entry("still-works".into()).or_default();
        assert!(rooms.lock_ok().contains_key("still-works"));
    }
}
