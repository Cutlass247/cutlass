//! The Cutlass project document — an Automerge CRDT, per Spike A.
//!
//! Locked-in rules from spikes/crdt-timeline/FINDINGS.md:
//! - Clips live in a MAP keyed by id; playback order is DERIVED from
//!   (track, start), never stored as list order.
//! - Moves write (track, start) atomically in one transaction so they
//!   merge as a unit.
//! - Conflicts are UI events, not errors (surfaced later via sync layer).

use automerge::{transaction::Transactable, AutoCommit, ObjId, ObjType, ReadDoc, ScalarValue, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let mut doc = AutoCommit::new();
        doc.put(automerge::ROOT, "name", name).expect("init name");
        doc.put_object(automerge::ROOT, "clips", ObjType::Map)
            .expect("init clips");
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

    pub fn add_clip(&mut self, clip: &Clip) -> anyhow::Result<()> {
        let clips = self.clips_obj();
        let obj = self.doc.put_object(&clips, &clip.id, ObjType::Map)?;
        self.doc.put(&obj, "name", clip.name.as_str())?;
        self.doc.put(&obj, "media", clip.media.as_str())?;
        self.doc.put(&obj, "track", clip.track.as_str())?;
        self.doc.put(&obj, "start", clip.start)?;
        self.doc.put(&obj, "len", clip.len)?;
        self.doc.put(&obj, "src_in", clip.src_in)?;
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
        Some(Clip {
            id: id.to_string(),
            name: get_str("name")?,
            media: get_str("media")?,
            track: get_str("track")?,
            start: get_f64("start")?,
            len: get_f64("len")?,
            src_in: get_f64("src_in")?,
        })
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
    fn save_load_roundtrip() {
        let mut p = Project::new("Trailer");
        p.add_clip(&demo_clip("a", "V1", 0.0)).unwrap();
        let bytes = p.save();
        let p2 = Project::load(&bytes).unwrap();
        assert_eq!(p2.snapshot(), p.snapshot());
    }
}
