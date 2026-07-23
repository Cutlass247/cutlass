//! Verify retimed clips produce (non-silent) audio in live playback and
//! that the reader yields the exact timeline length at any speed.
//! `cargo run -p cutlass-engine --example retime_audio_check -- <audio-file>`
//! Defaults to a tone at %TEMP%/cutlass_tone.wav.

use cutlass_engine::player::{AudioClip, TrackReader};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        std::env::temp_dir()
            .join("cutlass_tone.wav")
            .to_string_lossy()
            .to_string()
    });
    let rate = 48_000u32;
    let len = 1.0f64; // 1s of timeline per case
    for speed in [1.0, 2.0, 0.5] {
        let clip = AudioClip { path: path.clone(), start: 0.0, len, src_in: 0.0, volume: 1.0, speed };
        let mut tr = TrackReader::new(vec![clip], rate, 0.0);
        let want_frames = (len * rate as f64) as usize;
        let mut got = 0usize;
        let mut peak = 0.0f32;
        while got < want_frames {
            let n = (want_frames - got).min(1024);
            let s = tr.read(n);
            assert_eq!(s.len(), n * 2, "reader must return exactly n stereo frames");
            for v in &s {
                peak = peak.max(v.abs());
            }
            got += n;
        }
        let status = if peak > 0.01 { "OK  " } else { "SILENT" };
        println!("{status} speed={speed:>4}  frames={got}  peak={peak:.3}");
        assert!(peak > 0.01, "retimed audio at {speed}x was silent");
    }
    println!("all speeds produced audio");
}
