use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const CONFIG_FILENAME: &str = "export_config.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(alias = "syft")]
    pub export: ExportConfig,
    pub monitor: MonitorConfig,
    pub ui_language: String,
    pub show_donate: bool,
    pub theme: String,
    /// The Roman-script face chosen on the Font page; `None` keeps the
    /// platform's own UI face.
    pub latin_font: Option<String>,
    /// The Han face, used as the fallback for CJK characters.
    pub han_font: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            export: ExportConfig::default(),
            monitor: MonitorConfig::default(),
            ui_language: "english".to_string(),
            show_donate: true,
            theme: "ocean".to_string(),
            latin_font: None,
            han_font: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    pub matched_save_path: String,
    pub bom_language: String,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            matched_save_path: String::new(),
            bom_language: "chinese".to_string(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportConfig {
    pub target: String,
    pub output_path: String,
    pub mode: String,
    pub kicad_mode: String,
    pub cadence_mode: String,
    pub merge: bool,
    pub append: bool,
    pub library_name: String,
    pub parallel: usize,
    pub continue_on_error: bool,
    pub lcsc_english: bool,
    pub force: bool,
    #[serde(alias = "schematic_color")]
    pub schematic_stroke_color: [u8; 3],
    /// `None` is the "no colour" choice: the symbol body is exported without a
    /// fill rather than painted in some default.
    pub schematic_fill_color: Option<[u8; 3]>,
    /// Rebuild downloaded 3D models without the EasyEDA logo; on unless turned off.
    pub remove_model_logo: bool,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            target: "altium".to_string(),
            output_path: "export".to_string(),
            mode: "full".to_string(),
            kicad_mode: "full".to_string(),
            cadence_mode: "full".to_string(),
            merge: false,
            append: false,
            library_name: "SeExMerged".to_string(),
            parallel: 4,
            continue_on_error: false,
            lcsc_english: false,
            force: false,
            // Preserve the existing Altium symbol colors as the initial choices.
            schematic_stroke_color: [11, 56, 104],
            schematic_fill_color: Some([255, 255, 224]),
            remove_model_logo: true,
        }
    }
}

impl AppConfig {
    fn config_path() -> PathBuf {
        ProjectDirs::from("com", "ref42", "SeExEgui")
            .map(|dirs| dirs.config_dir().join(CONFIG_FILENAME))
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join(CONFIG_FILENAME)))
            })
            .unwrap_or_else(|| PathBuf::from(CONFIG_FILENAME))
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, content);
        }
    }
}
