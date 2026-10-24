use std::path::PathBuf;
use std::sync::Arc;

use crate::cli::KicadVersion;
use crate::step_model::ModelCleanup;

#[derive(Debug, Clone)]
pub struct KicadExportOptions {
    pub symbol: bool,
    pub footprint: bool,
    pub model_3d: bool,
    pub full: bool,
    pub force: bool,
    pub kicad_version: KicadVersion,
    /// What happens to downloaded 3D models before they are written (logo removal).
    pub models: Arc<ModelCleanup>,
}

impl KicadExportOptions {
    pub fn targets_symbol(&self) -> bool {
        self.symbol || self.full
    }

    pub fn targets_footprint(&self) -> bool {
        self.footprint || self.full
    }

    pub fn targets_model_3d(&self) -> bool {
        self.model_3d || self.full
    }
}

#[derive(Debug, Clone)]
pub struct KicadLibraryOptions {
    pub output: PathBuf,
    pub library_name: Option<String>,
    pub symbol_lib: Option<PathBuf>,
    pub footprint_lib: Option<PathBuf>,
    pub model_lib: Option<PathBuf>,
}
