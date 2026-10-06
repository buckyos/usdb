//! Per-instance failpoints compiled only into indexer tests. No runtime environment switch.
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultPoint {
    EnergyPending,
    EventsApplied,
    EnergyWritten,
    EnergyFinalized,
    PassCommitWritten,
    TrackerPublished,
    HeightWritten,
    SqliteCommitted,
    ReorgPassRolledBack,
    ReorgEnergyRecovered,
    ReorgTrackerReloaded,
}

pub const PUBLICATION_POINTS: [FaultPoint; 8] = [
    FaultPoint::EnergyPending,
    FaultPoint::EventsApplied,
    FaultPoint::EnergyWritten,
    FaultPoint::EnergyFinalized,
    FaultPoint::PassCommitWritten,
    FaultPoint::TrackerPublished,
    FaultPoint::HeightWritten,
    FaultPoint::SqliteCommitted,
];
pub const REORG_POINTS: [FaultPoint; 3] = [
    FaultPoint::ReorgPassRolledBack,
    FaultPoint::ReorgEnergyRecovered,
    FaultPoint::ReorgTrackerReloaded,
];
pub const CRASH_EXIT_CODE: i32 = 86;

#[derive(Clone, Copy)]
pub enum FaultMode {
    Error,
    Crash,
}

#[derive(Default)]
pub struct PublicationFaults(Mutex<Option<(u32, FaultPoint, FaultMode)>>);
impl PublicationFaults {
    pub fn arm(&self, height: u32, point: FaultPoint, mode: FaultMode) {
        *self.0.lock().unwrap() = Some((height, point, mode));
    }
    pub fn hit(&self, height: u32, point: FaultPoint) -> Result<(), String> {
        let mut fault = self.0.lock().unwrap();
        if !fault
            .as_ref()
            .is_some_and(|(h, p, _)| *h == height && *p == point)
        {
            return Ok(());
        }
        let (_, _, mode) = fault.take().unwrap();
        drop(fault);
        match mode {
            FaultMode::Error => Err(format!(
                "Injected publication failure: block_height={height}, point={point:?}"
            )),
            // Unlike unwinding, exit leaves RocksDB WAL and SQLite crash recovery untouched.
            FaultMode::Crash => std::process::exit(CRASH_EXIT_CODE),
        }
    }
}
