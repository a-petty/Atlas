// rust_core/src/watcher.rs

use crossbeam_channel::{bounded, Receiver, Sender};
use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{
    new_debouncer, DebounceEventResult,
};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};
use thiserror::Error;
use walkdir::WalkDir;


// Re-export for external use
pub use notify::EventKind;

/// Represents a file system change event
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChangeEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

/// Errors that can occur in the file watcher
#[derive(Debug, Error)]
pub enum WatcherError {
    #[error("Failed to initialize watcher: {0}")]
    InitializationError(String),
    
    #[error("Failed to watch path: {0}")]
    WatchError(String),
    
    #[error("Watcher already stopped")]
    AlreadyStopped,
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Statistics about watcher operation
#[derive(Debug, Default, Clone)]
pub struct WatcherStats {
    pub events_received: usize,
    pub events_filtered: usize,
    pub errors_encountered: usize,
    pub files_currently_watched: usize,
}

/// Configuration for filtering which files to watch
#[derive(Debug, Clone)]
pub struct FileFilter {
    /// File extensions to watch (e.g., "py", "rs")
    pub extensions: Vec<String>,
    
    /// Directory names to ignore (e.g., ".git", "__pycache__")
    pub ignored_dirs: Vec<String>,
    
    /// File names to ignore (e.g., ".DS_Store")
    pub ignored_files: Vec<String>,
}

impl Default for FileFilter {
    fn default() -> Self {
        Self {
            extensions: vec!["py".to_string(), "pyi".to_string()],
            ignored_dirs: vec![
                ".git".to_string(),
                "__pycache__".to_string(),
                "node_modules".to_string(),
                ".venv".to_string(),
                "venv".to_string(),
                ".pytest_cache".to_string(),
                ".mypy_cache".to_string(),
                "build".to_string(),
                "dist".to_string(),
                ".egg-info".to_string(),
            ],
            ignored_files: vec![
                ".DS_Store".to_string(),
                "Thumbs.db".to_string(),
                ".gitignore".to_string(),
            ],
        }
    }
}

impl FileFilter {
    /// Check if a path should be watched
    pub fn should_watch(&self, path: &Path) -> bool {
        // Check if in ignored directory
        for component in path.components() {
            if let std::path::Component::Normal(name) = component {
                let name_str = name.to_string_lossy();
                if self.ignored_dirs.iter().any(|d| name_str == d.as_str()) {
                    return false;
                }
            }
        }
        
        // Check if ignored file
        if let Some(file_name) = path.file_name() {
            let file_name_str = file_name.to_string_lossy();
            if self.ignored_files.iter().any(|f| file_name_str == f.as_str()) {
                return false;
            }
        }
        
        // Check extension
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy();
            self.extensions.iter().any(|e| ext_str == e.as_str())
        } else {
            false
        }
    }
}

/// File system watcher service
pub struct FileWatcher {
    /// Thread handling file system events
    watcher_thread: Option<JoinHandle<()>>,
    
    /// Channel for sending stop signal
    stop_sender: Option<Sender<()>>,
    
    /// Channel for receiving file change events
    event_receiver: Receiver<FileChangeEvent>,
    
    /// Statistics (wrapped in Arc for thread sharing)
    stats: Arc<RwLock<WatcherStats>>,
    
    /// File filter configuration
    filter: FileFilter,
    
    /// Whether the watcher is currently running
    is_running: Arc<RwLock<bool>>,

    /// Set of file paths we've seen (for distinguishing create vs modify)
    known_files: Arc<RwLock<HashSet<PathBuf>>>,
}

impl FileWatcher {
    /// Create and start a new file watcher
    /// 
    /// # Arguments
    /// * `watch_path` - Directory to watch
    /// * `filter` - File filter configuration (use Default::default() for sensible defaults)
    /// 
    /// # Returns
    /// Result containing FileWatcher or WatcherError
    pub fn new(watch_path: PathBuf, filter: FileFilter) -> Result<Self, WatcherError> {
        let (event_tx, event_rx) = bounded(4096);
        let (stop_tx, stop_rx) = bounded(1);
        let stats = Arc::new(RwLock::new(WatcherStats::default()));
        let is_running = Arc::new(RwLock::new(false));

        // Scan directory and populate initial known files
        let known_files = Arc::new(RwLock::new(HashSet::new()));
        Self::scan_initial_files(&watch_path, &filter, &known_files)?;
        
        let mut watcher = Self {
            watcher_thread: None,
            stop_sender: Some(stop_tx),
            event_receiver: event_rx,
            stats: stats.clone(),
            filter: filter.clone(),
            is_running: is_running.clone(),
            known_files: known_files.clone(),
        };
        
        watcher.start(watch_path, event_tx, stop_rx, stats, filter, is_running, known_files)?;
        
        Ok(watcher)
    }

    /// Scan directory to build initial known files set
    fn scan_initial_files(
        root: &Path,
        filter: &FileFilter,
        known_files: &Arc<RwLock<HashSet<PathBuf>>>,
    ) -> Result<(), WatcherError> {
        log::debug!("[scan_initial_files] Scanning: {:?}", root);
        
        let mut files = known_files.write();
        let mut scanned = 0;
        let mut matched = 0;
        
        for entry in WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            scanned += 1;
            let path = entry.path();
            
            if filter.should_watch(path) {
                matched += 1;
                // Canonicalize path before inserting into known_files
                if let Ok(canonical_path) = path.canonicalize() {
                    files.insert(canonical_path);
                } else {
                    log::warn!("[scan_initial_files] Warning: Could not canonicalize path: {:?}", path);
                }
            }
        }
        
        log::debug!("[scan_initial_files] Complete: scanned={}, matched={}", scanned, matched);
        
        Ok(())
    }
    
    /// Check if watcher is currently running
    pub fn is_running(&self) -> bool {
        *self.is_running.read()
    }
    
    /// Get current statistics
    pub fn get_stats(&self) -> WatcherStats {
        self.stats.read().clone()
    }
    
    /// Get all pending events (non-blocking)
    pub fn poll_events(&self) -> Vec<FileChangeEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_receiver.try_recv() {
            events.push(event);
        }
        events
    }
    
    /// Wait for next event (blocking, with timeout)
    pub fn wait_for_event(&self, timeout: Duration) -> Option<FileChangeEvent> {
        self.event_receiver.recv_timeout(timeout).ok()
    }
    
    /// Stop the watcher and wait for thread to finish
    pub fn stop(&mut self) -> Result<(), WatcherError> {
        if self.stop_sender.is_none() {
            return Err(WatcherError::AlreadyStopped);
        }
        
        // Send stop signal
        if let Some(sender) = self.stop_sender.take() {
            let _ = sender.send(()); // Ignore send errors (receiver might be gone)
        }
        
        // Wait for thread to finish
        if let Some(handle) = self.watcher_thread.take() {
            handle
                .join()
                .map_err(|_| WatcherError::InitializationError("Thread panicked".to_string()))?;
        }
        
        *self.is_running.write() = false;
        
        Ok(())
    }
}

impl FileWatcher {
    /// Internal: Start the watcher thread
    fn start(
        &mut self,
        watch_path: PathBuf,
        event_tx: Sender<FileChangeEvent>,
        stop_rx: Receiver<()>,
        stats: Arc<RwLock<WatcherStats>>,
        filter: FileFilter,
        is_running: Arc<RwLock<bool>>,
        known_files: Arc<RwLock<HashSet<PathBuf>>>,
    ) -> Result<(), WatcherError> {
        // Verify path exists
        if !watch_path.exists() {
            return Err(WatcherError::WatchError(format!(
                "Path does not exist: {:?}",
                watch_path
            )));
        }
        
        let watch_path_clone = watch_path.clone();
        
        // Channel to signal that the watcher thread has started successfully
        let (init_tx, init_rx) = bounded(1);

        let thread = thread::Builder::new()
            .name("file-watcher".to_string())
            .spawn(move || {
                Self::watcher_thread_main(
                    watch_path_clone,
                    event_tx,
                    stop_rx,
                    stats,
                    filter,
                    is_running,
                    init_tx,
                    known_files,
                );
            })
            .map_err(|e| WatcherError::InitializationError(e.to_string()))?;
        
        self.watcher_thread = Some(thread);
        
        // Block until the watcher thread signals that it has started
        init_rx.recv()
            .map_err(|_| WatcherError::InitializationError("Watcher thread failed to start".to_string()))??;

        Ok(())
    }
    
    /// Native events are hints. Periodic metadata reconciliation recovers
    /// missed events, including filesystems where native delivery is unavailable.
    fn manifest(root: &Path, filter: &FileFilter) -> HashMap<PathBuf, (u64, Option<SystemTime>)> {
        WalkDir::new(root).into_iter()
            .filter_entry(|e| !e.file_type().is_dir() || !filter.ignored_dirs.iter()
                .any(|d| e.file_name().to_string_lossy() == d.as_str()))
            .filter_map(Result::ok).filter(|e| e.file_type().is_file())
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                Some((entry.path().canonicalize().ok()?, (metadata.len(), metadata.modified().ok())))
            }).collect()
    }

    /// Main watcher thread loop
    fn watcher_thread_main(
        watch_path: PathBuf,
        event_tx: Sender<FileChangeEvent>,
        stop_rx: Receiver<()>,
        stats: Arc<RwLock<WatcherStats>>,
        filter: FileFilter,
        is_running: Arc<RwLock<bool>>,
        init_tx: Sender<Result<(), WatcherError>>,
        known_files: Arc<RwLock<HashSet<PathBuf>>>,
    ) {
        // Create event channel for debouncer
        let (debounce_tx, debounce_rx) = bounded(1024);
        
        // Create debouncer
        let mut debouncer = match new_debouncer(
            Duration::from_millis(100), // 100ms debounce
            None, 
            move |result: DebounceEventResult| {
                if let Err(e) = debounce_tx.try_send(result) {
                    log::warn!("[FileWatcher] Failed to send debounced event: {}", e);
                }
            },
        ) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("[FileWatcher] Failed to create debouncer: {}", e);
                let _ = init_tx.send(Err(WatcherError::InitializationError(e.to_string())));
                return;
            }
        };
        
        // Start watching
        if let Err(e) = debouncer.watcher().watch(&watch_path, RecursiveMode::Recursive) {
            log::warn!("[FileWatcher] Failed to watch path: {}", e);
            let _ = init_tx.send(Err(WatcherError::WatchError(e.to_string())));
            return;
        }
        
        let mut manifest = Self::manifest(&watch_path, &filter);
        let mut last_reconcile = Instant::now();
        stats.write().files_currently_watched = known_files.read().len();
        *is_running.write() = true;
        
        // Signal that the watcher has started successfully
        if init_tx.send(Ok(())).is_err() {
            log::warn!("[FileWatcher] Failed to signal start, receiver likely dropped.");
             *is_running.write() = false;
            return;
        }

        // Main event loop
        loop {
            // Check for stop signal (non-blocking)
            if stop_rx.try_recv().is_ok() {
                break;
            }
            
            if last_reconcile.elapsed() >= Duration::from_millis(200) {
                let next = Self::manifest(&watch_path, &filter);
                let mut changes = Vec::new();
                for (path, signature) in &next {
                    match manifest.get(path) {
                        None => changes.push(FileChangeEvent::Created(path.clone())),
                        Some(old) if old != signature => changes.push(FileChangeEvent::Modified(path.clone())),
                        _ => {}
                    }
                }
                for path in manifest.keys().filter(|p| !next.contains_key(*p)) {
                    changes.push(FileChangeEvent::Deleted(path.clone()));
                }
                let mut counts = stats.write();
                for change in changes {
                    let path = match &change {
                        FileChangeEvent::Created(p) | FileChangeEvent::Modified(p) | FileChangeEvent::Deleted(p) => p,
                        FileChangeEvent::Renamed { to, .. } => to,
                    };
                    let path = path.clone();
                    counts.events_received += 1;
                    if !filter.should_watch(&path) {
                        counts.events_filtered += 1;
                    } else {
                        match event_tx.try_send(change) {
                            Ok(()) => {},
                            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                                *is_running.write() = false; return;
                            }
                            Err(crossbeam_channel::TrySendError::Full(_)) => {
                                // Do not acknowledge a change the consumer has
                                // never received. The next scan retries it from
                                // this manifest once queue capacity is available.
                                counts.errors_encountered += 1;
                                break;
                            }
                        }
                        if next.contains_key(&path) { known_files.write().insert(path.clone()); }
                        else { known_files.write().remove(&path); }
                    }
                    if let Some(signature) = next.get(&path) { manifest.insert(path, *signature); }
                    else { manifest.remove(&path); }
                }
                counts.files_currently_watched = known_files.read().len();
                last_reconcile = Instant::now();
            }
            // Check for file events (with timeout)
            match debounce_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(result) => {
                    match result {
                        Ok(events) => {
                            let mut stats_guard = stats.write();
                            stats_guard.events_received += events.len();
                            
                            for event in events {
                                log::debug!("[event_loop] Event: kind={:?}, paths={}", event.kind, event.paths.len());
                                
                                for path in &event.paths {
                                    log::debug!("[event_loop] Processing: {:?}", path);
                                    
                                    // Filter unwanted files
                                    if !filter.should_watch(path) {
                                        stats_guard.events_filtered += 1;
                                        log::debug!("[event_loop] Filtered out: {:?}", path);
                                        continue;
                                    }
                                    
                                    log::debug!("[event_loop] Calling convert_event_stateful");
                                    
                                    if let Some(change_event) = Self::convert_event_stateful(&event.kind, path, &known_files) {
                                        log::debug!("[event_loop] Sending event: {:?}", change_event);
                                        if let Err(crossbeam_channel::TrySendError::Disconnected(_)) = event_tx.try_send(change_event) {
                                            *is_running.write() = false;
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                        Err(errors) => {
                            let mut stats_guard = stats.write();
                            stats_guard.errors_encountered += errors.len();
                            
                            for error in errors {
                                log::warn!("[FileWatcher] Error: {:?}", error);
                            }
                        }
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Normal timeout, continue loop
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    log::warn!("[FileWatcher] Event channel disconnected");
                    break;
                }
            }
        }
        
        *is_running.write() = false;
    }
    
    /// Convert notify event to our event type, using file tracking
    fn convert_event_stateful(
        kind: &EventKind,
        path: &Path,
        known_files: &Arc<RwLock<HashSet<PathBuf>>>,
    ) -> Option<FileChangeEvent> {
        use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
        
        log::debug!("[convert_event_stateful] kind={:?}, path={:?}", kind, path); // Log full path
        
        // Canonicalize the event path immediately for consistent checks
        let canonical_event_path = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                // If path cannot be canonicalized, it might be a deletion
                // Check if it was a known file that now doesn't exist.
                let mut files = known_files.write();
                if files.remove(path) { // Attempt to remove with original (potentially non-canonical) path
                    log::debug!("  → Result: Detected deletion (path no longer exists)");
                    return Some(FileChangeEvent::Deleted(path.to_path_buf()));
                }
                log::debug!("  → Result: Ignored (path cannot be canonicalized and not a known deleted file)");
                return None; // Cannot process, likely a transient state or invalid path
            }
        };

        let mut files = known_files.write(); // Get write lock once
        let was_known = files.contains(&canonical_event_path);

        match kind {
            EventKind::Create(CreateKind::File) 
            | EventKind::Create(CreateKind::Any) => {
                log::debug!("  → was_known={}, total_known={}", was_known, files.len());
                if was_known {
                    log::debug!("  → Result: Modified (atomic write or re-creation)");
                    Some(FileChangeEvent::Modified(canonical_event_path))
                } else {
                    log::debug!("  → Result: Created (new file)");
                    files.insert(canonical_event_path.clone());
                    Some(FileChangeEvent::Created(canonical_event_path))
                }
            }
            
            // Explicitly handle Modify events
            EventKind::Modify(ModifyKind::Data(_)) 
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Modify(ModifyKind::Metadata(_)) => {
                if was_known && !canonical_event_path.exists() { // If it was known and now doesn't exist
                    log::debug!("  → Result: Detected deletion (modify on non-existent file)");
                    files.remove(&canonical_event_path);
                    Some(FileChangeEvent::Deleted(canonical_event_path))
                } else {
                    log::debug!("  → Result: Modified");
                    Some(FileChangeEvent::Modified(canonical_event_path))
                }
            }
            
            EventKind::Remove(RemoveKind::File) 
            | EventKind::Remove(RemoveKind::Any) => {
                log::debug!("  → Result: Deleted");
                files.remove(&canonical_event_path);
                Some(FileChangeEvent::Deleted(canonical_event_path))
            }
            
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                // This typically means the file was renamed *from* 'path' or *to* 'path'.
                // If 'path' is the 'from' path, it's effectively a deletion of 'from'.
                // If 'path' is the 'to' path, it's effectively a creation of 'to'.
                // For simplicity, we'll treat this as a modification for now,
                // but a more robust solution might require examining event.paths to get both 'from' and 'to'.
                log::debug!("  → Result: Modified (rename)");
                Some(FileChangeEvent::Modified(canonical_event_path))
            }
            
            _ => {
                log::debug!("  → Result: Ignored");
                None
            }
        }
    }
}

/// Implement Drop to ensure clean shutdown
impl Drop for FileWatcher {
    fn drop(&mut self) {
        if self.is_running() {
            let _ = self.stop(); // Ignore errors on drop
        }
    }
}
