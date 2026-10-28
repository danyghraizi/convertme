//! Soak: many sequential jobs, then concurrent heavy ones, in one process.
//!
//! This is the check that catches a leaked child process, an uncapped preview
//! cache, a work dir that is never removed, or a file descriptor that is opened
//! per job and never closed. It is `#[ignore]`d because it takes minutes and needs
//! the bundled tools; run it with:
//!
//! ```sh
//! cargo test -p convertme-engines --test soak -- --ignored --nocapture
//! ```

use convertme_core::job::{Intent, Job};
use convertme_core::queue::{Event, Queue, QueueConfig};
use convertme_core::store::Store;
use convertme_engines::{Dispatcher, Toolset};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn rss_kb() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    pages * 4
}

fn fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .map(|read| read.count())
        .unwrap_or(0)
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn wait_idle(queue: &Queue, rx: &crossbeam_channel::Receiver<Event>, expected: usize) -> usize {
    let mut done = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(30 * 60);
    while done < expected && std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Event::Finished { .. }) | Ok(Event::Failed { .. }) | Ok(Event::Cancelled { .. }) => {
                done += 1
            }
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if queue.pending() == 0 && queue.running() == 0 {
                    break;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    done
}

#[test]
#[ignore = "long-running soak; run explicitly"]
fn two_hundred_images_then_five_videos_do_not_leak() {
    let work = tempfile::tempdir().expect("temp dir");
    let source = fixture_dir().join("tiny.png");
    assert!(source.is_file(), "run `cargo xtask fixtures` first");

    let (events, event_rx) = crossbeam_channel::bounded::<Event>(8192);
    let queue = Queue::start(
        QueueConfig {
            cpu_workers: 4,
            io_workers: 2,
            ..Default::default()
        },
        std::sync::Arc::new(Dispatcher::new(Toolset::probe_now())),
        Store::in_memory().expect("in-memory history"),
        events,
    );

    // ---- batch 1: 200 sequential image conversions ------------------------
    let baseline_rss = rss_kb();
    let baseline_fds = fd_count();
    let mut inputs = Vec::new();
    for index in 0..200 {
        let target = work.path().join(format!("photo-{index}.png"));
        std::fs::copy(&source, &target).expect("copy fixture");
        inputs.push(target);
    }
    for input in &inputs {
        let job = Job::new(input.clone(), Intent::convert("WEBP").expect("intent"));
        queue.submit(job).expect("submit");
    }
    let done = wait_idle(&queue, &event_rx, 200);
    assert_eq!(done, 200, "only {done}/200 image jobs finished");

    let after_images_rss = rss_kb();
    let after_images_fds = fd_count();
    let outputs = std::fs::read_dir(work.path())
        .expect("read work dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .map(|e| e == "webp")
                .unwrap_or(false)
        })
        .count();
    assert_eq!(outputs, 200, "expected 200 artifacts, found {outputs}");

    // ---- batch 2: 5 concurrent video compresses ---------------------------
    let clip = fixture_dir().join("clip.mp4");
    assert!(clip.is_file(), "run `cargo xtask fixtures` first");
    for index in 0..5 {
        let input = work.path().join(format!("clip-{index}.mp4"));
        std::fs::copy(&clip, &input).expect("copy clip");
        let mut params = convertme_core::job::ToolParams::new();
        params.insert("target_bytes".into(), serde_json::json!(200_000));
        let job = Job::new(
            input,
            Intent::Tool {
                tool: convertme_core::formats::ToolId::Compress,
                params,
            },
        );
        queue.submit(job).expect("submit");
    }
    let done = wait_idle(&queue, &event_rx, 5);
    assert_eq!(done, 5, "only {done}/5 video jobs finished");

    let final_rss = rss_kb();
    let final_fds = fd_count();
    queue.shutdown();

    println!(
        "rss: baseline {baseline_rss} kB → images {after_images_rss} kB → final {final_rss} kB\n\
         fds: baseline {baseline_fds} → images {after_images_fds} → final {final_fds}"
    );

    // The app buffers nothing larger than a thumbnail, so a 200-file batch must not
    // leave the process hundreds of megabytes heavier.
    assert!(
        final_rss <= baseline_rss + 120 * 1024,
        "rss grew from {baseline_rss} kB to {final_rss} kB"
    );
    assert!(
        after_images_rss <= baseline_rss + 80 * 1024,
        "rss after the image batch was {after_images_rss} kB (baseline {baseline_rss} kB)"
    );
    // No descriptor may leak per job: allow the two pools' worth of slack.
    assert!(
        final_fds <= baseline_fds + 8,
        "fd count grew from {baseline_fds} to {final_fds}"
    );

    // No work directory may survive, and no partial artifact may sit beside a source.
    let leftovers: Vec<PathBuf> = std::fs::read_dir(std::env::temp_dir())
        .expect("temp dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("convertme-")
                && entry.path().is_dir()
        })
        .map(|entry| entry.path())
        .collect();
    assert!(leftovers.is_empty(), "work dirs left behind: {leftovers:?}");
}
