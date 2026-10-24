//! Cadence export: OrCAD Capture symbols and Allegro footprints, written as files the
//! user imports into Cadence, the way the KiCad and Altium targets write their
//! libraries.
//!
//! Capture's `.olb` and Allegro's `.dra` / `.psm` / `.pad` are undocumented binary
//! formats that only Cadence's own programs write, so SeEx writes the formats Cadence
//! reads in their place:
//!
//! * symbols as one Capture **Library XML** file, which Capture turns into an `.olb`
//!   with `File > Import > Library XML` (see [`capture`]);
//! * footprints as Allegro **SKILL** scripts, one per footprint plus a library script
//!   that builds them all into `.dra`, `.psm` and `.pad` files (see [`allegro`]);
//! * 3D models as STEP files, which the footprints map.
//!
//! Lands follow IPC-7352 where SeEx can calculate them (see [`ipc7352`]); other
//! footprints keep the manufacturer's lands with IPC-7352 courtyard and silkscreen
//! rules.

pub mod allegro;
pub mod capture;
pub mod ipc7352;
pub mod step_place;
pub mod workflow;

#[cfg(test)]
mod tests;

pub use workflow::{
    CadenceBatchOptions, CadenceBatchSummary, CadenceExportOptions, export_cadence_batch,
};
