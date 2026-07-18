//! The Cutlass project document — an Automerge CRDT, per Spike A.
//!
//! Locked-in rules from spikes/crdt-timeline/FINDINGS.md:
//! - Clips live in a MAP keyed by id; playback order is DERIVED from
//!   (track, start), never stored as list order.
//! - Moves write (track, start) atomically in one transaction so they
//!   merge as a unit.
//! - Conflicts are UI events, not errors (surfaced later via sync layer).

use std::collections::BTreeMap;

use automerge::{transaction::Transactable, AutoCommit, ObjId, ObjType, ReadDoc, ScalarValue, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: String,
    pub name: String,
    /// media pool id this clip references
    pub media: String,
    pub track: String,
    /// timeline position, seconds
    pub start: f64,
    /// clip duration on the timeline, seconds
    pub len: f64,
    /// source in-point, seconds
    pub src_in: f64,
    /// Effect Controls: sparse map of param → value (absent = default).
    /// BTreeMap keeps snapshot/equality deterministic. Keys: brightness,
    /// contrast, saturation, scale, rot, pos_x, pos_y, fade_in, fade_out,
    /// volume.
    #[serde(default)]
    pub fx: BTreeMap<String, f64>,
}

/// Effect params parsed from a clip's `fx` map to identity-defaulted
/// values. Shared vocabulary between preview, export, and playback.
pub fn fx_from_json(clip: &serde_json::Value) -> BTreeMap<String, f64> {
    clip.get("fx")
        .and_then(|f| f.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f)))
                .collect()
        })
        .unwrap_or_default()
}

pub struct Project {
    doc: AutoCommit,
}

impl Default for Project {
    fn default() -> Self {
        Self::new("Untitled")
    }
}

impl Project {
    pub fn new(name: &str) -> Self {
        // Deterministic bootstrap: fixed actor + time 0 makes the initial
        // change byte-identical across every Cutlass instance, so any two
        // projects share ancestry and can merge in a collab session
        // without root-map conflicts. Real edits use a random actor.
        let mut doc = AutoCommit::new();
        doc.set_actor(automerge::ActorId::from(b"cutlass.bootstrap".as_slice()));
        doc.put_object(automerge::ROOT, "clips", ObjType::Map)
            .expect("init clips");
        doc.put_object(automerge::ROOT, "media", ObjType::Map)
            .expect("init media");
        doc.commit_with(
            automerge::transaction::CommitOptions::default().with_time(0),
        );
        doc.set_actor(automerge::ActorId::random());
        doc.put(automerge::ROOT, "name", name).expect("init name");
        Self { doc }
    }

    pub fn load(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(Self {
            doc: AutoCommit::load(bytes)?,
        })
    }

    pub fn save(&mut self) -> Vec<u8> {
        self.doc.save()
    }

    fn clips_obj(&self) -> ObjId {
        let (_, id) = self
            .doc
            .get(automerge::ROOT, "clips")
            .expect("read clips")
            .expect("clips map exists");
        id
    }

    /// The media pool lives in the document so a saved/synced project
    /// carries everything needed to rebuild itself.
    fn media_obj(&mut self) -> ObjId {
        if let Ok(Some((_, id))) = self.doc.get(automerge::ROOT, "media") {
            return id;
        }
        self.doc
            .put_object(automerge::ROOT, "media", ObjType::Map)
            .expect("create media map")
    }

    pub fn set_media(
        &mut self,
        id: &str,
        name: &str,
        path: &str,
        duration_s: f64,
    ) -> anyhow::Result<()> {
        let media = self.media_obj();
        let obj = self.doc.put_object(&media, id, ObjType::Map)?;
        self.doc.put(&obj, "name", name)?;
        self.doc.put(&obj, "path", path)?;
        self.doc.put(&obj, "duration_s", duration_s)?;
        Ok(())
    }

    /// (id, name, path, duration_s) for every media entry in the doc.
    pub fn media_entries(&mut self) -> Vec<(String, String, String, f64)> {
        let media = self.media_obj();
        let ids: Vec<String> = self.doc.keys(&media).collect();
        ids.into_iter()
            .filter_map(|id| {
                let (_, obj) = self.doc.get(&media, &id).ok()??;
                let s = |k: &str| -> Option<String> {
                    match self.doc.get(&obj, k).ok()?? {
                        (Value::Scalar(v), _) => match v.as_ref() {
                            ScalarValue::Str(v) => Some(v.to_string()),
                            _ => None,
                        },
                        _ => None,
                    }
                };
                let d = match self.doc.get(&obj, "duration_s").ok()?? {
                    (Value::Scalar(v), _) => v.as_ref().to_f64()?,
                    _ => return None,
                };
                Some((id, s("name")?, s("path")?, d))
            })
            .collect()
    }

    pub fn add_clip(&mut self, clip: &Clip) -> anyhow::Result<()> {
        let clips = self.clips_obj();
        let obj = self.doc.put_object(&clips, &clip.id, ObjType::Map)?;
        self.doc.put(&obj, "name", clip.name.as_str())?;
        self.doc.put(&obj, "media", clip.media.as_str())?;
        self.doc.put(&obj, "track", clip.track.as_str())?;
        self.doc.put(&obj, "start", clip.start)?;
        self.doc.put(&obj, "len", clip.len)?;
        self.doc.put(&obj, "src_in", clip.src_in)?;
        if !clip.fx.is_empty() {
            self.write_fx(&obj, &clip.fx)?;
        }
        Ok(())
    }

    /// (Re)write a clip's fx sub-map to exactly `fx`.
    fn write_fx(&mut self, clip_obj: &ObjId, fx: &BTreeMap<String, f64>) -> anyhow::Result<()> {
        let fxobj = self.doc.put_object(clip_obj, "fx", ObjType::Map)?;
        for (k, v) in fx {
            self.doc.put(&fxobj, k.as_str(), *v)?;
        }
        Ok(())
    }

    /// Set one Effect Controls parameter on a clip. Creates the fx map on
    /// first use; each param is its own key so concurrent effect edits
    /// merge cleanly.
    pub fn set_effect(&mut self, id: &str, key: &str, value: f64) -> anyhow::Result<()> {
        let clips = self.clips_obj();
        let (_, obj) = self
            .doc
            .get(&clips, id)?
            .ok_or_else(|| anyhow::anyhow!("no clip {id}"))?;
        let fxobj = match self.doc.get(&obj, "fx")? {
            Some((Value::Object(ObjType::Map), fxid)) => fxid,
            _ => self.doc.put_object(&obj, "fx", ObjType::Map)?,
        };
        self.doc.put(&fxobj, key, value)?;
        Ok(())
    }

    /// Move = (track, start) written in one transaction (merges as a unit).
    pub fn move_clip(&mut self, id: &str, track: &str, start: f64) -> anyhow::Result<()> {
        let clips = self.clips_obj();
        let (_, obj) = self
            .doc
            .get(&clips, id)?
            .ok_or_else(|| anyhow::anyhow!("no clip {id}"))?;
        self.doc.put(&obj, "track", track)?;
        self.doc.put(&obj, "start", start)?;
        Ok(())
    }

    pub fn remove_clip(&mut self, id: &str) -> anyhow::Result<()> {
        let clips = self.clips_obj();
        self.doc.delete(&clips, id)?;
        Ok(())
    }

    /// Trim = (start, len, src_in) written together so a trim merges as a
    /// unit. Left-edge trims change all three; right-edge trims only len.
    pub fn trim_clip(
        &mut self,
        id: &str,
        start: f64,
        len: f64,
        src_in: f64,
    ) -> anyhow::Result<()> {
        let clips = self.clips_obj();
        let (_, obj) = self
            .doc
            .get(&clips, id)?
            .ok_or_else(|| anyhow::anyhow!("no clip {id}"))?;
        self.doc.put(&obj, "start", start)?;
        self.doc.put(&obj, "len", len)?;
        self.doc.put(&obj, "src_in", src_in)?;
        Ok(())
    }

    /// Razor out a source range [src_from, src_to) from a clip — the
    /// transcript edit primitive ("delete these words"). The clip splits
    /// into up to two, and later clips on the track ripple left by the
    /// removed duration. `new_id` names the right-hand part if one exists.
    pub fn razor_out(
        &mut self,
        id: &str,
        src_from: f64,
        src_to: f64,
        new_id: &str,
    ) -> anyhow::Result<()> {
        let snap = self.snapshot();
        let clip = snap["clips"]
            .as_array()
            .and_then(|cs| cs.iter().find(|c| c["id"] == id))
            .ok_or_else(|| anyhow::anyhow!("no clip {id}"))?
            .clone();
        let (start, len, src_in) = (
            clip["start"].as_f64().unwrap_or(0.0),
            clip["len"].as_f64().unwrap_or(0.0),
            clip["src_in"].as_f64().unwrap_or(0.0),
        );
        let src_end = src_in + len;
        let from = src_from.clamp(src_in, src_end);
        let to = src_to.clamp(src_in, src_end);
        let removed = to - from;
        if removed <= 1e-9 {
            return Ok(());
        }
        let left_len = from - src_in;
        let right_len = src_end - to;

        if right_len > 1e-9 {
            self.add_clip(&Clip {
                id: new_id.to_string(),
                name: clip["name"].as_str().unwrap_or("").to_string(),
                media: clip["media"].as_str().unwrap_or("").to_string(),
                track: clip["track"].as_str().unwrap_or("V1").to_string(),
                start: start + left_len,
                len: right_len,
                src_in: to,
                fx: fx_from_json(&clip), // split inherits the clip's effects
            })?;
        }
        if left_len > 1e-9 {
            self.trim_clip(id, start, left_len, src_in)?;
        } else {
            self.remove_clip(id)?;
        }
        // ripple everything after the original clip left by the cut length
        let clips_obj = self.clips_obj();
        if let Some(cs) = snap["clips"].as_array() {
            for c in cs {
                let cid = c["id"].as_str().unwrap_or("");
                let cstart = c["start"].as_f64().unwrap_or(0.0);
                if cid != id && c["track"] == clip["track"] && cstart > start + len - 1e-9 {
                    if let Some((_, obj)) = self.doc.get(&clips_obj, cid)? {
                        self.doc.put(&obj, "start", (cstart - removed).max(0.0))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Ripple delete: remove the clip and close the gap — every later clip
    /// on the same track shifts left by the removed length.
    pub fn remove_clip_ripple(&mut self, id: &str) -> anyhow::Result<()> {
        let snap = self.snapshot();
        let clips = snap["clips"].as_array().cloned().unwrap_or_default();
        let removed = clips
            .iter()
            .find(|c| c["id"] == id)
            .ok_or_else(|| anyhow::anyhow!("no clip {id}"))?
            .clone();
        let (track, start, len) = (
            removed["track"].as_str().unwrap_or("").to_string(),
            removed["start"].as_f64().unwrap_or(0.0),
            removed["len"].as_f64().unwrap_or(0.0),
        );
        self.remove_clip(id)?;
        let clips_obj = self.clips_obj();
        for c in &clips {
            let cid = c["id"].as_str().unwrap_or("");
            let cstart = c["start"].as_f64().unwrap_or(0.0);
            if cid != id && c["track"] == track.as_str() && cstart > start {
                if let Some((_, obj)) = self.doc.get(&clips_obj, cid)? {
                    self.doc.put(&obj, "start", (cstart - len).max(0.0))?;
                }
            }
        }
        Ok(())
    }

    /// Materialize the known schema to JSON for the UI. Clip order in the
    /// returned array is the derived render order: (track, start).
    pub fn snapshot(&self) -> serde_json::Value {
        let name = match self.doc.get(automerge::ROOT, "name") {
            Ok(Some((Value::Scalar(s), _))) => match s.as_ref() {
                ScalarValue::Str(s) => s.to_string(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let clips_obj = self.clips_obj();
        let mut clips: Vec<Clip> = self
            .doc
            .keys(&clips_obj)
            .filter_map(|id| self.read_clip(&clips_obj, &id))
            .collect();
        clips.sort_by(|a, b| {
            a.track
                .cmp(&b.track)
                .then(a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal))
        });
        json!({ "name": name, "clips": clips })
    }

    fn read_clip(&self, clips_obj: &ObjId, id: &str) -> Option<Clip> {
        let (_, obj) = self.doc.get(clips_obj, id).ok()??;
        let get_str = |key: &str| -> Option<String> {
            match self.doc.get(&obj, key).ok()?? {
                (Value::Scalar(s), _) => match s.as_ref() {
                    ScalarValue::Str(v) => Some(v.to_string()),
                    _ => None,
                },
                _ => None,
            }
        };
        let get_f64 = |key: &str| -> Option<f64> {
            match self.doc.get(&obj, key).ok()?? {
                (Value::Scalar(s), _) => s.as_ref().to_f64(),
                _ => None,
            }
        };
        let mut fx = BTreeMap::new();
        if let Ok(Some((Value::Object(ObjType::Map), fxid))) = self.doc.get(&obj, "fx") {
            for k in self.doc.keys(&fxid) {
                if let Ok(Some((Value::Scalar(s), _))) = self.doc.get(&fxid, &k) {
                    if let Some(v) = s.as_ref().to_f64() {
                        fx.insert(k, v);
                    }
                }
            }
        }
        Some(Clip {
            id: id.to_string(),
            name: get_str("name")?,
            media: get_str("media")?,
            track: get_str("track")?,
            start: get_f64("start")?,
            len: get_f64("len")?,
            src_in: get_f64("src_in")?,
            fx,
        })
    }

    /// All clips, in derived render order.
    pub fn clips_state(&self) -> Vec<Clip> {
        let clips_obj = self.clips_obj();
        let mut clips: Vec<Clip> = self
            .doc
            .keys(&clips_obj)
            .filter_map(|id| self.read_clip(&clips_obj, &id))
            .collect();
        clips.sort_by(|a, b| {
            a.track
                .cmp(&b.track)
                .then(a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal))
        });
        clips
    }

    /// Drive the clips map to exactly `target`, as new CRDT changes.
    /// This is the undo/redo primitive: restoring an earlier state via
    /// forward operations stays correct under collab (a byte rollback
    /// would resurrect remote edits on the next sync).
    pub fn restore_clips(&mut self, target: &[Clip]) -> anyhow::Result<()> {
        let current = self.clips_state();
        for t in target {
            match current.iter().find(|c| c.id == t.id) {
                None => self.add_clip(t)?,
                Some(c) if c != t => {
                    let clips = self.clips_obj();
                    let (_, obj) = self
                        .doc
                        .get(&clips, &t.id)?
                        .ok_or_else(|| anyhow::anyhow!("clip vanished: {}", t.id))?;
                    self.doc.put(&obj, "name", t.name.as_str())?;
                    self.doc.put(&obj, "media", t.media.as_str())?;
                    self.doc.put(&obj, "track", t.track.as_str())?;
                    self.doc.put(&obj, "start", t.start)?;
                    self.doc.put(&obj, "len", t.len)?;
                    self.doc.put(&obj, "src_in", t.src_in)?;
                    if c.fx != t.fx {
                        self.write_fx(&obj, &t.fx)?;
                    }
                }
                Some(_) => {}
            }
        }
        for c in &current {
            if !target.iter().any(|t| t.id == c.id) {
                self.remove_clip(&c.id)?;
            }
        }
        Ok(())
    }

    /// Razor multiple source ranges out of one media's V1 clips in a
    /// single logical edit (silence/filler removal). Ranges are applied
    /// high-to-low so earlier ranges stay inside clips with stable ids.
    /// Returns how many cuts landed.
    pub fn razor_out_ranges(
        &mut self,
        media_id: &str,
        ranges: &[(f64, f64)],
        id_seed: &str,
    ) -> anyhow::Result<u32> {
        let mut ranges: Vec<(f64, f64)> = ranges.to_vec();
        ranges.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut cut = 0;
        for (i, (from, to)) in ranges.iter().enumerate() {
            let mid = (from + to) / 2.0;
            let target = self.clips_state().into_iter().find(|c| {
                c.track == "V1"
                    && c.media == media_id
                    && mid >= c.src_in
                    && mid < c.src_in + c.len
            });
            if let Some(clip) = target {
                self.razor_out(&clip.id, *from, *to, &format!("{id_seed}-{i}"))?;
                cut += 1;
            }
        }
        Ok(cut)
    }

    // ── sync (Automerge sync protocol; used by the collab session) ─────
    pub fn generate_sync_message(
        &mut self,
        state: &mut automerge::sync::State,
    ) -> Option<Vec<u8>> {
        use automerge::sync::SyncDoc;
        self.doc.sync().generate_sync_message(state).map(|m| m.encode())
    }

    pub fn receive_sync_message(
        &mut self,
        state: &mut automerge::sync::State,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        use automerge::sync::SyncDoc;
        let msg = automerge::sync::Message::decode(bytes)?;
        self.doc.sync().receive_sync_message(state, msg)?;
        Ok(())
    }

    /// End of the last clip on `track` — where an appended clip starts.
    pub fn track_end(&self, track: &str) -> f64 {
        self.snapshot()["clips"]
            .as_array()
            .map(|clips| {
                clips
                    .iter()
                    .filter(|c| c["track"] == track)
                    .map(|c| c["start"].as_f64().unwrap_or(0.0) + c["len"].as_f64().unwrap_or(0.0))
                    .fold(0.0, f64::max)
            })
            .unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_clip(id: &str, track: &str, start: f64) -> Clip {
        Clip {
            id: id.into(),
            name: format!("{id}.mp4"),
            media: format!("m-{id}"),
            track: track.into(),
            start,
            len: 4.0,
            src_in: 0.0,
            fx: BTreeMap::new(),
        }
    }

    #[test]
    fn add_move_snapshot_roundtrip() {
        let mut p = Project::new("Trailer");
        p.add_clip(&demo_clip("a", "V1", 0.0)).unwrap();
        p.add_clip(&demo_clip("b", "V1", 4.0)).unwrap();
        p.move_clip("a", "V2", 1.5).unwrap();

        let snap = p.snapshot();
        assert_eq!(snap["name"], "Trailer");
        let clips = snap["clips"].as_array().unwrap();
        assert_eq!(clips.len(), 2);
        // derived order: V1/b before V2/a
        assert_eq!(clips[0]["id"], "b");
        assert_eq!(clips[1]["id"], "a");
        assert_eq!(clips[1]["start"], 1.5);
        assert_eq!(p.track_end("V1"), 8.0);
    }

    #[test]
    fn trim_writes_all_three_fields() {
        let mut p = Project::new("T");
        p.add_clip(&demo_clip("a", "V1", 2.0)).unwrap();
        // left-edge trim 1s into the clip
        p.trim_clip("a", 3.0, 3.0, 1.0).unwrap();
        let snap = p.snapshot();
        assert_eq!(snap["clips"][0]["start"], 3.0);
        assert_eq!(snap["clips"][0]["len"], 3.0);
        assert_eq!(snap["clips"][0]["src_in"], 1.0);
    }

    #[test]
    fn razor_out_splits_and_ripples() {
        let mut p = Project::new("T");
        let mut a = demo_clip("a", "V1", 0.0);
        a.len = 10.0;
        p.add_clip(&a).unwrap();
        p.add_clip(&demo_clip("b", "V1", 10.0)).unwrap();
        // cut source 3..5 out of clip a (src_in 0)
        p.razor_out("a", 3.0, 5.0, "a-r").unwrap();
        let snap = p.snapshot();
        let get = |id: &str| {
            snap["clips"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == id)
                .cloned()
                .unwrap()
        };
        let left = get("a");
        assert_eq!((left["start"].as_f64(), left["len"].as_f64(), left["src_in"].as_f64()),
                   (Some(0.0), Some(3.0), Some(0.0)));
        let right = get("a-r");
        assert_eq!((right["start"].as_f64(), right["len"].as_f64(), right["src_in"].as_f64()),
                   (Some(3.0), Some(5.0), Some(5.0)));
        assert_eq!(get("b")["start"], 8.0); // rippled left by 2
    }

    #[test]
    fn razor_out_at_clip_head_drops_left_part() {
        let mut p = Project::new("T");
        p.add_clip(&demo_clip("a", "V1", 2.0)).unwrap(); // len 4, src_in 0
        p.razor_out("a", 0.0, 1.5, "a-r").unwrap();
        let snap = p.snapshot();
        let clips = snap["clips"].as_array().unwrap();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0]["id"], "a-r");
        assert_eq!(clips[0]["start"], 2.0);
        assert_eq!(clips[0]["len"], 2.5);
        assert_eq!(clips[0]["src_in"], 1.5);
    }

    #[test]
    fn ripple_delete_closes_the_gap() {
        let mut p = Project::new("T");
        p.add_clip(&demo_clip("a", "V1", 0.0)).unwrap();
        p.add_clip(&demo_clip("b", "V1", 4.0)).unwrap();
        p.add_clip(&demo_clip("c", "V1", 8.0)).unwrap();
        p.add_clip(&demo_clip("x", "V2", 6.0)).unwrap(); // other track: untouched
        p.remove_clip_ripple("a").unwrap();
        let snap = p.snapshot();
        let get = |id: &str| {
            snap["clips"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == id)
                .map(|c| c["start"].as_f64().unwrap())
        };
        assert_eq!(get("a"), None);
        assert_eq!(get("b"), Some(0.0));
        assert_eq!(get("c"), Some(4.0));
        assert_eq!(get("x"), Some(6.0));
    }

    #[test]
    fn effects_persist_and_undo() {
        let mut p = Project::new("Fx");
        p.add_clip(&demo_clip("a", "V1", 0.0)).unwrap();
        let before = p.clips_state();

        p.set_effect("a", "brightness", 0.3).unwrap();
        p.set_effect("a", "volume", 0.5).unwrap();
        let snap = p.snapshot();
        assert_eq!(snap["clips"][0]["fx"]["brightness"], 0.3);
        assert_eq!(snap["clips"][0]["fx"]["volume"], 0.5);

        let after = p.clips_state();
        assert_ne!(before, after); // fx changed identity → undoable
        p.restore_clips(&before).unwrap(); // undo strips effects
        assert!(p.clips_state()[0].fx.is_empty());
        p.restore_clips(&after).unwrap(); // redo restores them
        assert_eq!(p.clips_state()[0].fx.get("brightness"), Some(&0.3));
    }

    #[test]
    fn restore_clips_round_trips_any_edit() {
        let mut p = Project::new("T");
        p.add_clip(&demo_clip("a", "V1", 0.0)).unwrap();
        p.add_clip(&demo_clip("b", "V1", 4.0)).unwrap();
        let before = p.clips_state();

        p.move_clip("a", "V2", 9.0).unwrap();
        p.remove_clip("b").unwrap();
        p.add_clip(&demo_clip("c", "V1", 1.0)).unwrap();
        let after = p.clips_state();

        p.restore_clips(&before).unwrap(); // undo
        assert_eq!(p.clips_state(), before);
        p.restore_clips(&after).unwrap(); // redo
        assert_eq!(p.clips_state(), after);
    }

    #[test]
    fn razor_out_ranges_cuts_all_high_to_low() {
        let mut p = Project::new("T");
        let mut a = demo_clip("a", "V1", 0.0);
        a.len = 10.0;
        a.media = "m".into();
        p.add_clip(&a).unwrap();
        let n = p
            .razor_out_ranges("m", &[(2.0, 3.0), (6.0, 7.0)], "cut")
            .unwrap();
        assert_eq!(n, 2);
        let clips = p.clips_state();
        assert_eq!(clips.len(), 3);
        let total: f64 = clips.iter().map(|c| c.len).sum();
        assert!((total - 8.0).abs() < 1e-6, "total {total}");
        // contiguous: no gaps after ripple
        assert_eq!(clips[0].start, 0.0);
        assert!((clips[1].start - clips[0].len).abs() < 1e-6);
    }

    #[test]
    fn save_load_roundtrip() {
        let mut p = Project::new("Trailer");
        p.add_clip(&demo_clip("a", "V1", 0.0)).unwrap();
        p.set_media("m-a", "a.mp4", "C:/media/a.mp4", 12.5).unwrap();
        let bytes = p.save();
        let mut p2 = Project::load(&bytes).unwrap();
        assert_eq!(p2.snapshot(), p.snapshot());
        assert_eq!(
            p2.media_entries(),
            vec![("m-a".into(), "a.mp4".into(), "C:/media/a.mp4".into(), 12.5)]
        );
    }

    #[test]
    fn independently_created_projects_share_ancestry() {
        // the product case: two fresh instances meet in a collab room
        let mut a = Project::new("A");
        let mut b = Project::new("B");
        a.add_clip(&demo_clip("a1", "V1", 0.0)).unwrap();
        let mut sa = automerge::sync::State::new();
        let mut sb = automerge::sync::State::new();
        for _ in 0..20 {
            let ma = a.generate_sync_message(&mut sa);
            if let Some(m) = &ma {
                b.receive_sync_message(&mut sb, m).unwrap();
            }
            let mb = b.generate_sync_message(&mut sb);
            if let Some(m) = &mb {
                a.receive_sync_message(&mut sa, m).unwrap();
            }
            if ma.is_none() && mb.is_none() {
                break;
            }
        }
        assert_eq!(a.snapshot(), b.snapshot());
        assert_eq!(b.snapshot()["clips"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn two_projects_converge_over_sync_protocol() {
        let mut a = Project::new("Collab");
        a.add_clip(&demo_clip("a1", "V1", 0.0)).unwrap();
        let mut b = Project::load(&a.save()).unwrap();
        b.add_clip(&demo_clip("b1", "V2", 2.0)).unwrap();
        a.move_clip("a1", "V1", 5.0).unwrap();

        let mut sa = automerge::sync::State::new();
        let mut sb = automerge::sync::State::new();
        // ping-pong until quiescent
        for _ in 0..20 {
            let ma = a.generate_sync_message(&mut sa);
            if let Some(m) = &ma {
                b.receive_sync_message(&mut sb, m).unwrap();
            }
            let mb = b.generate_sync_message(&mut sb);
            if let Some(m) = &mb {
                a.receive_sync_message(&mut sa, m).unwrap();
            }
            if ma.is_none() && mb.is_none() {
                break;
            }
        }
        assert_eq!(a.snapshot(), b.snapshot());
        let snap = a.snapshot();
        assert_eq!(snap["clips"].as_array().unwrap().len(), 2);
        assert_eq!(snap["clips"][0]["id"], "a1");
        assert_eq!(snap["clips"][0]["start"], 5.0);
    }
}
