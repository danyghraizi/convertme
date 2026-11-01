//! One controllable audio sink for the whole app.
//!
//! The web view must never decode audio: WebKitGTK's media stack crashed the process
//! on Play (LESSONS 61/66). Playback is therefore an external player, but with a
//! different lifecycle from the old "spawn and wait" call: starting one preview stops
//! the previous one, the dialog can stop/pause it, and closing the dialog stops it.
//! There is exactly one sink, so two preview buttons can never play over each other.

use convertme_engines::probe;
use parking_lot::Mutex;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The running player, with the generation that owns the slot.
type Slot = Arc<Mutex<Option<(u64, Child)>>>;

/// A single external audio player, replaceable and stoppable.
#[derive(Clone, Default)]
pub struct AudioSink {
    inner: Slot,
    generation: Arc<AtomicU64>,
}

impl AudioSink {
    /// Start playback at `start` seconds, stopping whatever was playing.
    ///
    /// Returns as soon as the player has started; the dialog drives its own clock and
    /// calls [`AudioSink::stop`] at the end of a trim or when it closes.
    pub fn play(&self, source: &Path, start: f64) -> Result<(), String> {
        self.stop();
        let (name, mut command) = player_command(source, start)?;
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {name}: {e}"))?;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.inner.lock() = Some((generation, child));

        // Reap a player that ends on its own; the slot is cleared only if it still
        // belongs to this generation (a newer Play must not be cleared by an old one).
        let inner = self.inner.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let mut slot = inner.lock();
                let Some((owner, child)) = slot.as_mut() else {
                    break;
                };
                if *owner != generation {
                    break;
                }
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => {
                        slot.take();
                        break;
                    }
                    Ok(None) => {}
                }
            }
        });
        Ok(())
    }

    /// Stop the player and wait for it, so no child is left behind.
    pub fn stop(&self) {
        let taken = self.inner.lock().take();
        if let Some((_generation, mut child)) = taken {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    #[cfg(test)]
    fn occupied(&self) -> bool {
        self.inner.lock().is_some()
    }
}

/// The argv for the first installed player. `ffplay` and `mpv` can start at `start`;
/// the simple sinks (`paplay`, `pw-play`, `aplay`) cannot seek, so a trimmed preview
/// asks for one of the two capable players rather than silently playing from zero.
fn player_command(source: &Path, start: f64) -> Result<(String, Command), String> {
    let player = ["ffplay", "mpv", "paplay", "pw-play", "aplay"]
        .iter()
        .find_map(|name| probe::which(name).map(|program| (*name, program)))
        .ok_or_else(|| {
            "no audio player is installed (ffplay, mpv, paplay, pw-play or aplay)".to_string()
        })?;
    let (name, program) = player;
    let start = if start.is_finite() {
        start.max(0.0)
    } else {
        0.0
    };
    let mut command = Command::new(program);
    match name {
        "ffplay" => {
            command.args(["-nodisp", "-autoexit", "-loglevel", "quiet"]);
            if start > 0.0 {
                command.arg("-ss").arg(format!("{start:.3}"));
            }
            command.arg(source);
        }
        "mpv" => {
            command.args(["--no-video", "--really-quiet", "--no-terminal"]);
            if start > 0.0 {
                command.arg(format!("--start={start:.3}"));
            }
            command.arg(source);
        }
        _ => {
            if start > 0.05 {
                return Err(format!(
                    "{name} cannot start in the middle of a file; install ffplay or mpv \
                     for trimmed previews"
                ));
            }
            command.arg(source);
        }
    }
    Ok((name.to_string(), command))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_player_starts_at_the_requested_second() {
        let source = Path::new("/tmp/convertme-sink-test.wav");
        let Ok((name, command)) = player_command(source, 4.5) else {
            return; // no player on this machine
        };
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        match name.as_str() {
            "ffplay" => {
                assert!(
                    args.windows(2).any(|pair| pair == ["-ss", "4.500"]),
                    "{args:?}"
                );
            }
            "mpv" => {
                assert!(args.iter().any(|arg| arg == "--start=4.500"), "{args:?}");
            }
            _ => {}
        }
        assert!(
            args.iter().any(|arg| arg == "/tmp/convertme-sink-test.wav"),
            "{args:?}"
        );
    }

    #[test]
    fn stop_is_idempotent_and_clears_the_slot() {
        let sink = AudioSink::default();
        assert!(!sink.occupied());
        sink.stop();
        sink.stop();
        assert!(!sink.occupied());
    }
}
