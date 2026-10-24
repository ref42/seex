//! SeEx: the clipboard monitor and EDA library exporter.
//!
//! The window is drawn with GPUI; everything under `src/ui/` is the front end,
//! and the pipeline modules below do the EasyEDA/LCSC → KiCad/Altium work.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod altium;
mod batch;
mod bom;
mod cadence;
mod cli;
mod config;
mod easyeda;
mod error;
mod exporter;
mod extract;
mod kicad;
mod lceda;
mod lcsc;
mod metadata;
mod monitor;
mod progress;
mod step_logo;
mod step_model;
mod ui;
mod util;

fn main() {
    ui::run();
}
