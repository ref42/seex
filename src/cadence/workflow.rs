//! Batch export of LCSC components to Cadence import files.
//!
//! ```text
//! <output>/<Library>.xml            Capture Library XML: File > Import > Library XML
//! <output>/<Library>_allegro/       one SKILL script per footprint, and <Library>.il,
//!                                   which builds them all into .dra / .psm / .pad files
//! <output>/<Library>_step/          STEP models, mapped by the footprint scripts
//! ```
//!
//! Like the KiCad export, a run refuses to write into a library that is already there
//! unless it is asked to append. Appending keeps what the library holds and adds the
//! new parts; `force` replaces parts, footprints and models that are already there.
//! Parts that share an IPC-7352 land pattern share one footprint and its 3D model.
//! The models are written as EasyEDA serves them (without the logo, by default) and
//! each footprint maps its model where EasyEDA places it.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::cadence::allegro::{
    StepModel, allegro_identifier, build_allegro_footprint_with, footprint_name_for, library_script,
};
use crate::cadence::capture::{
    PinLabels, build_capture_part_with, capture_identifier, package_blocks, render_library_xml,
};
use crate::cadence::ipc7352;
use crate::cadence::step_place::model_on_footprint;
use crate::easyeda::{ComponentData, EasyedaApi};
use crate::error::{AppError, Result};
use crate::kicad::workflow::{apply_requested_metadata, validate_lcsc_id};
use crate::kicad::{KiFootprint, KiPin, KiSymbol, build_ki_footprint, build_ki_symbol};
use crate::lcsc::LcscClient;
use crate::monitor::ExportProgressCallback;
use crate::step_model::ModelCleanup;

/// How long one component may take before the batch gives up on it (as for KiCad).
const COMPONENT_TIMEOUT: Duration = Duration::from_secs(180);
/// Allegro keeps package symbol names to 31 characters.
const MAX_FOOTPRINT_NAME: usize = 31;

#[derive(Debug, Clone, Default)]
pub struct CadenceExportOptions {
    pub symbol: bool,
    pub footprint: bool,
    pub model_3d: bool,
    pub full: bool,
    pub force: bool,
}

impl CadenceExportOptions {
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

#[derive(Clone)]
pub struct CadenceBatchOptions {
    pub ids: Vec<String>,
    pub output: PathBuf,
    pub library_name: String,
    pub export: CadenceExportOptions,
    pub english_metadata: bool,
    pub parallel: usize,
    pub continue_on_error: bool,
    /// Add to the library already in the output directory instead of refusing it.
    pub append: bool,
    /// What happens to downloaded 3D models before they are written (logo removal).
    pub models: Arc<ModelCleanup>,
    pub progress: Option<ExportProgressCallback>,
}

#[derive(Debug, Clone, Default)]
pub struct CadenceBatchSummary {
    pub total: usize,
    pub skipped: usize,
    pub success: usize,
    pub failed: usize,
    pub failed_ids: Vec<String>,
    pub failure_details: Vec<String>,
    pub generated_files: Vec<PathBuf>,
    /// Conversion notes worth reading (lands kept from the manufacturer, renamed pins...).
    pub notes: Vec<String>,
    pub symbols_written: usize,
    pub footprints_written: usize,
    /// Footprints calculated as IPC-7352 land patterns.
    pub footprints_ipc: usize,
    pub models_written: usize,
    pub models_skipped: usize,
    pub models_missing: usize,
}

/// Where one library's files go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CadenceLayout {
    /// Library name as Capture accepts it.
    pub name: String,
    /// Capture Library XML file.
    pub xml: PathBuf,
    /// Allegro footprint scripts, and the footprints they build.
    pub allegro_dir: PathBuf,
    /// STEP models.
    pub model_dir: PathBuf,
}

impl CadenceLayout {
    pub fn new(output: &Path, library_name: &str) -> Self {
        let name = capture_identifier(library_name);
        Self {
            xml: output.join(format!("{name}.xml")),
            allegro_dir: output.join(format!("{name}_allegro")),
            model_dir: output.join(format!("{name}_step")),
            name,
        }
    }

    /// The library script that builds every footprint.
    pub fn library_script(&self) -> PathBuf {
        self.allegro_dir.join(format!("{}.il", self.name))
    }

    fn footprint_script(&self, footprint: &str) -> PathBuf {
        self.allegro_dir.join(format!("{footprint}.il"))
    }

    fn model_file(&self, footprint: &str) -> PathBuf {
        self.model_dir.join(format!("{footprint}.step"))
    }

    /// Footprint scripts in the library folder, sorted (the library script excluded).
    fn footprint_scripts(&self) -> Vec<String> {
        let library = self.name.to_ascii_lowercase();
        let mut names: Vec<String> = fs::read_dir(&self.allegro_dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| has_extension(path, "il"))
            .filter_map(|path| Some(path.file_stem()?.to_str()?.to_string()))
            .filter(|stem| stem.to_ascii_lowercase() != library)
            .collect();
        names.sort();
        names
    }
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// What one component contributed.
struct ExportedPart {
    lcsc_id: String,
    title: String,
    /// `<Package>` fragment and part name, when the symbol is written.
    symbol: Option<(String, String)>,
    footprint_written: bool,
    footprint_ipc: bool,
    model: Option<ModelOutcome>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelOutcome {
    Written,
    Skipped,
    Missing,
}

/// State the parallel parts share.
struct Shared {
    api: EasyedaApi,
    lcsc: Option<LcscClient>,
    layout: CadenceLayout,
    export: CadenceExportOptions,
    models: Arc<ModelCleanup>,
    /// Capture parts the library already has (upper case), kept unless `force`.
    existing_parts: HashSet<String>,
    /// Footprints claimed by a part of this run: the first part to use one writes it.
    claimed: Mutex<HashSet<String>>,
}

pub async fn export_cadence_batch(options: CadenceBatchOptions) -> Result<CadenceBatchSummary> {
    validate_options(&options)?;
    let layout = CadenceLayout::new(&options.output, &options.library_name);
    reject_existing_library(&options, &layout)?;

    let existing_xml = if options.export.targets_symbol() {
        fs::read_to_string(&layout.xml).unwrap_or_default()
    } else {
        String::new()
    };
    let existing_blocks = package_blocks(&existing_xml);
    let existing_parts = existing_blocks
        .iter()
        .map(|(name, _)| name.to_ascii_uppercase())
        .collect();

    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for id in &options.ids {
        let id = id.trim().to_ascii_uppercase();
        if !id.is_empty() && seen.insert(id.clone()) {
            ids.push(id);
        }
    }

    let mut summary = CadenceBatchSummary {
        total: ids.len(),
        ..CadenceBatchSummary::default()
    };
    if let Some(progress) = &options.progress {
        progress(0, summary.total, &crate::progress::starting(ids.len()));
    }

    let shared = Arc::new(Shared {
        api: EasyedaApi::new(),
        lcsc: options.english_metadata.then(LcscClient::new),
        layout: layout.clone(),
        export: options.export.clone(),
        models: Arc::clone(&options.models),
        existing_parts,
        claimed: Mutex::new(HashSet::new()),
    });
    let semaphore = Arc::new(Semaphore::new(options.parallel.max(1)));
    let done = Arc::new(AtomicUsize::new(0));
    let mut join_set = JoinSet::new();
    for id in ids {
        let shared = Arc::clone(&shared);
        let semaphore = Arc::clone(&semaphore);
        let progress = options.progress.clone();
        let done = Arc::clone(&done);
        let total = summary.total;
        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("Cadence batch semaphore should remain open");
            if let Some(progress) = &progress {
                progress(
                    done.load(Ordering::Relaxed),
                    total,
                    &crate::progress::fetching(&id),
                );
            }
            let result =
                match tokio::time::timeout(COMPONENT_TIMEOUT, export_part(&shared, &id)).await {
                    Ok(result) => result,
                    Err(_) => Err(AppError::Other(format!(
                        "{id}: gave up after {}s",
                        COMPONENT_TIMEOUT.as_secs()
                    ))),
                };
            done.fetch_add(1, Ordering::Relaxed);
            (id, result)
        });
    }

    let mut parts = Vec::new();
    let mut first_error = None;
    while let Some(joined) = join_set.join_next().await {
        let (id, result) = match joined {
            Ok(joined) => joined,
            Err(err) => (
                "?".to_string(),
                Err(AppError::Other(format!(
                    "Cadence batch task join failed: {err}"
                ))),
            ),
        };
        let message = match result {
            Ok(part) => {
                summary.success += 1;
                let message = crate::progress::exported(&part.lcsc_id, Some(&part.title));
                parts.push(part);
                message
            }
            Err(err) => {
                summary.failed += 1;
                summary.failed_ids.push(id.clone());
                summary.failure_details.push(format!("{id}: {err}"));
                if first_error.is_none() {
                    first_error = Some(err);
                }
                crate::progress::failed(&id)
            }
        };
        if let Some(progress) = &options.progress {
            progress(summary.success + summary.failed, summary.total, &message);
        }
    }
    parts.sort_by(|a, b| a.lcsc_id.cmp(&b.lcsc_id));

    for part in &parts {
        let wrote_anything = part.symbol.is_some()
            || part.footprint_written
            || part.model == Some(ModelOutcome::Written);
        if !wrote_anything {
            summary.skipped += 1;
        }
        if part.footprint_written {
            summary.footprints_written += 1;
            if part.footprint_ipc {
                summary.footprints_ipc += 1;
            }
        }
        match part.model {
            Some(ModelOutcome::Written) => summary.models_written += 1,
            Some(ModelOutcome::Skipped) => summary.models_skipped += 1,
            Some(ModelOutcome::Missing) => summary.models_missing += 1,
            None => {}
        }
        summary.notes.extend(
            part.notes
                .iter()
                .map(|note| format!("{}: {note}", part.lcsc_id)),
        );
    }

    // The symbols of this run join the ones the library keeps, in one document.
    let new_parts: Vec<(String, String)> = parts
        .iter()
        .filter_map(|part| part.symbol.clone())
        .collect();
    if !new_parts.is_empty() {
        summary.symbols_written = new_parts.len();
        let replaced: HashSet<String> = new_parts
            .iter()
            .map(|(name, _)| name.to_ascii_uppercase())
            .collect();
        let mut packages: Vec<String> = existing_blocks
            .into_iter()
            .filter(|(name, _)| !replaced.contains(&name.to_ascii_uppercase()))
            .map(|(_, block)| block)
            .collect();
        packages.extend(new_parts.into_iter().map(|(_, xml)| xml));
        if let Some(dir) = layout.xml.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(
            &layout.xml,
            render_library_xml(&format!("{}.olb", layout.name), &packages),
        )?;
    }
    if options.export.targets_footprint() {
        let footprints = layout.footprint_scripts();
        if !footprints.is_empty() {
            fs::write(
                layout.library_script(),
                library_script(&layout.name, &layout.allegro_dir, &footprints),
            )?;
        }
    }
    summary.generated_files = generated_files(&layout, &options.export);

    let stopped_with = |summary: &CadenceBatchSummary, first_error: Option<AppError>| {
        let reason = first_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "Cadence export failed".to_string());
        if summary.failed_ids.is_empty() {
            AppError::Other(reason)
        } else {
            AppError::Other(format!(
                "{reason}. Failed IDs: {}",
                summary.failed_ids.join(", ")
            ))
        }
    };
    if summary.failed > 0 && !options.continue_on_error {
        return Err(stopped_with(&summary, first_error));
    }
    if summary.success == 0 {
        return Err(stopped_with(&summary, first_error));
    }
    Ok(summary)
}

fn validate_options(options: &CadenceBatchOptions) -> Result<()> {
    if options.ids.is_empty() {
        return Err(AppError::Other(
            "no valid LCSC IDs found for Cadence export".to_string(),
        ));
    }
    if options.parallel == 0 {
        return Err(AppError::Other("--parallel must be at least 1".to_string()));
    }
    let export = &options.export;
    if !(export.targets_symbol() || export.targets_footprint() || export.targets_model_3d()) {
        return Err(AppError::Other(
            "choose what to export: symbol, footprint, 3D or full".to_string(),
        ));
    }
    Ok(())
}

/// Refuses to write into a library that is already there unless the run appends,
/// the rule the KiCad and Altium exports follow.
fn reject_existing_library(options: &CadenceBatchOptions, layout: &CadenceLayout) -> Result<()> {
    if options.append {
        return Ok(());
    }
    let export = &options.export;
    let mut existing = Vec::new();
    if export.targets_symbol() && layout.xml.exists() {
        existing.push(layout.xml.display().to_string());
    }
    if export.targets_footprint() && !layout.footprint_scripts().is_empty() {
        existing.push(layout.allegro_dir.display().to_string());
    }
    let models_only = export.model_3d && !export.full && !export.footprint;
    if models_only && holds_extension(&layout.model_dir, "step") {
        existing.push(layout.model_dir.display().to_string());
    }
    if existing.is_empty() {
        Ok(())
    } else {
        Err(AppError::Other(format!(
            "the Cadence library already exists ({}); turn on Append to add to it",
            existing.join(", ")
        )))
    }
}

fn holds_extension(dir: &Path, ext: &str) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .filter_map(|entry| entry.ok())
            .any(|entry| has_extension(&entry.path(), ext))
    })
}

fn generated_files(layout: &CadenceLayout, export: &CadenceExportOptions) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if export.targets_symbol() && layout.xml.exists() {
        files.push(layout.xml.clone());
    }
    if export.targets_footprint() && layout.library_script().exists() {
        files.push(layout.library_script());
    }
    if export.targets_model_3d() && holds_extension(&layout.model_dir, "step") {
        files.push(layout.model_dir.clone());
    }
    files
}

async fn export_part(shared: &Shared, lcsc_id: &str) -> Result<ExportedPart> {
    validate_lcsc_id(lcsc_id)?;
    let mut component = shared.api.get_component_data(lcsc_id).await?;
    apply_requested_metadata(&mut component, shared.lcsc.as_ref()).await;
    let export = &shared.export;
    let layout = &shared.layout;
    let title = strip_lcsc_suffix(&component.title, lcsc_id);
    let part_name = capture_identifier(&format!("{title}_{lcsc_id}"));
    let mut notes = Vec::new();

    // The symbol's pins say which pad is the cathode; the footprint name is set
    // once the footprint is known.
    let mut ki_symbol = build_ki_symbol(&component, &part_name, String::new());
    let first = first_pin(ki_symbol.as_ref().ok(), &component.designator);
    let chosen = footprint_for(&component, lcsc_id, &title, &first, &mut notes)?;
    let footprint_name = chosen.name.clone();

    let mut symbol = None;
    if export.targets_symbol()
        && (export.force
            || !shared
                .existing_parts
                .contains(&part_name.to_ascii_uppercase()))
    {
        let ki_symbol = ki_symbol
            .as_mut()
            .map_err(|err| AppError::Other(err.to_string()))?;
        ki_symbol.footprint = footprint_name.clone();
        let part = build_capture_part_with(
            ki_symbol,
            &part_name,
            &footprint_name,
            pin_labels(&component),
        );
        notes.extend(part.warnings.iter().map(|w| format!("symbol: {w}")));
        symbol = Some((part.name, part.xml));
    }

    // The first part of the run that uses a footprint writes it and its model.
    let first_user = shared
        .claimed
        .lock()
        .map(|mut claimed| claimed.insert(footprint_name.clone()))
        .unwrap_or(true);
    let model_file = layout.model_file(&footprint_name);
    let mut model = None;
    if export.targets_model_3d() {
        model = Some(match &component.model_3d {
            None => ModelOutcome::Missing,
            Some(_) if !first_user => ModelOutcome::Skipped,
            Some(_) if model_file.exists() && !export.force => ModelOutcome::Skipped,
            Some(info) => {
                let step = shared.api.download_3d_step(&info.uuid).await?;
                // Written as EasyEDA has it: the footprint maps it with EasyEDA's own
                // placement (see `step_place`).
                let step = shared.models.clean(lcsc_id, &step);
                fs::create_dir_all(&layout.model_dir)?;
                fs::write(&model_file, &step)?;
                ModelOutcome::Written
            }
        });
    }

    let script = layout.footprint_script(&footprint_name);
    let footprint_written = export.targets_footprint()
        && first_user
        && (export.force || !script.exists() || model == Some(ModelOutcome::Written));
    if footprint_written {
        let model_dir = layout.model_dir.clone();
        let model_name = format!("{footprint_name}.step");
        let step = if model_file.exists() && component.model_3d.is_some() {
            let data = fs::read(&model_file).ok();
            Some(StepModel {
                file: &model_name,
                dir: &model_dir,
                transform: model_on_footprint(
                    &component,
                    &chosen.easyeda,
                    chosen.ipc,
                    data.as_deref(),
                    &mut notes,
                ),
            })
        } else {
            None
        };
        let refdes = designator_prefix(&component.designator).unwrap_or_else(|| "U".to_string());
        let allegro =
            build_allegro_footprint_with(&chosen.footprint, &footprint_name, &refdes, step);
        notes.extend(allegro.warnings.iter().map(|w| format!("footprint: {w}")));
        fs::create_dir_all(&layout.allegro_dir)?;
        fs::write(&script, allegro.script)?;
    }

    Ok(ExportedPart {
        lcsc_id: lcsc_id.to_string(),
        title,
        symbol,
        footprint_written,
        footprint_ipc: chosen.ipc.is_some(),
        model,
        notes,
    })
}

/// The footprint chosen for a part.
struct ChosenFootprint {
    name: String,
    footprint: KiFootprint,
    /// EasyEDA's own footprint, which its model placement refers to.
    easyeda: KiFootprint,
    /// For a calculated land pattern: its turn (degrees) and shift (mm) from
    /// EasyEDA's footprint.
    ipc: Option<(f64, (f64, f64))>,
}

/// The footprint for a part: the IPC-7352 land pattern when SeEx can calculate it
/// for the package, otherwise EasyEDA's lands with IPC-7352 courtyard and
/// silkscreen rules.
fn footprint_for(
    component: &ComponentData,
    lcsc_id: &str,
    title: &str,
    first: &str,
    notes: &mut Vec<String>,
) -> Result<ChosenFootprint> {
    let own_name = footprint_name_for(lcsc_id, title);
    let easyeda = build_ki_footprint(component, &own_name)?;
    let prefix = designator_prefix(&component.designator).unwrap_or_default();
    match ipc7352::land_pattern(&component.package, &prefix, &easyeda, first) {
        Ok(pattern) => {
            notes.extend(pattern.notes.iter().cloned());
            Ok(ChosenFootprint {
                name: allegro_identifier(&pattern.name, MAX_FOOTPRINT_NAME),
                footprint: pattern.footprint,
                easyeda,
                ipc: Some((pattern.turn, pattern.shift)),
            })
        }
        Err(reason) => {
            notes.push(format!(
                "IPC-7352 lands not calculated ({reason}); the manufacturer's lands are kept with IPC-7352 courtyard and silkscreen rules"
            ));
            let (footprint, _) = ipc7352::conform(&easyeda);
            Ok(ChosenFootprint {
                name: own_name,
                footprint,
                easyeda,
                ipc: None,
            })
        }
    }
}

/// The pin IPC puts first: the cathode of a diode or LED (the pin named K or
/// cathode), otherwise pin 1.
fn first_pin(symbol: Option<&KiSymbol>, designator: &str) -> String {
    let polarized = matches!(designator_prefix(designator).as_deref(), Some("D" | "LED"));
    let pins: Vec<&KiPin> = symbol
        .map(|symbol| {
            symbol
                .pins
                .iter()
                .chain(symbol.units.iter().flat_map(|unit| unit.pins.iter()))
                .collect()
        })
        .unwrap_or_default();
    if polarized {
        let cathode = pins.iter().find(|pin| {
            let name = pin.name.trim().to_ascii_uppercase();
            name == "K" || name == "CATHODE" || name == "KA" || name == "C" && pins.len() == 2
        });
        if let Some(pin) = cathode {
            return pin.number.clone();
        }
    }
    "1".to_string()
}

/// `BAT54WS_C466657` -> `BAT54WS` for part C466657.
fn strip_lcsc_suffix(title: &str, lcsc_id: &str) -> String {
    let title = title.trim();
    for sep in ['_', '-', ' '] {
        let suffix = format!("{sep}{lcsc_id}");
        if title.len() > suffix.len()
            && title
                .to_ascii_uppercase()
                .ends_with(&suffix.to_ascii_uppercase())
        {
            return title[..title.len() - suffix.len()].to_string();
        }
    }
    title.to_string()
}

/// Reference prefix from EasyEDA's designator (`R?` -> `R`, `U1` -> `U`).
fn designator_prefix(designator: &str) -> Option<String> {
    let prefix: String = designator
        .trim()
        .trim_end_matches(|c: char| c == '?' || c.is_ascii_digit())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!prefix.is_empty()).then_some(prefix)
}

/// Show pin names / numbers only if EasyEDA shows them on at least one pin
/// (resistors, capacitors and the like hide both).
fn pin_labels(component: &ComponentData) -> PinLabels {
    let lines = component.data_str.iter().chain(
        component
            .symbol_parts
            .iter()
            .flat_map(|part| part.data_str.iter()),
    );
    let shown = |segment: Option<&str>| {
        segment
            .and_then(|segment| segment.split('~').next())
            .is_some_and(|flag| !matches!(flag.trim(), "" | "0" | "none"))
    };
    let mut any_pin = false;
    let mut labels = PinLabels {
        names: false,
        numbers: false,
    };
    for line in lines.filter(|line| line.starts_with("P~")) {
        any_pin = true;
        let segments: Vec<&str> = line.split("^^").collect();
        labels.names |= shown(segments.get(3).copied());
        labels.numbers |= shown(segments.get(4).copied());
    }
    if any_pin {
        labels
    } else {
        PinLabels::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("seex_cadence_{tag}_{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn options(output: &Path, append: bool) -> CadenceBatchOptions {
        CadenceBatchOptions {
            ids: vec!["C25744".to_string()],
            output: output.to_path_buf(),
            library_name: "Parts".to_string(),
            export: CadenceExportOptions {
                full: true,
                ..CadenceExportOptions::default()
            },
            english_metadata: false,
            parallel: 1,
            continue_on_error: false,
            append,
            models: Default::default(),
            progress: None,
        }
    }

    #[test]
    fn the_library_files_sit_side_by_side() {
        let layout = CadenceLayout::new(Path::new("out"), "My Parts");
        assert_eq!(layout.name, "My_Parts");
        assert_eq!(layout.xml, Path::new("out").join("My_Parts.xml"));
        assert_eq!(
            layout.allegro_dir,
            Path::new("out").join("My_Parts_allegro")
        );
        assert_eq!(layout.model_dir, Path::new("out").join("My_Parts_step"));
        assert_eq!(
            layout.library_script(),
            Path::new("out")
                .join("My_Parts_allegro")
                .join("My_Parts.il")
        );
    }

    #[test]
    fn an_existing_library_is_only_written_into_when_appending() {
        let output = temp_dir("append");
        let layout = CadenceLayout::new(&output, "Parts");
        fs::write(&layout.xml, "<Lib/>").unwrap();
        let refused = reject_existing_library(&options(&output, false), &layout);
        assert!(refused.unwrap_err().to_string().contains("Append"));
        assert!(reject_existing_library(&options(&output, true), &layout).is_ok());
        let _ = fs::remove_dir_all(&output);
    }

    #[test]
    fn the_library_script_skips_itself() {
        let output = temp_dir("scripts");
        let layout = CadenceLayout::new(&output, "Parts");
        fs::create_dir_all(&layout.allegro_dir).unwrap();
        for name in ["sot95p280x145-5n", "Parts", "resc1005x40n"] {
            fs::write(layout.allegro_dir.join(format!("{name}.il")), "").unwrap();
        }
        fs::write(layout.allegro_dir.join("notes.txt"), "").unwrap();
        assert_eq!(
            layout.footprint_scripts(),
            ["resc1005x40n", "sot95p280x145-5n"]
        );
        let _ = fs::remove_dir_all(&output);
    }

    #[test]
    fn names_lose_the_lcsc_suffix_and_designators_their_number() {
        assert_eq!(strip_lcsc_suffix("BAT54WS_C466657", "C466657"), "BAT54WS");
        assert_eq!(
            strip_lcsc_suffix("0402WGF1002TCE", "C25744"),
            "0402WGF1002TCE"
        );
        assert_eq!(designator_prefix("R?").as_deref(), Some("R"));
        assert_eq!(designator_prefix("U12").as_deref(), Some("U"));
        assert_eq!(designator_prefix("?"), None);
    }
}
