use crate::status::{SyncPhase, SyncStatusManagerRef};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::Mutex;

pub struct IndexOutput {
    mp: MultiProgress,
    load_bar: Mutex<Option<ProgressBar>>,
    index_bar: Mutex<Option<ProgressBar>>,
    status: SyncStatusManagerRef,
}

impl IndexOutput {
    pub fn new(status: SyncStatusManagerRef) -> Self {
        let mp = MultiProgress::new();

        Self {
            mp,
            load_bar: Mutex::new(None),
            index_bar: Mutex::new(None),
            status,
        }
    }

    pub fn status(&self) -> &SyncStatusManagerRef {
        &self.status
    }

    fn create_bar(&self, prefix: &str, estimated_total: bool) -> ProgressBar {
        let bar = self.mp.add(ProgressBar::new(0));
        let style = if estimated_total {
            ProgressStyle::default_bar()
                .template("{prefix:.bold} {spinner:.green} [{elapsed_precise}] {pos} processed (~{len} estimated) {per_sec} {msg}")
                .expect("Invalid estimated progress template")
        } else {
            ProgressStyle::default_bar()
                .template("{prefix:.bold} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {per_sec} {percent}% ({eta_precise} remaining) {msg}")
                .expect("Invalid progress bar template")
                .progress_chars("#>-")
        };
        bar.set_style(style);
        bar.set_prefix(prefix.to_string());
        bar
    }

    pub fn println(&self, msg: &str) {
        info!("{}", msg);
        if let Err(e) = self.mp.println(msg) {
            error!("Failed to print message to console: {}", e);
        }
        self.status.update_message(Some(msg.to_string()));
    }

    pub fn eprintln(&self, msg: &str) {
        error!("{}", msg);
        if let Err(e) = self.mp.println(msg) {
            error!("Failed to print error message to console: {}", e);
        }
        self.status.update_message(Some(msg.to_string()));
    }

    // Load methods
    pub fn start_load(&self, total: u64) {
        self.start_load_bar("Load", total, false);
    }

    /// Starts a distinct load task whose total is an approximate source count.
    pub fn start_estimated_load_stage(&self, label: &str, estimated_total: u64) {
        self.start_load_bar(label, estimated_total, true);
    }

    fn start_load_bar(&self, label: &str, total: u64, estimated_total: bool) {
        let mut load_bar = self.load_bar.lock().unwrap();
        assert!(load_bar.is_none(), "Load bar already started");

        let bar = self.create_bar(label, estimated_total);
        bar.set_length(total);
        bar.set_position(0);
        bar.reset_elapsed();
        bar.reset_eta();
        *load_bar = Some(bar);
        drop(load_bar);

        self.status
            .update_phase(SyncPhase::Loading, Some(label.to_string()));
        self.status.update_status(0, total, Some(label.to_string()));
    }

    pub fn update_load_total_count(&self, total: u64) {
        let load_bar = self.load_bar.lock().unwrap();
        if let Some(bar) = load_bar.as_ref() {
            bar.set_length(total);
        }
        self.status.update_total(total, None);
    }

    pub fn update_load_current_count(&self, current: u64) {
        let load_bar = self.load_bar.lock().unwrap();
        if let Some(bar) = load_bar.as_ref() {
            bar.set_position(current);
        }
        self.status.update_current(current, None);
    }

    pub fn set_load_message(&self, msg: &str) {
        let load_bar = self.load_bar.lock().unwrap();
        if let Some(bar) = load_bar.as_ref() {
            bar.set_message(msg.to_string());
        }
        self.status.update_message(Some(msg.to_string()));
    }

    pub fn finish_load(&self) {
        self.finish_load_stage("Loading complete");
    }

    /// Finishes the active load task while retaining its completed line in the terminal.
    pub fn finish_load_stage(&self, message: &str) {
        let mut load_bar = self.load_bar.lock().unwrap();
        if let Some(bar) = load_bar.take() {
            bar.finish_with_message(message.to_string());
        }
        self.status.update_message(Some(message.to_string()));
    }

    // Index methods
    pub fn start_index(&self, total: u64, current: u64) {
        let bar: ProgressBar = self.create_bar("Index", false);
        bar.set_length(total);
        bar.set_position(current);
        bar.reset_eta();

        {
            let mut index_bar = self.index_bar.lock().unwrap();
            assert!(index_bar.is_none(), "Index bar already started");
            *index_bar = Some(bar);
        }

        self.status
            .update_phase(SyncPhase::Indexing, Some("Starting indexer".to_string()));
        self.status.update_total(total, None);
        self.status.update_current(current, None);
    }

    pub fn is_index_started(&self) -> bool {
        let index_bar = self.index_bar.lock().unwrap();
        index_bar.is_some()
    }

    pub fn update_total_block_height(&self, block_height: u64) {
        let index_bar = self.index_bar.lock().unwrap();
        if let Some(bar) = index_bar.as_ref() {
            bar.set_length(block_height);
        }

        self.status.update_total(block_height, None);
    }

    pub fn update_current_height(&self, current_height: u64) {
        let index_bar = self.index_bar.lock().unwrap();
        if let Some(bar) = index_bar.as_ref() {
            bar.set_position(current_height);
        }

        self.status.update_current(current_height, None);
    }

    pub fn set_index_message(&self, msg: &str) {
        let index_bar = self.index_bar.lock().unwrap();
        if let Some(bar) = index_bar.as_ref() {
            bar.set_message(msg.to_string());
        }

        self.status.update_message(Some(msg.to_string()));
    }

    pub fn finish_index(&self) {
        let mut index_bar = self.index_bar.lock().unwrap();
        if let Some(bar) = index_bar.take() {
            bar.finish_with_message("Indexing complete");
        }

        self.status
            .update_phase(SyncPhase::Synced, Some("Indexed complete".to_string()));
    }
}

pub type IndexOutputRef = std::sync::Arc<IndexOutput>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::SyncStatusManager;
    use std::sync::Arc;

    #[test]
    fn test_starting_new_load_stage_resets_progress_status() {
        let status = Arc::new(SyncStatusManager::new());
        let output = IndexOutput::new(status.clone());

        output.start_estimated_load_stage("First phase", 100);
        output.update_load_current_count(125);
        output.finish_load_stage("First phase complete");

        output.start_estimated_load_stage("Second phase", 500);
        let current = status.get_status();
        assert_eq!(current.phase, SyncPhase::Loading);
        assert_eq!(current.current, 0);
        assert_eq!(current.total, 500);
        assert_eq!(current.message.as_deref(), Some("Second phase"));

        output.update_load_current_count(10);
        output.finish_load_stage("Second phase complete");
        let completed = status.get_status();
        assert_eq!(completed.current, 10);
        assert_eq!(completed.total, 500);
        assert_eq!(completed.message.as_deref(), Some("Second phase complete"));
    }
}
