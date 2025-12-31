use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use std::sync::Mutex;
use crate::status::{SyncStatusManagerRef, SyncPhase};

pub struct IndexOutput {
    mp: MultiProgress,
    load_bar: Mutex<Option<ProgressBar>>,
    index_bar: Mutex<Option<ProgressBar>>,
    status: SyncStatusManagerRef,
}

impl IndexOutput {
    pub fn new(status: SyncStatusManagerRef) -> Self {
        let mp = MultiProgress::new();

        Self { mp, load_bar: Mutex::new(None), index_bar: Mutex::new(None), status }
    }

    pub fn status(&self) -> &SyncStatusManagerRef {
        &self.status
    }

    fn create_bar(&self) -> ProgressBar {
        let bar = self.mp.add(ProgressBar::new(0));
        bar.set_style(
            ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {per_sec} {percent}% ({eta_precise} remaining) {msg}")
            .expect("Invalid progress bar template")
            .progress_chars("#>-"),
        );
        bar
    }

    pub fn println(&self, msg: &str) {
        if let Err(e) = self.mp.println(msg) {
            error!("Failed to print message to console: {}", e);
        }
        self.status.update_message(Some(msg.to_string()));
    }

    // Load methods
    pub fn start_load(&self, total: u64) {
        let bar = self.create_bar();
        bar.set_length(total);
        {
            let mut load_bar = self.load_bar.lock().unwrap();
            assert!(load_bar.is_none(), "Load bar already started");
            *load_bar = Some(bar);
        }

        self.status.update_phase(SyncPhase::Loading, Some("Starting block load".to_string()));
        self.status.update_total(total, None);
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
        let mut load_bar = self.load_bar.lock().unwrap();
        if let Some(bar) = load_bar.take() {
            bar.finish_with_message("Loading complete");
        }
        self.status.update_message(Some("Loading complete".to_string()));
    }

    // Index methods
    pub fn start_index(&self, total: u64, current: u64) {
        let bar: ProgressBar = self.create_bar();
        bar.set_length(total);

        for _ in 0..current {
            bar.inc(1);
        }

        let mut index_bar = self.index_bar.lock().unwrap();
        assert!(index_bar.is_none(), "Index bar already started");
        *index_bar = Some(bar);

        self.status.update_phase(SyncPhase::Indexing, Some("Starting indexer".to_string()));
        self.status.update_total(total, None);
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

        self.status.update_phase(SyncPhase::Synced, Some("Indexed complete".to_string()));
    }
}


pub type IndexOutputRef = std::sync::Arc<IndexOutput>;