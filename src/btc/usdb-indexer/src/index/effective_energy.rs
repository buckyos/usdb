use super::content::{MinerPassKind, MinerPassState};
use super::energy::{PassEnergyManagerRef, PassEnergyResult};
use super::energy_formula::{Energy, calc_collab_contribution, calc_standard_effective_energy};
use super::pass::MinerPassManagerRef;
use crate::storage::{MinerPassSnapshotInfo, MinerPassStorageRef, PassEnergyRecord};
use ord::InscriptionId;
use std::collections::BTreeSet;
use std::sync::Arc;
use usdb_util::USDBScriptHash;

/// Query mode used by derived UIP-0004 energy resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivedPassEnergyMode {
    /// Require a raw energy record exactly at the requested height.
    Exact,
    /// Use the latest raw record at or before the requested height and project it to that height.
    AtOrBefore,
}

/// Runtime-derived energy view for one pass at one BTC height.
///
/// This view keeps UIP-0003 raw energy separate from UIP-0004 effective energy:
/// collab contribution and effective energy are derived at query time and are
/// never written back to the raw energy ledger.
#[derive(Clone, Debug)]
pub struct DerivedPassEnergySnapshot {
    /// Stored raw energy record used as the query base.
    pub record: PassEnergyRecord,
    /// Pass state at the query height.
    pub state: MinerPassState,
    /// Pass kind at the query height.
    pub pass_kind: MinerPassKind,
    /// UIP-0003 raw energy at the query height.
    pub raw_energy: Energy,
    /// Aggregated UIP-0004 collab contribution at the query height.
    pub collab_contribution: Energy,
    /// UIP-0004 effective energy at the query height.
    pub effective_energy: Energy,
}

/// Read-only UIP-0004 effective energy resolver.
///
/// The resolver combines raw energy records, pass history snapshots, and
/// collab Leader resolution. It intentionally has no write path, so candidate
/// scoring and RPC reads cannot mutate the UIP-0003 raw energy database.
pub struct EffectiveEnergyResolver {
    pass_storage: MinerPassStorageRef,
    pass_energy_manager: PassEnergyManagerRef,
    miner_pass_manager: MinerPassManagerRef,
    collab_page_size: usize,
}

/// Shared reference to the runtime derived effective energy resolver.
pub type EffectiveEnergyResolverRef = Arc<EffectiveEnergyResolver>;

impl EffectiveEnergyResolver {
    /// Build a read-only resolver over pass storage, raw energy, and Leader resolution.
    pub fn new(
        pass_storage: MinerPassStorageRef,
        pass_energy_manager: PassEnergyManagerRef,
        miner_pass_manager: MinerPassManagerRef,
        collab_page_size: usize,
    ) -> Self {
        Self {
            pass_storage,
            pass_energy_manager,
            miner_pass_manager,
            collab_page_size: collab_page_size.max(1),
        }
    }

    /// Resolve raw, collab contribution, and effective energy for one pass.
    ///
    /// `raw_energy` follows the requested mode for the target pass. Collab
    /// contributions always use UIP-0003 raw energy at the same query height,
    /// resolved with at-or-before projection so a collab does not need to have
    /// an exact record at every query height.
    pub fn resolve_pass_energy(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
        mode: DerivedPassEnergyMode,
    ) -> Result<Option<DerivedPassEnergySnapshot>, String> {
        let Some((record, raw_result)) =
            self.resolve_target_raw_energy(inscription_id, block_height, mode)?
        else {
            return Ok(None);
        };

        let Some(pass_snapshot) = self
            .pass_storage
            .get_pass_snapshot_from_history_at_height(inscription_id, block_height)?
        else {
            let msg = format!(
                "Pass snapshot missing while resolving effective energy: inscription_id={}, block_height={}, record_height={}",
                inscription_id, block_height, record.block_height
            );
            error!("{}", msg);
            return Err(msg);
        };

        let state = pass_snapshot.pass.state.clone();
        if raw_result.state != state {
            let msg = format!(
                "Pass state and energy state mismatch while resolving effective energy: inscription_id={}, block_height={}, pass_state={}, energy_state={}, record_height={}",
                inscription_id,
                block_height,
                state.as_str(),
                raw_result.state.as_str(),
                record.block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let pass_kind = pass_snapshot.pass.pass_kind;
        let (collab_contribution, effective_energy) =
            if state == MinerPassState::Active && pass_kind == MinerPassKind::Standard {
                let collab_contribution = self.resolve_standard_collab_contribution(
                    inscription_id,
                    &pass_snapshot.pass.owner,
                    block_height,
                )?;
                (
                    collab_contribution,
                    calc_standard_effective_energy(raw_result.energy, collab_contribution),
                )
            } else {
                (0, 0)
            };

        Ok(Some(DerivedPassEnergySnapshot {
            record,
            state,
            pass_kind,
            raw_energy: raw_result.energy,
            collab_contribution,
            effective_energy,
        }))
    }

    fn resolve_target_raw_energy(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
        mode: DerivedPassEnergyMode,
    ) -> Result<Option<(PassEnergyRecord, PassEnergyResult)>, String> {
        let record = match mode {
            DerivedPassEnergyMode::Exact => self
                .pass_energy_manager
                .get_pass_energy_record_exact(inscription_id, block_height)?,
            DerivedPassEnergyMode::AtOrBefore => self
                .pass_energy_manager
                .get_pass_energy_record_at_or_before(inscription_id, block_height)?,
        };
        let Some(record) = record else {
            return Ok(None);
        };

        let raw_result = match mode {
            DerivedPassEnergyMode::Exact => PassEnergyResult {
                energy: record.energy,
                state: record.state.clone(),
            },
            DerivedPassEnergyMode::AtOrBefore => self
                .pass_energy_manager
                .project_energy_record_no_balance_change(&record, block_height),
        };

        Ok(Some((record, raw_result)))
    }

    fn resolve_standard_collab_contribution(
        &self,
        leader_pass_id: &InscriptionId,
        leader_owner: &USDBScriptHash,
        block_height: u32,
    ) -> Result<Energy, String> {
        let mut seen_collabs = BTreeSet::new();
        let mut aggregate: Energy = 0;

        self.accumulate_collab_contribution_pages(
            |page| {
                self.pass_storage
                    .get_active_collab_passes_by_leader_pass_id_from_history_at_height(
                        page,
                        self.collab_page_size,
                        block_height,
                        leader_pass_id,
                    )
            },
            leader_pass_id,
            block_height,
            &mut seen_collabs,
            &mut aggregate,
            "leader_pass_id",
        )?;

        self.accumulate_collab_contribution_pages(
            |page| {
                self.pass_storage
                    .get_active_collab_passes_by_leader_btc_owner_from_history_at_height(
                        page,
                        self.collab_page_size,
                        block_height,
                        leader_owner,
                    )
            },
            leader_pass_id,
            block_height,
            &mut seen_collabs,
            &mut aggregate,
            "leader_btc_owner",
        )?;

        Ok(aggregate)
    }

    fn accumulate_collab_contribution_pages<F>(
        &self,
        mut load_page: F,
        leader_pass_id: &InscriptionId,
        block_height: u32,
        seen_collabs: &mut BTreeSet<InscriptionId>,
        aggregate: &mut Energy,
        source: &'static str,
    ) -> Result<(), String>
    where
        F: FnMut(usize) -> Result<Vec<MinerPassSnapshotInfo>, String>,
    {
        let mut page = 0usize;

        loop {
            let collabs = load_page(page)?;
            if collabs.is_empty() {
                break;
            }

            for collab_snapshot in &collabs {
                if !seen_collabs.insert(collab_snapshot.pass.inscription_id) {
                    continue;
                }

                let Some(resolved_leader) = self
                    .miner_pass_manager
                    .resolve_collab_leader_at_height(&collab_snapshot.pass, block_height)?
                else {
                    continue;
                };
                if resolved_leader.leader.pass.inscription_id != *leader_pass_id {
                    continue;
                }

                let Some(collab_raw) = self.resolve_raw_energy_at_or_before(
                    &collab_snapshot.pass.inscription_id,
                    block_height,
                )?
                else {
                    let msg = format!(
                        "Active collab pass missing raw energy while resolving Leader contribution: collab_inscription_id={}, leader_inscription_id={}, block_height={}",
                        collab_snapshot.pass.inscription_id, leader_pass_id, block_height
                    );
                    error!("{}", msg);
                    return Err(msg);
                };
                if collab_raw.state != MinerPassState::Active {
                    let msg = format!(
                        "Active collab pass energy state mismatch while resolving Leader contribution: collab_inscription_id={}, leader_inscription_id={}, block_height={}, energy_state={}",
                        collab_snapshot.pass.inscription_id,
                        leader_pass_id,
                        block_height,
                        collab_raw.state.as_str()
                    );
                    error!("{}", msg);
                    return Err(msg);
                }

                *aggregate =
                    (*aggregate).saturating_add(calc_collab_contribution(collab_raw.energy));
            }

            if collabs.len() < self.collab_page_size {
                break;
            }
            page = page.checked_add(1).ok_or_else(|| {
                let msg = format!(
                    "Collab pagination overflow while resolving effective energy: leader_inscription_id={}, block_height={}, source={}",
                    leader_pass_id, block_height, source
                );
                error!("{}", msg);
                msg
            })?;
        }

        Ok(())
    }

    fn resolve_raw_energy_at_or_before(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<PassEnergyResult>, String> {
        let Some(record) = self
            .pass_energy_manager
            .get_pass_energy_record_at_or_before(inscription_id, block_height)?
        else {
            return Ok(None);
        };
        Ok(Some(
            self.pass_energy_manager
                .project_energy_record_no_balance_change(&record, block_height),
        ))
    }
}
