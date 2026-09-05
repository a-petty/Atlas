use semantic_engine::watcher::{FileChangeEvent, FileFilter, FileWatcher};
use std::fs;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;
use tempfile::tempdir;

/// Enhanced helper: Wait for specific event with comprehensive diagnostics
fn wait_for_event_matching<F>(
    watcher: &FileWatcher,
    timeout: Duration,
    predicate: F,
    test_name: &str,
) -> Option<FileChangeEvent>
where
    F: Fn(&FileChangeEvent) -> bool,
{
    let start = std::time::Instant::now();
    let mut all_events_seen = Vec::new();
    let mut poll_count = 0;
    
    println!("[{}] Starting event wait (timeout: {:?})", test_name, timeout);
    println!("[{}] Watcher running: {}", test_name, watcher.is_running());
    
    while start.elapsed() < timeout {
        poll_count += 1;
        let events = watcher.poll_events();
        
        if !events.is_empty() {
            println!("[{}] Poll #{}: received {} events", test_name, poll_count, events.len());
        }
        
        // Log all events for debugging
        for event in &events {
            println!("[{}]   Event: {:?}", test_name, event);
            all_events_seen.push(event.clone());
        }
        
        if let Some(event) = events.into_iter().find(|e| predicate(e)) {
            println!("[{}] ✅ FOUND matching event after {:?}", test_name, start.elapsed());
            return Some(event);
        }
        
        sleep(Duration::from_millis(50));
    }
    
    // DIAGNOSTIC OUTPUT ON FAILURE
    println!("\n[{}] ==================== FAILURE DIAGNOSTICS ====================", test_name);
    println!("[{}] ❌ TIMEOUT after {:?}", test_name, start.elapsed());
    println!("[{}] Total polls: {}", test_name, poll_count);
    println!("[{}] Total events seen: {}", test_name, all_events_seen.len());
    
    if all_events_seen.is_empty() {
        println!("[{}] 🚨 NO EVENTS RECEIVED AT ALL!", test_name);
        println!("[{}] Possible causes:", test_name);
        println!("[{}]   1. Watcher thread not running/crashed", test_name);
        println!("[{}]   2. All events being filtered out", test_name);
        println!("[{}]   3. Filesystem not generating events", test_name);
        println!("[{}]   4. Event conversion returning None", test_name);
    } else {
        println!("[{}] Events received but none matched predicate:", test_name);
        for (i, event) in all_events_seen.iter().enumerate() {
            println!("[{}]   Event {}: {:?}", test_name, i + 1, event);
        }
        println!("[{}] This means the predicate is too strict or events are wrong type", test_name);
    }
    
    let stats = watcher.get_stats();
    println!("[{}] Watcher statistics:", test_name);
    println!("[{}]   events_received: {}", test_name, stats.events_received);
    println!("[{}]   events_filtered: {}", test_name, stats.events_filtered);
    println!("[{}]   errors_encountered: {}", test_name, stats.errors_encountered);
    
    if stats.events_filtered > 0 {
        println!("[{}] ⚠️  Some events were filtered!", test_name);
    }
    if stats.errors_encountered > 0 {
        println!("[{}] ⚠️  Errors occurred during watching!", test_name);
    }
    
    println!("[{}] ============================================================\n", test_name);
    
    None
}

#[test]
fn test_watcher_creation_success() {
    println!("\n=== TEST: test_watcher_creation_success ===");
    let dir = tempdir().unwrap();
    let watcher = FileWatcher::new(dir.path().to_path_buf(), FileFilter::default());
    
    assert!(watcher.is_ok(), "Should create watcher successfully");
    
    let watcher = watcher.unwrap();
    assert!(watcher.is_running(), "Watcher should be running");
    println!("✅ Test passed\n");
}

#[test]
fn test_watcher_creation_invalid_path() {
    println!("\n=== TEST: test_watcher_creation_invalid_path ===");
    let invalid_path = PathBuf::from("/nonexistent/path/that/does/not/exist");
    let result = FileWatcher::new(invalid_path, FileFilter::default());
    
    assert!(result.is_err(), "Should fail with invalid path");
    println!("✅ Test passed\n");
}

#[test]
fn test_detect_file_creation() {
    println!("\n=== TEST: test_detect_file_creation ===");
    
    let dir = tempdir().unwrap();
    let root = dir.path();
    println!("[test_detect_file_creation] Test directory: {:?}", root);
    
    let watcher = FileWatcher::new(root.to_path_buf(), FileFilter::default()).unwrap();
    println!("[test_detect_file_creation] Watcher created successfully");
    println!("[test_detect_file_creation] Watcher running: {}", watcher.is_running());
    
    sleep(Duration::from_millis(1000));
    println!("[test_detect_file_creation] Initial wait complete");
    
    // Create a Python file
    let test_file = root.join("test_create.py");
    println!("[test_detect_file_creation] Creating file: {:?}", test_file);
    fs::write(&test_file, "# Test file").unwrap();
    println!("[test_detect_file_creation] File created on filesystem");
    println!("[test_detect_file_creation] File exists: {}", test_file.exists());

    // Canonicalize the path for comparison due to potential /var -> /private/var on macOS
    let canonical_test_file = test_file.canonicalize().unwrap();
    println!("[test_detect_file_creation] Canonical test file: {:?}", canonical_test_file);
    
    // Wait for create event
    let event = wait_for_event_matching(
        &watcher,
        Duration::from_secs(5),
        |e| matches!(e, FileChangeEvent::Created(p) if p == &canonical_test_file),
        "test_detect_file_creation",
    );
    
    assert!(event.is_some(), "Should detect file creation");
    println!("✅ Test passed\n");
}

#[test]
fn test_detect_file_modification() {
    println!("\n=== TEST: test_detect_file_modification ===");
    
    let dir = tempdir().unwrap();
    let root = dir.path();
    println!("[test_detect_file_modification] Test directory: {:?}", root);
    
    let test_file = root.join("test_modify.py");
    println!("[test_detect_file_modification] Creating initial file: {:?}", test_file);
    fs::write(&test_file, "# Initial content").unwrap();

    let canonical_test_file = test_file.canonicalize().unwrap();
    println!("[test_detect_file_modification] Canonical test file: {:?}", canonical_test_file);
    
    let watcher = FileWatcher::new(root.to_path_buf(), FileFilter::default()).unwrap();
    println!("[test_detect_file_modification] Watcher created successfully");
    
    sleep(Duration::from_millis(1000));
    println!("[test_detect_file_modification] Initial wait complete");
    
    // Clear any initial events
    let initial_events = watcher.poll_events();
    println!("[test_detect_file_modification] Cleared {} initial events", initial_events.len());
    
    // Modify file
    println!("[test_detect_file_modification] Modifying file");
    fs::write(&test_file, "# Modified content").unwrap();
    println!("[test_detect_file_modification] File modified on filesystem");
    
    // Wait for modify event
    let event = wait_for_event_matching(
        &watcher,
        Duration::from_secs(5),
        |e| matches!(e, FileChangeEvent::Modified(p) if p == &canonical_test_file),
        "test_detect_file_modification",
    );
    
    assert!(event.is_some(), "Should detect file modification");
    println!("✅ Test passed\n");
}

#[test]
fn test_detect_file_deletion() {
    println!("\n=== TEST: test_detect_file_deletion ===");
    
    let dir = tempdir().unwrap();
    let root = dir.path();
    println!("[test_detect_file_deletion] Test directory: {:?}", root);
    
    let test_file = root.join("test_delete.py");
    println!("[test_detect_file_deletion] Creating file: {:?}", test_file);
    fs::write(&test_file, "# To be deleted").unwrap();

    let canonical_test_file = test_file.canonicalize().unwrap();
    println!("[test_detect_file_deletion] Canonical test file: {:?}", canonical_test_file);
    
    let watcher = FileWatcher::new(root.to_path_buf(), FileFilter::default()).unwrap();
    println!("[test_detect_file_deletion] Watcher created successfully");
    
    sleep(Duration::from_millis(1000));
    println!("[test_detect_file_deletion] Initial wait complete");
    
    // Clear initial events
    let initial_events = watcher.poll_events();
    println!("[test_detect_file_deletion] Cleared {} initial events", initial_events.len());
    
    // Delete file
    println!("[test_detect_file_deletion] Deleting file");
    fs::remove_file(&test_file).unwrap();
    println!("[test_detect_file_deletion] File deleted from filesystem");
    println!("[test_detect_file_deletion] File exists: {}", test_file.exists());
    
    // Wait for delete event
    let event = wait_for_event_matching(
        &watcher,
        Duration::from_secs(5),
        |e| matches!(e, FileChangeEvent::Deleted(p) if p == &canonical_test_file),
        "test_detect_file_deletion",
    );
    
    assert!(event.is_some(), "Should detect file deletion");
    println!("✅ Test passed\n");
}

#[test]
fn test_file_filtering() {
    println!("\n=== TEST: test_file_filtering ===");
    
    let dir = tempdir().unwrap();
    let root = dir.path();
    println!("[test_file_filtering] Test directory: {:?}", root);
    
    let watcher = FileWatcher::new(root.to_path_buf(), FileFilter::default()).unwrap();
    println!("[test_file_filtering] Watcher created successfully");
    
    sleep(Duration::from_millis(1000));
    println!("[test_file_filtering] Initial wait complete");
    
    // Create Python file (should be detected)
    let py_file = root.join("test.py");
    println!("[test_file_filtering] Creating Python file: {:?}", py_file);
    fs::write(&py_file, "# Python").unwrap();

    let canonical_py_file = py_file.canonicalize().unwrap();
    println!("[test_file_filtering] Canonical Python file: {:?}", canonical_py_file);
    
    // Create non-Python file (should be filtered)
    let txt_file = root.join("test.txt");
    println!("[test_file_filtering] Creating text file: {:?}", txt_file);
    fs::write(&txt_file, "text").unwrap();

    let canonical_txt_file = txt_file.canonicalize().unwrap();
    println!("[test_file_filtering] Canonical text file: {:?}", canonical_txt_file);
    
    sleep(Duration::from_millis(1000));
    println!("[test_file_filtering] Waiting for events to settle");
    
    let events = watcher.poll_events();
    println!("[test_file_filtering] Received {} events total", events.len());
    
    for (i, event) in events.iter().enumerate() {
        println!("[test_file_filtering]   Event {}: {:?}", i + 1, event);
    }
    
    // Should only see Python file
    let py_events: Vec<_> = events.iter()
        .filter(|e| match e {
            FileChangeEvent::Created(p) | FileChangeEvent::Modified(p) => p == &canonical_py_file,
            _ => false,
        })
        .collect();
    
    println!("[test_file_filtering] Python file events: {}", py_events.len());
    
    let txt_events: Vec<_> = events.iter()
        .filter(|e| match e {
            FileChangeEvent::Created(p) | FileChangeEvent::Modified(p) => p == &canonical_txt_file,
            _ => false,
        })
        .collect();
    
    println!("[test_file_filtering] Text file events: {}", txt_events.len());
    
    let stats = watcher.get_stats();
    println!("[test_file_filtering] Stats: events_received={}, events_filtered={}",
             stats.events_received, stats.events_filtered);
    
    assert!(!py_events.is_empty(), "Should detect Python file");
    assert!(txt_events.is_empty(), "Should filter out txt file");
    println!("✅ Test passed\n");
}

#[test]
fn test_ignore_pycache() {
    println!("\n=== TEST: test_ignore_pycache ===");
    
    let dir = tempdir().unwrap();
    let root = dir.path();
    
    let watcher = FileWatcher::new(root.to_path_buf(), FileFilter::default()).unwrap();
    sleep(Duration::from_millis(200));
    
    // Create __pycache__ directory
    let pycache = root.join("__pycache__");
    fs::create_dir(&pycache).unwrap();
    
    // Create file in __pycache__
    let pyc_file = pycache.join("test.pyc");
    fs::write(&pyc_file, "bytecode").unwrap();
    
    sleep(Duration::from_millis(500));
    
    let events = watcher.poll_events();
    
    // Should not see __pycache__ files
    let pycache_events: Vec<_> = events.iter()
        .filter(|e| match e {
            FileChangeEvent::Created(p) | FileChangeEvent::Modified(p) => {
                p.to_string_lossy().contains("__pycache__")
            }
            _ => false,
        })
        .collect();
    
    assert!(pycache_events.is_empty(), "Should ignore __pycache__");
    println!("✅ Test passed\n");
}

#[test]
fn test_watcher_stop() {
    println!("\n=== TEST: test_watcher_stop ===");
    
    let dir = tempdir().unwrap();
    let mut watcher = FileWatcher::new(dir.path().to_path_buf(), FileFilter::default()).unwrap();
    
    assert!(watcher.is_running(), "Should be running initially");
    
    watcher.stop().expect("Should stop successfully");
    
    assert!(!watcher.is_running(), "Should not be running after stop");
    
    // Second stop should fail
    let result = watcher.stop();
    assert!(result.is_err(), "Second stop should return error");
    println!("✅ Test passed\n");
}

#[test]
fn test_watcher_statistics() {
    println!("\n=== TEST: test_watcher_statistics ===");
    
    let dir = tempdir().unwrap();
    let root = dir.path();
    
    let watcher = FileWatcher::new(root.to_path_buf(), FileFilter::default()).unwrap();
    sleep(Duration::from_millis(200));
    
    // Create some files
    for i in 0..3 {
        let file = root.join(format!("test_{}.py", i));
        fs::write(&file, format!("# File {}", i)).unwrap();
    }
    
    sleep(Duration::from_millis(500));
    
    let stats = watcher.get_stats();
    
    println!("Stats: {:?}", stats);
    
    assert!(stats.events_received > 0, "Should have received events");
    println!("✅ Test passed\n");
}

#[test]
fn test_debouncing() {
    println!("\n=== TEST: test_debouncing ===");
    
    let dir = tempdir().unwrap();
    let root = dir.path();
    
    let test_file = root.join("test_debounce.py");
    fs::write(&test_file, "# Initial").unwrap();
    
    let watcher = FileWatcher::new(root.to_path_buf(), FileFilter::default()).unwrap();
    sleep(Duration::from_millis(200));
    
    // Clear initial events
    let _ = watcher.poll_events();
    
    // Make multiple rapid modifications
    for i in 0..10 {
        fs::write(&test_file, format!("# Modification {}", i)).unwrap();
        sleep(Duration::from_millis(10));
    }
    
    // Wait for debouncing to settle
    sleep(Duration::from_millis(500));
    
    let events = watcher.poll_events();
    let modify_count = events.iter()
        .filter(|e| matches!(e, FileChangeEvent::Modified(_)))
        .count();
    
    println!("Received {} modify events for 10 changes", modify_count);
    
    // Should be significantly fewer than 10 due to debouncing
    assert!(modify_count < 10, "Should debounce rapid changes");
    println!("✅ Test passed\n");
}
fn overflow_event_path(event: FileChangeEvent) -> std::path::PathBuf {
    match event {
        FileChangeEvent::Created(p) | FileChangeEvent::Modified(p) | FileChangeEvent::Deleted(p) => p,
        FileChangeEvent::Renamed {to, ..} => to,
    }
}

#[test]
fn output_overflow_must_reconcile_after_consumer_resumes() {
    let root = tempfile::tempdir().unwrap();
    let mut watcher = FileWatcher::new(root.path().to_owned(), FileFilter::default()).unwrap();
    const FILES: usize = 6000;
    for i in 0..FILES {
        fs::write(root.path().join(format!("f_{i:05}.py")), b"x = 1\n").unwrap();
    }
    std::thread::sleep(Duration::from_secs(3));
    let first = watcher.poll_events();
    let first_events = first.len();
    let mut observed: std::collections::HashSet<_> = first.into_iter().map(overflow_event_path).collect();
    let mut later_events = 0;
    let resume = std::time::Instant::now();
    while resume.elapsed() < Duration::from_secs(3) {
        let events = watcher.poll_events();
        later_events += events.len();
        observed.extend(events.into_iter().map(overflow_event_path));
        std::thread::sleep(Duration::from_millis(50));
    }
    let stats = watcher.get_stats();
    let stop_start = std::time::Instant::now();
    watcher.stop().unwrap();
    println!("OVERFLOW files={FILES} first_events={first_events} later_events={later_events} unique_observed={} missing={} stats={stats:?} stop_ms={}", observed.len(), FILES-observed.len(), stop_start.elapsed().as_millis());
    assert_eq!(observed.len(), FILES, "Reconciliation must eventually surface every persistent file after output queue overflow");
}

#[test]
fn full_output_queue_stop_completes() {
    let root = tempfile::tempdir().unwrap();
    let mut watcher = FileWatcher::new(root.path().to_owned(), FileFilter::default()).unwrap();
    for i in 0..6000 {
        fs::write(root.path().join(format!("f_{i:05}.py")), b"x = 1\n").unwrap();
    }
    std::thread::sleep(Duration::from_secs(2));
    let start = std::time::Instant::now();
    watcher.stop().unwrap();
    println!("STOP full output queue completed in {} ms; buffered_events={}", start.elapsed().as_millis(), watcher.poll_events().len());
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!watcher.is_running());
}
