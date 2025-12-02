use indicatif::{ProgressBar, ProgressStyle};

pub struct IndexOutput {
    bar: ProgressBar,
}

impl IndexOutput {
    pub fn new(block_height: u64) -> Self {
        let bar = ProgressBar::new(block_height);
        bar.set_style(
            ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .expect("Invalid progress bar template")
            .progress_chars("#>-"),
        );

        Self { bar }
    }

    pub fn update_total_block_height(&self, block_height: u64) {
        self.bar.set_length(block_height);
    }

    pub fn update_current_height(&self, current_height: u64) {
        self.bar.set_position(current_height);
    }

    pub fn set_message(&self, msg: &str) {
        self.bar.set_message(msg.to_string());
    }

    pub fn finish(&self) {
        self.bar.finish_with_message("Indexing complete");
    }
}


pub type IndexOutputRef = std::sync::Arc<IndexOutput>;