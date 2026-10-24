use crate::altium::workflow::download_step;
use crate::batch::{BatchOptions, BatchSummary, export_batch};
use crate::cadence::{
    CadenceBatchOptions, CadenceBatchSummary, CadenceExportOptions, export_cadence_batch,
};
use crate::cli::KicadVersion;
use crate::kicad::{
    KicadBatchOptions, KicadBatchSummary, KicadExportOptions, KicadLibraryOptions,
    export_kicad_batch,
};
use crate::lceda::LcedaClient;
use crate::step_model::ModelCleanup;
use futures_util::stream::{self, StreamExt};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::monitor::{ExportProgressCallback, ExportProgressState, MonitorState};

const DEFAULT_LIBRARY_NAME: &str = "SeExMerged";
const DEFAULT_OUTPUT_DIR: &str = "export_export";

pub struct ExportRequest {
    pub ids: Vec<String>,
    pub target: String,
    pub output_path: String,
    pub mode: String,
    pub merge: bool,
    pub append: bool,
    pub library_name: String,
    pub parallel: usize,
    pub continue_on_error: bool,
    pub lcsc_english: bool,
    pub force: bool,
    pub schematic_stroke_color: [u8; 3],
    pub schematic_fill_color: Option<[u8; 3]>,
    /// Rebuild downloaded 3D models without the EasyEDA logo (on by default).
    pub remove_model_logo: bool,
}

pub fn spawn_export(state: Arc<Mutex<MonitorState>>, req: ExportRequest) {
    if let Ok(mut s) = state.lock() {
        s.export_running = true;
        s.export_last_result = None;
        s.export_progress = Some(ExportProgressState {
            current: 0,
            total: req.ids.len(),
            message: "Preparing export export...".to_string(),
        });
    }

    std::thread::spawn(move || {
        let started_at = Instant::now();
        let models = Arc::new(ModelCleanup::new(req.remove_model_logo));
        let input_path = if is_altium_target(&req) {
            Some(create_temp_input_path())
        } else {
            None
        };
        let result: Result<String, String> = (|| {
            if let Some(path) = &input_path {
                write_ids_file(path, &req.ids)?;
            }
            set_progress(&state, running_message(&req), 0, req.ids.len());
            if is_altium_target(&req) {
                if normalize_altium_mode(&req.mode) == "3d" {
                    return run_altium_3d_api(&req, &state, started_at, &models);
                }
                let input_path = input_path
                    .as_ref()
                    .ok_or_else(|| "missing export batch input path".to_string())?;
                return run_altium_api(
                    &req,
                    input_path,
                    started_at,
                    progress_callback(state.clone()),
                    &models,
                );
            }

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|err| err.to_string())?;
            runtime.block_on(async {
                if is_cadence_target(&req) {
                    let summary = export_cadence_batch(build_cadence_batch_options(
                        &req,
                        Some(progress_callback(state.clone())),
                        &models,
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
                    return Ok(format_cadence_summary(&req, &summary, started_at.elapsed()));
                }
                let summary = export_kicad_batch(build_kicad_batch_options(
                    &req,
                    Some(progress_callback(state.clone())),
                    &models,
                ))
                .await
                .map_err(|e| e.to_string())?;
                Ok(format_kicad_summary(&req, &summary, started_at.elapsed()))
            })
        })();

        if let Some(path) = &input_path {
            let _ = fs::remove_file(path);
        }

        let mut message = match result {
            Ok(message) => message,
            Err(err) => format_error(&req, &err, started_at.elapsed()),
        };
        if exports_models(&req) {
            for line in models.summary_lines() {
                message.push('\n');
                message.push_str(&line);
            }
        }

        if let Ok(mut s) = state.lock() {
            s.export_running = false;
            s.export_progress = None;
            s.export_last_result = Some(message.clone());
            s.add_debug_log(message);
        }
    });
}

fn set_progress(
    state: &Arc<Mutex<MonitorState>>,
    message: impl Into<String>,
    current: usize,
    total: usize,
) {
    if let Ok(mut s) = state.lock() {
        s.export_progress = Some(ExportProgressState {
            current,
            total,
            message: message.into(),
        });
    }
}

fn running_message(req: &ExportRequest) -> String {
    let count = req.ids.len();
    if is_kicad_target(req) {
        format!("Running export KiCad batch for {} items...", count)
    } else if is_cadence_target(req) {
        format!("Running Cadence export for {} items...", count)
    } else if normalize_altium_mode(&req.mode) == "3d" {
        format!("Downloading Altium 3D models for {} items...", count)
    } else if req.merge && req.append {
        format!("Appending into merged export library ({} items)...", count)
    } else if req.merge {
        format!("Merging {} export items...", count)
    } else {
        format!("Running export batch for {} items...", count)
    }
}

fn create_temp_input_path() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "seex_export_ids_{}_{}.txt",
        std::process::id(),
        stamp
    ))
}

fn write_ids_file(path: &PathBuf, ids: &[String]) -> Result<(), String> {
    let mut file = BufWriter::new(fs::File::create(path).map_err(|e| e.to_string())?);
    for id in ids {
        writeln!(file, "{}", id).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())
}

fn run_altium_api(
    req: &ExportRequest,
    input_path: &Path,
    started_at: Instant,
    progress: ExportProgressCallback,
    models: &Arc<ModelCleanup>,
) -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime.block_on(async {
        let client = LcedaClient::new();
        let summary = export_batch(
            &client,
            build_batch_options(req, &input_path.to_path_buf(), Some(progress), models),
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(format_altium_summary(req, &summary, started_at.elapsed()))
    })
}

fn run_altium_3d_api(
    req: &ExportRequest,
    state: &Arc<Mutex<MonitorState>>,
    started_at: Instant,
    models: &Arc<ModelCleanup>,
) -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime.block_on(async {
        let client = LcedaClient::new();
        let output = PathBuf::from(normalize_output_path(&req.output_path));
        let total = req.ids.len();
        let force = req.force;
        let parallel = req.parallel.max(1);
        let mut jobs = stream::iter(req.ids.clone().into_iter().map(|id| {
            let client = client.clone();
            let output = output.clone();
            let models = Arc::clone(models);
            async move {
                let result = async {
                    let item = client.select_item(&id, 1).await?;
                    download_step(&client, &item, &output, force, &models).await
                }
                .await;
                (id, result)
            }
        }))
        .buffer_unordered(parallel);

        let mut success = 0usize;
        let mut failed = Vec::new();
        let mut completed = 0usize;
        while let Some((id, result)) = jobs.next().await {
            completed += 1;
            match result {
                Ok(_) => success += 1,
                Err(error) => failed.push(format!("{id}: {error}")),
            }
            set_progress(
                state,
                format!("Downloading 3D models: {completed}/{total}"),
                completed,
                total,
            );
        }

        let mut lines = vec![
            if failed.is_empty() {
                "Status: Success".to_string()
            } else {
                "Status: Completed with errors".to_string()
            },
            "Target: Altium".to_string(),
            format!("Elapsed: {}", format_duration(started_at.elapsed())),
            format!("3D models: success {} | failed {}", success, failed.len()),
            "Targets: 3D".to_string(),
            format!("Output directory: {}", output.display()),
            format!("Parallel: {parallel}"),
        ];
        if !failed.is_empty() {
            lines.push(format!("Failed IDs: {}", failed.join(", ")));
        }
        Ok(lines.join("\n"))
    })
}

fn build_batch_options(
    req: &ExportRequest,
    input_path: &PathBuf,
    progress: Option<ExportProgressCallback>,
    models: &Arc<ModelCleanup>,
) -> BatchOptions {
    let mode = normalize_altium_mode(&req.mode);
    let append = req.append && req.merge;
    BatchOptions {
        input: input_path.clone(),
        output: PathBuf::from(normalize_output_path(&req.output_path)),
        schlib: mode == "schlib",
        pcblib: mode == "pcblib",
        full: mode == "full",
        merge: req.merge,
        append,
        library_name: effective_library_name(req),
        parallel: req.parallel.max(1),
        continue_on_error: req.continue_on_error,
        english_metadata: req.lcsc_english,
        force: req.force,
        schematic_stroke_color: req.schematic_stroke_color,
        schematic_fill_color: req.schematic_fill_color,
        models: Arc::clone(models),
        progress,
    }
}

fn build_kicad_batch_options(
    req: &ExportRequest,
    progress: Option<ExportProgressCallback>,
    models: &Arc<ModelCleanup>,
) -> KicadBatchOptions {
    let mode = normalize_kicad_mode(&req.mode);
    KicadBatchOptions {
        ids: req.ids.clone(),
        library: KicadLibraryOptions {
            output: PathBuf::from(normalize_output_path(&req.output_path)),
            library_name: Some(resolve_library_name(req)),
            symbol_lib: None,
            footprint_lib: None,
            model_lib: None,
        },
        export: KicadExportOptions {
            symbol: mode == "symbol",
            footprint: mode == "footprint",
            model_3d: mode == "3d",
            full: mode == "full",
            force: req.force,
            kicad_version: KicadVersion::V6,
            models: Arc::clone(models),
        },
        english_metadata: req.lcsc_english,
        parallel: req.parallel.max(1),
        continue_on_error: req.continue_on_error,
        append: req.append,
        progress,
    }
}

fn build_cadence_batch_options(
    req: &ExportRequest,
    progress: Option<ExportProgressCallback>,
    models: &Arc<ModelCleanup>,
) -> CadenceBatchOptions {
    let mode = normalize_cadence_mode(&req.mode);
    CadenceBatchOptions {
        ids: req.ids.clone(),
        output: PathBuf::from(normalize_output_path(&req.output_path)),
        library_name: resolve_library_name(req),
        export: CadenceExportOptions {
            symbol: mode == "symbol",
            footprint: mode == "footprint",
            model_3d: mode == "3d",
            full: mode == "full",
            force: req.force,
        },
        english_metadata: req.lcsc_english,
        parallel: req.parallel.max(1),
        continue_on_error: req.continue_on_error,
        append: req.append,
        models: Arc::clone(models),
        progress,
    }
}

fn progress_callback(state: Arc<Mutex<MonitorState>>) -> ExportProgressCallback {
    Arc::new(move |current, total, subject| {
        set_progress(
            &state,
            format!("{current}/{total}: {subject}"),
            current,
            total,
        );
    })
}

fn is_kicad_target(req: &ExportRequest) -> bool {
    normalize_target(&req.target) == "kicad"
}

fn is_cadence_target(req: &ExportRequest) -> bool {
    normalize_target(&req.target) == "cadence"
}

fn is_altium_target(req: &ExportRequest) -> bool {
    normalize_target(&req.target) == "altium"
}

/// Whether the run downloads 3D models, so its logo removal is worth reporting.
fn exports_models(req: &ExportRequest) -> bool {
    if is_altium_target(req) {
        matches!(normalize_altium_mode(&req.mode), "full" | "pcblib" | "3d")
    } else if is_cadence_target(req) {
        matches!(normalize_cadence_mode(&req.mode), "full" | "3d")
    } else {
        matches!(normalize_kicad_mode(&req.mode), "full" | "3d")
    }
}

fn normalize_target(target: &str) -> &str {
    match target.trim().to_ascii_lowercase().as_str() {
        "kicad" => "kicad",
        "cadence" => "cadence",
        _ => "altium",
    }
}

fn normalize_altium_mode(mode: &str) -> &str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "schlib" => "schlib",
        "pcblib" => "pcblib",
        "3d" => "3d",
        _ => "full",
    }
}

fn normalize_kicad_mode(mode: &str) -> &str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "symbol" => "symbol",
        "footprint" => "footprint",
        "3d" => "3d",
        _ => "full",
    }
}

fn normalize_cadence_mode(mode: &str) -> &str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "symbol" => "symbol",
        "footprint" => "footprint",
        "3d" => "3d",
        _ => "full",
    }
}

fn normalize_output_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        DEFAULT_OUTPUT_DIR.to_string()
    } else {
        trimmed.to_string()
    }
}

fn effective_library_name(req: &ExportRequest) -> Option<String> {
    if is_altium_target(req) && !req.merge {
        return None;
    }

    Some(resolve_library_name(req))
}

fn resolve_library_name(req: &ExportRequest) -> String {
    let trimmed = req.library_name.trim();
    if trimmed.is_empty() {
        DEFAULT_LIBRARY_NAME.to_string()
    } else {
        trimmed.to_string()
    }
}

fn format_altium_summary(req: &ExportRequest, summary: &BatchSummary, elapsed: Duration) -> String {
    let mut lines = vec![
        "Status: Success".to_string(),
        "Target: Altium".to_string(),
        format!("Elapsed: {}", format_duration(elapsed)),
        format!(
            "export Altium batch complete. Total: {} | Skipped: {} | Success: {} | Failed: {}",
            summary.total, summary.skipped, summary.success, summary.failed,
        ),
    ];

    lines.push(format!(
        "Targets: {} | Merge: {} | Parallel: {}",
        mode_label(req),
        merge_label(req),
        req.parallel.max(1),
    ));

    lines.push(format!(
        "Continue on error: {} | LCSC English: {} | Force: {}",
        yes_no(req.continue_on_error),
        yes_no(req.lcsc_english),
        yes_no(req.force),
    ));

    if req.merge {
        lines.push(format!(
            "Library name: {}",
            effective_library_name(req).unwrap_or_else(|| DEFAULT_LIBRARY_NAME.to_string())
        ));
    }

    if !summary.failed_ids.is_empty() {
        lines.push(format!("Failed IDs: {}", summary.failed_ids.join(", ")));
    }

    if summary.generated_files.is_empty() {
        lines.push(format!("Output directory: {}", summary.output.display()));
        if !req.merge {
            lines.push(format!("Folders: {}", target_folders(req).join(", ")));
        }
    } else {
        for path in &summary.generated_files {
            lines.push(format!("Generated: {}", path.display()));
        }
    }

    lines.join("\n")
}

fn format_kicad_summary(
    req: &ExportRequest,
    summary: &KicadBatchSummary,
    elapsed: Duration,
) -> String {
    let mut lines = vec![
        "Status: Success".to_string(),
        "Target: KiCad".to_string(),
        format!("Elapsed: {}", format_duration(elapsed)),
        format!(
            "export KiCad batch complete. Total: {} | Skipped: {} | Success: {} | Failed: {}",
            summary.total, summary.skipped, summary.success, summary.failed,
        ),
    ];

    lines.push(format!(
        "Targets: {} | Parallel: {}",
        mode_label(req),
        req.parallel.max(1),
    ));

    lines.push(format!(
        "Continue on error: {} | LCSC English: {} | Force: {}",
        yes_no(req.continue_on_error),
        yes_no(req.lcsc_english),
        yes_no(req.force),
    ));

    lines.push(format!("Library name: {}", resolve_library_name(req)));
    lines.push(format!(
        "Stage totals: fetch {} | metadata {} | symbol {} | footprint {} | 3d {}",
        format_duration(summary.timings.component_fetch),
        format_duration(summary.timings.metadata),
        format_duration(summary.timings.symbol),
        format_duration(summary.timings.footprint),
        format_duration(summary.timings.model_3d),
    ));
    lines.push(format!(
        "3D models: written {} | skipped {} | missing {}",
        summary.models.written, summary.models.skipped, summary.models.missing
    ));

    if !summary.failed_ids.is_empty() {
        lines.push(format!("Failed IDs: {}", summary.failed_ids.join(", ")));
    }

    if !summary.failure_details.is_empty() {
        lines.push("Failure details:".to_string());
        for detail in &summary.failure_details {
            lines.push(format!("- {}", detail));
        }
    }

    if summary.generated_files.is_empty() {
        lines.push(format!("Output directory: {}", summary.output.display()));
        lines.push(format!("Folders: {}", target_folders(req).join(", ")));
    } else {
        for path in &summary.generated_files {
            lines.push(format!("Generated: {}", path.display()));
        }
    }

    lines.join("\n")
}

fn format_cadence_summary(
    req: &ExportRequest,
    summary: &CadenceBatchSummary,
    elapsed: Duration,
) -> String {
    let mut lines = vec![
        "Status: Success".to_string(),
        "Target: Cadence".to_string(),
        format!("Elapsed: {}", format_duration(elapsed)),
        format!(
            "export Cadence batch complete. Total: {} | Skipped: {} | Success: {} | Failed: {}",
            summary.total, summary.skipped, summary.success, summary.failed,
        ),
        format!(
            "Targets: {} | Parallel: {}",
            mode_label(req),
            req.parallel.max(1),
        ),
        format!(
            "Continue on error: {} | LCSC English: {} | Force: {}",
            yes_no(req.continue_on_error),
            yes_no(req.lcsc_english),
            yes_no(req.force),
        ),
        format!("Library name: {}", resolve_library_name(req)),
        format!(
            "Capture symbols: {} | Allegro footprints: {} ({} IPC-7352) | 3D models: written {} | skipped {} | missing {}",
            summary.symbols_written,
            summary.footprints_written,
            summary.footprints_ipc,
            summary.models_written,
            summary.models_skipped,
            summary.models_missing,
        ),
    ];

    if !summary.failed_ids.is_empty() {
        lines.push(format!("Failed IDs: {}", summary.failed_ids.join(", ")));
    }
    if !summary.failure_details.is_empty() {
        lines.push("Failure details:".to_string());
        for detail in &summary.failure_details {
            lines.push(format!("- {}", detail));
        }
    }
    for path in &summary.generated_files {
        lines.push(format!("Generated: {}", path.display()));
    }
    lines.push("Capture: File > Import > Library XML turns the .xml into an .olb".to_string());
    lines.push(
        "Allegro: in PCB Editor run skill load(\"<library>_allegro/<library>.il\") to build the .dra/.psm/.pad files"
            .to_string(),
    );
    if !summary.notes.is_empty() {
        lines.push("Notes:".to_string());
        for note in &summary.notes {
            lines.push(format!("- {}", note));
        }
    }
    lines.join("\n")
}

fn format_duration(duration: Duration) -> String {
    let total_ms = duration.as_millis();
    if total_ms < 60_000 {
        return format!("{}.{:02}s", total_ms / 1_000, (total_ms % 1_000) / 10);
    }

    let total_secs = duration.as_secs();
    let hours = total_secs / 3_600;
    let minutes = (total_secs % 3_600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn format_error(req: &ExportRequest, error: &str, elapsed: Duration) -> String {
    let target = if is_kicad_target(req) {
        "KiCad"
    } else if is_cadence_target(req) {
        "Cadence"
    } else {
        "Altium"
    };
    let failed_ids = crate::extract::extract_lcsc_ids(error);
    let mut lines = vec![
        "Status: Failed".to_string(),
        format!("Target: {target}"),
        format!("Elapsed: {}", format_duration(elapsed)),
    ];
    if !failed_ids.is_empty() {
        lines.push(format!("Failed IDs: {}", failed_ids.join(", ")));
    } else {
        lines.push(error.to_string());
    }
    lines.push(format!("Targets: {}", mode_label(req)));
    lines.push(format!(
        "Output directory: {}",
        normalize_output_path(&req.output_path)
    ));
    lines.push(format!(
        "Library name: {}",
        effective_library_name(req).unwrap_or_else(|| DEFAULT_LIBRARY_NAME.to_string())
    ));
    lines.join("\n")
}

fn mode_label(req: &ExportRequest) -> &'static str {
    if is_cadence_target(req) {
        match normalize_cadence_mode(&req.mode) {
            "symbol" => "Capture",
            "footprint" => "Allegro",
            "3d" => "3D",
            _ => "Full",
        }
    } else if is_kicad_target(req) {
        match normalize_kicad_mode(&req.mode) {
            "symbol" => "Symbol",
            "footprint" => "Footprint",
            "3d" => "3D",
            _ => "Full",
        }
    } else {
        match normalize_altium_mode(&req.mode) {
            "schlib" => "SchLib",
            "pcblib" => "PcbLib",
            "3d" => "3D",
            _ => "Full",
        }
    }
}

fn merge_label(req: &ExportRequest) -> &'static str {
    match (req.merge, req.append) {
        (true, true) => "ON (append)",
        (true, false) => "ON",
        _ => "OFF",
    }
}

fn target_folders(req: &ExportRequest) -> Vec<&'static str> {
    if is_cadence_target(req) {
        match normalize_cadence_mode(&req.mode) {
            "symbol" => vec!["*.xml"],
            "footprint" => vec!["*_allegro"],
            "3d" => vec!["*_step"],
            _ => vec!["*.xml", "*_allegro", "*_step"],
        }
    } else if is_kicad_target(req) {
        match normalize_kicad_mode(&req.mode) {
            "symbol" => vec!["*.kicad_sym"],
            "footprint" => vec!["*.pretty"],
            "3d" => vec!["*.3dshapes"],
            _ => vec!["*.kicad_sym", "*.pretty", "*.3dshapes"],
        }
    } else {
        match normalize_altium_mode(&req.mode) {
            "schlib" => vec!["schlib"],
            "pcblib" => vec!["pcblib"],
            "3d" => vec!["*.step"],
            _ => vec!["schlib", "pcblib"],
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "ON" } else { "OFF" }
}
