//! The screenshot loop, run on a thread inside the app instead of a separate scheduled task. It
//! only captures: it never publishes, purges or reads the event store, and the engine's purge step
//! removes what it writes once the day is published.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono_tz::Tz;

use crate::screen_capture;
use crate::screenshot_rules::{Decision, ScreenshotSettings, decide};

/// A running daemon. Dropping it stops the loop at its next check.
pub struct Daemon {
    stop: Arc<AtomicBool>,
    last: Arc<Mutex<String>>,
    stopped: Arc<Mutex<Option<String>>>,
}

impl Daemon {
    pub fn start(
        settings: ScreenshotSettings,
        data_dir: PathBuf,
        timezone: Tz,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let last = Arc::new(Mutex::new("starting".to_owned()));
        let stopped = Arc::new(Mutex::new(None));
        let (thread_stop, thread_last, thread_stopped) = (stop.clone(), last.clone(), stopped.clone());
        std::thread::spawn(move || {
            let interval = Duration::from_secs(settings.interval_seconds.max(1));
            while !thread_stop.load(Ordering::Relaxed) {
                let (line, stopped) = iterate(&settings, &data_dir, timezone);
                *thread_last.lock().unwrap_or_else(|e| e.into_inner()) = line.clone();
                if stopped {
                    *thread_stopped.lock().unwrap_or_else(|e| e.into_inner()) = Some(line);
                    wake();
                    return;
                }
                wake();
                let mut slept = Duration::ZERO;
                while slept < interval && !thread_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                    slept += Duration::from_millis(500);
                }
            }
        });
        Self { stop, last, stopped }
    }

    /// Why the loop stopped itself, returned once so the app notifies once.
    pub fn take_stopped(&self) -> Option<String> {
        self.stopped.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// The last iteration, in the engine daemon's one-line format.
    pub fn last(&self) -> String {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// One iteration; returns the log line and whether the loop must stop.
fn iterate(settings: &ScreenshotSettings, data_dir: &std::path::Path, timezone: Tz) -> (String, bool) {
    let free = match screen_capture::free_megabytes(data_dir) {
        Ok(free) => free,
        Err(e) => return (format!("stopped: {e}"), true),
    };
    let window = screen_capture::active_window();
    let now = chrono::Utc::now().with_timezone(&timezone);
    match decide(settings, data_dir, free, &window, now) {
        Ok(Decision::Stopped(reason)) => (format!("stopped: {reason}"), true),
        Ok(Decision::Skipped(reason)) => (format!("{} skipped: {reason}", now.format("%H:%M:%S")), false),
        Ok(Decision::Capture(path)) => {
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .map_err(|e| e.to_string())
                .and_then(|()| screen_capture::capture_png())
                .and_then(|png| std::fs::write(&path, png).map_err(|e| e.to_string()));
            match written {
                Ok(()) => (
                    format!("{} captured -> {}", now.format("%H:%M:%S"), path.display()),
                    false,
                ),
                Err(e) => (format!("{} capture failed: {e}", now.format("%H:%M:%S")), false),
            }
        }
        Err(e) => (format!("stopped: {e}"), true),
    }
}
