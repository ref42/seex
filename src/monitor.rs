use arboard::Clipboard;
use chrono::Local;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::extract::extract_lcsc_ids;

pub type ExportProgressCallback = Arc<dyn Fn(usize, usize, &str) + Send + Sync>;

pub fn normalize_path_string(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn default_save_path(filename: &str) -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(filename)))
        .map(|p| normalize_path_string(&p.to_string_lossy()))
        .unwrap_or_else(|| filename.to_string())
}

#[derive(Clone, serde::Serialize)]
pub struct MatchedEntry {
    pub lcsc_id: String,
    pub first_seen: String,
    pub last_seen: String,
    pub count: usize,
    pub source_text: String,
}

#[derive(Clone)]
pub struct ExportProgressState {
    pub current: usize,
    pub total: usize,
    pub message: String,
}

pub struct MonitorState {
    pub last_content: String,
    pub history: Vec<(String, String)>,
    pub matched: Vec<MatchedEntry>,
    pub keyword: String,
    pub initialized: bool,
    pub monitoring: bool,
    pub match_debug_log: Vec<String>,
    pub export_target: String,
    pub export_output_path: String,
    pub export_last_result: Option<String>,
    pub export_running: bool,
    pub export_progress: Option<ExportProgressState>,
    pub export_mode: String,
    pub export_kicad_mode: String,
    pub export_cadence_mode: String,
    pub export_merge: bool,
    pub export_append: bool,
    pub export_library_name: String,
    pub export_parallel: usize,
    pub export_continue_on_error: bool,
    pub export_lcsc_english: bool,
    pub export_force: bool,
    /// Rebuild downloaded 3D models without the EasyEDA logo.
    pub export_remove_model_logo: bool,
    pub matched_save_path: String,
    pub bom_language: String,
    pub bom_running: bool,
    pub bom_last_result: Option<String>,
}

impl MonitorState {
    pub fn new() -> Self {
        Self {
            last_content: String::new(),
            history: Vec::new(),
            matched: Vec::new(),
            keyword: String::new(),
            initialized: false,
            monitoring: true,
            match_debug_log: Vec::new(),
            export_target: "altium".to_string(),
            export_output_path: "export_export".to_string(),
            export_last_result: None,
            export_running: false,
            export_progress: None,
            export_mode: "full".to_string(),
            export_kicad_mode: "full".to_string(),
            export_cadence_mode: "full".to_string(),
            export_merge: false,
            export_append: false,
            export_library_name: "SeExMerged".to_string(),
            export_parallel: 4,
            export_continue_on_error: false,
            export_lcsc_english: false,
            export_force: false,
            export_remove_model_logo: true,
            matched_save_path: default_save_path("matched.csv"),
            bom_language: "chinese".to_string(),
            bom_running: false,
            bom_last_result: None,
        }
    }

    pub fn add_debug_log(&mut self, msg: String) {
        self.match_debug_log.insert(0, msg);
        if self.match_debug_log.len() > 50 {
            self.match_debug_log.pop();
        }
    }

    pub fn set_keyword(&mut self, keyword: String) {
        self.add_debug_log(format!("Keyword set: [{}]", &keyword));
        self.keyword = keyword;
        self.rematch_history();
    }

    fn rematch_history(&mut self) {
        if self.keyword.is_empty() {
            return;
        }
        let existing: HashSet<String> = self
            .matched
            .iter()
            .map(|entry| entry.lcsc_id.clone())
            .collect();
        let mut new_matches: Vec<MatchedEntry> = Vec::new();
        for (time, content) in &self.history {
            for extracted in extract_lcsc_ids(content) {
                if !existing.contains(&extracted)
                    && !new_matches.iter().any(|entry| entry.lcsc_id == extracted)
                {
                    new_matches.push(MatchedEntry {
                        lcsc_id: extracted,
                        first_seen: time.clone(),
                        last_seen: time.clone(),
                        count: 1,
                        source_text: content.clone(),
                    });
                }
            }
        }
        if !new_matches.is_empty() {
            let count = new_matches.len();
            for item in new_matches.into_iter().rev() {
                self.matched.insert(0, item);
            }
            if self.matched.len() > 100 {
                self.matched.truncate(100);
            }
            self.add_debug_log(format!("Rematch: found {} new results from history", count));
        }
    }

    pub fn process_clipboard_change(&mut self, content: String) -> bool {
        if !self.monitoring {
            return false;
        }

        let trimmed = content.trim().to_string();
        if trimmed.is_empty() {
            return false;
        }

        if !self.initialized {
            // On initialization, check if content has IDs we care about
            let extracted_ids = extract_lcsc_ids(&trimmed);
            if !extracted_ids.is_empty() {
                // Initialize and process this content since it has valid IDs
                self.last_content = trimmed.clone();
                self.initialized = true;
                self.add_debug_log("Listener initialized with valid content".to_string());
                self.record_import(trimmed, "Initial content");
                return true;
            } else {
                // Just initialize without processing
                self.last_content = trimmed;
                self.initialized = true;
                self.add_debug_log("Listener initialized".to_string());
                return false;
            }
        }

        // Check if content has changed
        if trimmed == self.last_content {
            return false;
        }

        // Update last_content and process
        self.last_content = trimmed.clone();
        self.record_import(trimmed, "New content");
        true
    }

    /// Import text supplied explicitly by the user, independent of monitor state.
    pub fn import_text(&mut self, content: String) -> bool {
        let trimmed = content.trim().to_string();
        if trimmed.is_empty() {
            return false;
        }

        self.last_content = trimmed.clone();
        self.initialized = true;
        self.record_import(trimmed, "Manual import");
        true
    }

    fn record_import(&mut self, trimmed: String, source: &str) {
        let timestamp = Local::now().format("%H:%M:%S").to_string();
        self.add_debug_log(format!(
            "{}: {}",
            source,
            trimmed.chars().take(50).collect::<String>()
        ));

        self.history.insert(0, (timestamp.clone(), trimmed.clone()));
        if self.history.len() > 50 {
            self.history.pop();
        }

        let extracted_ids = extract_lcsc_ids(&trimmed);
        if extracted_ids.is_empty() {
            self.add_debug_log("No match found".to_string());
        } else {
            let mut new_count = 0usize;
            let mut existing_count = 0usize;
            for extracted in extracted_ids {
                if let Some(entry) = self
                    .matched
                    .iter_mut()
                    .find(|entry| entry.lcsc_id == extracted)
                {
                    entry.last_seen = timestamp.clone();
                    entry.count += 1;
                    entry.source_text = trimmed.clone();
                    existing_count += 1;
                } else {
                    self.matched.insert(
                        0,
                        MatchedEntry {
                            lcsc_id: extracted,
                            first_seen: timestamp.clone(),
                            last_seen: timestamp.clone(),
                            count: 1,
                            source_text: trimmed.clone(),
                        },
                    );
                    new_count += 1;
                    if self.matched.len() > 200 {
                        self.matched.pop();
                    }
                }
            }
            self.add_debug_log(format!(
                "Matched {} IDs: {} new, {} already known",
                new_count + existing_count,
                new_count,
                existing_count
            ));
        }
    }

    pub fn set_export_output_path(&mut self, path: String) {
        self.export_output_path = normalize_path_string(&path);
    }

    pub fn set_export_target(&mut self, target: String) {
        self.export_target = match target.trim().to_ascii_lowercase().as_str() {
            "kicad" => "kicad".to_string(),
            "cadence" => "cadence".to_string(),
            _ => "altium".to_string(),
        };
    }

    pub fn set_export_mode(&mut self, mode: String) {
        self.export_mode = match mode.trim().to_ascii_lowercase().as_str() {
            "schlib" => "schlib".to_string(),
            "pcblib" => "pcblib".to_string(),
            "3d" => "3d".to_string(),
            _ => "full".to_string(),
        };
    }

    pub fn set_export_kicad_mode(&mut self, mode: String) {
        self.export_kicad_mode = match mode.trim().to_ascii_lowercase().as_str() {
            "symbol" => "symbol".to_string(),
            "footprint" => "footprint".to_string(),
            "3d" => "3d".to_string(),
            _ => "full".to_string(),
        };
    }

    pub fn set_export_cadence_mode(&mut self, mode: String) {
        self.export_cadence_mode = match mode.trim().to_ascii_lowercase().as_str() {
            "symbol" => "symbol".to_string(),
            "footprint" => "footprint".to_string(),
            "3d" => "3d".to_string(),
            _ => "full".to_string(),
        };
    }

    pub fn set_export_remove_model_logo(&mut self, remove: bool) {
        self.export_remove_model_logo = remove;
    }

    pub fn set_export_merge(&mut self, merge: bool) {
        self.export_merge = merge;
        if !merge {
            self.export_append = false;
        }
    }

    pub fn set_export_append(&mut self, append: bool) {
        self.export_append = append;
        if append {
            self.export_merge = true;
        }
    }

    pub fn set_export_library_name(&mut self, library_name: String) {
        self.export_library_name = library_name;
    }

    pub fn set_export_parallel(&mut self, parallel: usize) {
        self.export_parallel = parallel.max(1);
    }

    pub fn set_export_continue_on_error(&mut self, continue_on_error: bool) {
        self.export_continue_on_error = continue_on_error;
    }

    pub fn set_export_lcsc_english(&mut self, lcsc_english: bool) {
        self.export_lcsc_english = lcsc_english;
    }

    pub fn set_export_force(&mut self, force: bool) {
        self.export_force = force;
    }

    pub fn set_matched_save_path(&mut self, path: String) {
        let trimmed = path.trim();
        self.matched_save_path = if trimmed.is_empty() {
            default_save_path("matched.csv")
        } else {
            normalize_path_string(trimmed)
        };
    }

    pub fn set_bom_language(&mut self, language: String) {
        self.bom_language = match language.trim().to_ascii_lowercase().as_str() {
            "english" => "english".to_string(),
            _ => "chinese".to_string(),
        };
        self.export_lcsc_english = self.bom_language == "english";
    }

    pub fn delete_history(&mut self, index: usize) {
        if index < self.history.len() {
            self.history.remove(index);
        }
    }

    pub fn delete_matched(&mut self, index: usize) {
        if index < self.matched.len() {
            self.matched.remove(index);
        }
    }

    pub fn clear_all(&mut self) {
        self.history.clear();
        self.matched.clear();
        // What is on the clipboard stays there, and the record of what has been
        // imported is kept so the monitor recognises it and leaves the cleared
        // lists alone. Forgetting that record was the original bug — everything
        // came straight back. Arming a "skip the next read" instead was worse:
        // if the user copied something new in the next instant, that new copy
        // was the read that got skipped, and it was never imported at all.
        self.initialized = true;
        self.match_debug_log.clear();
        self.export_last_result = None;
        self.export_running = false;
        self.export_progress = None;
        self.bom_last_result = None;
        self.bom_running = false;
    }

    /// Drops the import history, leaving the matched ids alone: the history is
    /// the log of what was on the clipboard, the matches are what the user keeps.
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    pub fn get_unique_ids(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.matched
            .iter()
            .filter_map(|entry| {
                let id = entry.lcsc_id.trim().to_ascii_uppercase();
                (!id.is_empty() && seen.insert(id.clone())).then_some(id)
            })
            .collect()
    }
}

pub struct MonitorHandle {
    stop: Arc<AtomicBool>,
    poll_thread: Option<JoinHandle<()>>,
}

impl MonitorHandle {
    pub fn spawn(state: Arc<Mutex<MonitorState>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let state_poll = Arc::clone(&state);
        let stop_poll = Arc::clone(&stop);

        let poll_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            let mut clipboard: Option<Clipboard> = None;
            let mut consecutive_errors = 0u32;

            while !stop_poll.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(300));

                if clipboard.is_none() {
                    clipboard = Clipboard::new().ok();
                    if clipboard.is_none() {
                        consecutive_errors += 1;
                        // Exponential backoff on repeated failures
                        if consecutive_errors > 3 {
                            thread::sleep(Duration::from_millis(1000));
                        }
                        continue;
                    }
                }

                if let Some(ref mut clip) = clipboard {
                    match clip.get_text() {
                        Ok(content) => {
                            consecutive_errors = 0;
                            if let Ok(mut s) = state_poll.lock() {
                                s.process_clipboard_change(content);
                            }
                        }
                        Err(_) => {
                            consecutive_errors += 1;
                            // Only reset clipboard handle after multiple consecutive errors
                            if consecutive_errors > 5 {
                                clipboard = None;
                            }
                        }
                    }
                }
            }
        });

        Self {
            stop,
            poll_thread: Some(poll_thread),
        }
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.poll_thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MatchedEntry, MonitorState, normalize_path_string};

    #[test]
    fn normalizes_windows_path_separators() {
        assert_eq!(
            normalize_path_string(r"C:\Users\Admin\Desktop\SeEx"),
            "C:/Users/Admin/Desktop/SeEx"
        );
    }

    #[test]
    fn unique_ids_normalize_duplicate_entries() {
        let mut state = MonitorState::new();
        state.matched = vec![
            MatchedEntry {
                lcsc_id: "C2040".to_string(),
                first_seen: String::new(),
                last_seen: String::new(),
                count: 1,
                source_text: String::new(),
            },
            MatchedEntry {
                lcsc_id: " c2040 ".to_string(),
                first_seen: String::new(),
                last_seen: String::new(),
                count: 1,
                source_text: String::new(),
            },
        ];

        assert_eq!(state.get_unique_ids(), vec!["C2040"]);
    }

    #[test]
    fn manual_import_works_when_monitoring_is_paused() {
        let mut state = MonitorState::new();
        state.monitoring = false;

        assert!(
            state.import_text(
                "C2040, https://lcsc.com/product-detail/C2927505.html C2040".to_string()
            )
        );
        assert_eq!(state.get_unique_ids(), vec!["C2927505", "C2040"]);
    }

    #[test]
    fn clearing_leaves_the_lists_empty_and_keeps_capturing() {
        let mut state = MonitorState::new();

        assert!(state.process_clipboard_change("C2040".to_string()));
        assert_eq!(state.get_unique_ids(), vec!["C2040"]);
        assert_eq!(state.history.len(), 1);

        state.clear_all();

        // The clipboard still holds what was cleared: polling it again must not
        // bring it back.
        assert!(!state.process_clipboard_change("C2040".to_string()));
        assert!(state.matched.is_empty(), "matched must stay empty");
        assert!(state.history.is_empty(), "history must stay empty");

        // A copy made right after clearing is a new sighting and must be
        // captured. Skipping the next read to make the clear stick swallowed
        // exactly this one, which is why capturing appeared to stop.
        assert!(state.process_clipboard_change("C4242".to_string()));
        assert_eq!(state.get_unique_ids(), vec!["C4242"]);
        assert_eq!(state.history.len(), 1);
    }

    #[test]
    fn a_deleted_match_stays_deleted_until_the_clipboard_changes() {
        let mut state = MonitorState::new();
        state.process_clipboard_change("C2040\nC4242".to_string());
        let matched = |state: &MonitorState, id: &str| {
            state.get_unique_ids().iter().any(|matched| matched == id)
        };
        assert!(matched(&state, "C2040") && matched(&state, "C4242"));

        let index = state
            .matched
            .iter()
            .position(|entry| entry.lcsc_id == "C2040")
            .expect("C2040 was matched");
        state.delete_matched(index);

        // The same clipboard is not read a second time, so the deletion holds.
        assert!(!state.process_clipboard_change("C2040\nC4242".to_string()));
        assert!(!matched(&state, "C2040"), "the deleted id must stay gone");
        assert!(matched(&state, "C4242"));

        // Copying it again is a new sighting, and that brings it back.
        assert!(state.process_clipboard_change("C2040".to_string()));
        assert!(matched(&state, "C2040"));
    }

    #[test]
    fn clearing_the_history_leaves_the_matched_ids_alone() {
        let mut state = MonitorState::new();
        state.process_clipboard_change("C2040".to_string());

        state.clear_history();

        assert!(state.history.is_empty());
        assert_eq!(state.get_unique_ids(), vec!["C2040"]);
        assert!(
            !state.process_clipboard_change("C2040".to_string()),
            "the same clipboard is not imported twice"
        );
    }
}
