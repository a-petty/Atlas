//! Real-time diagnostic logging to /tmp/atlas_diag.log.
//! Callers can `tail -f /tmp/atlas_diag.log` during long builds.

use std::fs::OpenOptions;
use std::io::Write;

const LOG_PATH: &str = "/tmp/atlas_diag.log";

/// Append a line to the diag log file and flush immediately.
pub fn diag_log(msg: &str) {
    eprintln!("{}", msg);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(LOG_PATH) {
        let _ = writeln!(f, "{}", msg);
        let _ = f.flush();
    }
}

/// Truncate the log file (call at start of a build).
pub fn diag_reset() {
    if let Ok(_) = OpenOptions::new().create(true).write(true).truncate(true).open(LOG_PATH) {}
}
