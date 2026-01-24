use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

const BAR_PREFIX: &[&str] = &["BTC", "Ordinals", "Balance History", "Index"];

pub struct IndexOutput {
    mp: MultiProgress,
    btc_bar: ProgressBar,
    ord_bar: ProgressBar,
    balance_history_bar: ProgressBar,
    index_bar: ProgressBar,
}

impl IndexOutput {
    pub fn new() -> Self {
        let mp = MultiProgress::new();

        let max_prefix_width = BAR_PREFIX.iter().map(|s| s.len()).max().unwrap_or(0);
        let btc_bar = Self::create_bar(format!(
            "{:<width$}",
            BAR_PREFIX[0],
            width = max_prefix_width
        ));
        let btc_bar = mp.add(btc_bar);

        let ord_bar = Self::create_bar(format!(
            "{:<width$}",
            BAR_PREFIX[1],
            width = max_prefix_width
        ));
        let ord_bar = mp.add(ord_bar);

        let balance_history_bar = Self::create_bar(format!(
            "{:<width$}",
            BAR_PREFIX[2],
            width = max_prefix_width
        ));
        let balance_history_bar = mp.add(balance_history_bar);

        let index_bar = Self::create_bar(format!(
            "{:<width$}",
            BAR_PREFIX[3],
            width = max_prefix_width
        ));
        let index_bar = mp.add(index_bar);

        Self {
            mp,
            btc_bar,
            ord_bar,
            balance_history_bar,
            index_bar,
        }
    }

    fn create_bar(prefix: String) -> ProgressBar {
        let bar = ProgressBar::new(0);
        bar.set_style(
            ProgressStyle::default_bar()
            .template("{prefix} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {per_sec} {percent}% ({eta_precise} remaining) {msg}")
            .expect("Invalid progress bar template")
            .progress_chars("#>-"),
        );
        bar.set_prefix(prefix);
        bar
    }

    pub fn println(&self, msg: &str) {
        info!("{}", msg);
        if let Err(e) = self.mp.println(msg) {
            error!("Failed to print message to console: {}", e);
        }
    }

    pub fn btc_bar(&self) -> &ProgressBar {
        &self.btc_bar
    }

    pub fn ord_bar(&self) -> &ProgressBar {
        &self.ord_bar
    }

    pub fn balance_history_bar(&self) -> &ProgressBar {
        &self.balance_history_bar
    }

    pub fn index_bar(&self) -> &ProgressBar {
        &self.index_bar
    }

    pub fn update_latest_block_height(&self, latest_block_height: u64) {
        self.btc_bar.set_length(latest_block_height);
        self.ord_bar.set_length(latest_block_height);
        self.balance_history_bar.set_length(latest_block_height);
        self.index_bar.set_length(latest_block_height);
    }
}

pub type IndexOutputRef = std::sync::Arc<IndexOutput>;
