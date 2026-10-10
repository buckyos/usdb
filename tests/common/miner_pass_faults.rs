//! Per-instance failpoints compiled only into indexer tests. No runtime environment switch.
use std::sync::{Mutex, mpsc};
use std::time::Duration;

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

struct PublicationPause {
    height: u32,
    point: FaultPoint,
    entered: mpsc::SyncSender<()>,
    resume: mpsc::Receiver<()>,
}

#[derive(Default)]
pub struct PublicationFaults {
    fault: Mutex<Option<(u32, FaultPoint, FaultMode)>>,
    pause: Mutex<Option<PublicationPause>>,
}
impl PublicationFaults {
    pub fn arm(&self, height: u32, point: FaultPoint, mode: FaultMode) {
        *self.fault.lock().unwrap() = Some((height, point, mode));
    }
    /// Pause outside storage locks so a real concurrent RPC can inspect the committed prefix.
    pub fn pause(
        &self,
        height: u32,
        point: FaultPoint,
    ) -> (mpsc::Receiver<()>, mpsc::SyncSender<()>) {
        let (entered, observed) = mpsc::sync_channel(1);
        let (resume, continued) = mpsc::sync_channel(1);
        *self.pause.lock().unwrap() = Some(PublicationPause {
            height,
            point,
            entered,
            resume: continued,
        });
        (observed, resume)
    }
    pub fn hit(&self, height: u32, point: FaultPoint) -> Result<(), String> {
        let pause = {
            let mut pending = self.pause.lock().unwrap();
            if pending
                .as_ref()
                .is_some_and(|p| p.height == height && p.point == point)
            {
                pending.take()
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            pause
                .entered
                .send(())
                .map_err(|e| format!("Publication pause observer lost: {e}"))?;
            pause
                .resume
                .recv_timeout(Duration::from_secs(20))
                .map_err(|e| {
                    format!(
                        "Publication pause not resumed: height={height}, point={point:?}, error={e}"
                    )
                })?;
        }
        let mut fault = self.fault.lock().unwrap();
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
