use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::altium::footprint::build_pcblib_from_payload_with_model;
use crate::altium::merge::{
    PcblibRecordLibrary, SchlibRecord, normalize_lcsc_id, pcblib_records_from_library,
    read_pcblib_records, read_schlib_records, schlib_record_from_component, write_pcblib_records,
    write_schlib_records,
};
use crate::altium::pcblib::{PcbLibrary, scratch_payload_path, write_pcblib};
use crate::altium::schlib::NO_FILL_BGR;
use crate::altium::workflow::{
    build_schlib_component_from_detail_with_color, export_pcblib, export_schlib_with_options,
    load_placed_step_for_pcblib, resolved_footprint_name, step_model_candidates,
};
use crate::error::{AppError, Result};
use crate::lceda::{LcedaClient, SearchItem};
use crate::lcsc::{LcscClient, LcscProduct};
use crate::monitor::ExportProgressCallback;
use crate::step_model::ModelCleanup;
use crate::util::sanitize_filename;

#[derive(Clone)]
pub struct BatchOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub schlib: bool,
    pub pcblib: bool,
    pub full: bool,
    pub merge: bool,
    pub append: bool,
    pub library_name: Option<String>,
    pub parallel: usize,
    pub continue_on_error: bool,
    pub english_metadata: bool,
    pub force: bool,
    pub schematic_stroke_color: [u8; 3],
    /// `None` exports the symbol body with no fill at all.
    pub schematic_fill_color: Option<[u8; 3]>,
    /// What happens to downloaded 3D models before they are embedded (logo removal).
    pub models: Arc<ModelCleanup>,
    pub progress: Option<ExportProgressCallback>,
}

/// The Altium colour integer for a configured `#RRGGBB`.
fn color_bgr(color: [u8; 3]) -> i32 {
    color[0] as i32 | ((color[1] as i32) << 8) | ((color[2] as i32) << 16)
}

/// The same for an optional colour, where "no colour" is the sentinel the
/// SchLib writer turns into a transparent area.
fn fill_bgr(color: Option<[u8; 3]>) -> i32 {
    color.map_or(NO_FILL_BGR, color_bgr)
}

#[derive(Debug, Clone)]
pub struct BatchSummary {
    pub total: usize,
    pub skipped: usize,
    pub success: usize,
    pub failed: usize,
    pub failed_ids: Vec<String>,
    pub output: PathBuf,
    pub generated_files: Vec<PathBuf>,
}

pub async fn export_batch(client: &LcedaClient, options: BatchOptions) -> Result<BatchSummary> {
    let targets = ExportTargets::resolve(&options)?;
    if options.merge {
        return export_batch_merged(client, options, targets).await;
    }

    let options = Arc::new(options);

    fs::create_dir_all(&options.output)?;

    let input = fs::read_to_string(&options.input)?;
    let ids = parse_lcsc_ids(&input);
    if ids.is_empty() {
        return Err(AppError::Other(
            "no valid LCSC IDs found in batch input".to_string(),
        ));
    }

    let checkpoint_path = options.output.join(".checkpoint");
    let completed = load_checkpoint(&checkpoint_path)?;

    let mut pending = Vec::new();
    let mut skipped = 0usize;
    for id in ids {
        if !options.force && completed.contains(&id) {
            skipped += 1;
        } else {
            pending.push(id);
        }
    }

    let mut summary = BatchSummary {
        total: pending.len() + skipped,
        skipped,
        success: 0,
        failed: 0,
        failed_ids: Vec::new(),
        output: options.output.clone(),
        generated_files: Vec::new(),
    };

    let actual_parallel = if options.parallel > 1 && pending.len() > 1 {
        options.parallel
    } else {
        1
    };
    let mut progress = BatchProgress::new(
        summary.total,
        actual_parallel,
        batch_mode_label(&options),
        targets,
        &options.output,
        pending.clone(),
        options.progress.clone(),
    );
    if summary.skipped > 0 {
        progress.seed_skipped(summary.skipped, "checkpoint");
    }

    if pending.is_empty() {
        progress.finish();
        return Ok(summary);
    }

    if actual_parallel > 1 {
        run_parallel(
            client.clone(),
            options.clone(),
            targets,
            &checkpoint_path,
            pending,
            &mut summary,
            &mut progress,
        )
        .await?;
    } else {
        run_sequential(
            client.clone(),
            options.clone(),
            targets,
            &checkpoint_path,
            pending,
            &mut summary,
            &mut progress,
        )
        .await?;
    }

    progress.finish();
    Ok(summary)
}

#[derive(Debug, Clone, Copy)]
struct ExportTargets {
    schlib: bool,
    pcblib: bool,
}

impl ExportTargets {
    fn resolve(options: &BatchOptions) -> Result<Self> {
        if options.parallel == 0 {
            return Err(AppError::Other("--parallel must be at least 1".to_string()));
        }
        if options.append && !options.merge {
            return Err(AppError::Other(
                "--append is only supported together with --merge".to_string(),
            ));
        }

        let schlib = options.schlib || options.full;
        let pcblib = options.pcblib || options.full;
        if !schlib && !pcblib {
            return Err(AppError::Other(
                "at least one export target must be selected (--schlib, --pcblib, or --full)"
                    .to_string(),
            ));
        }
        if options.append && !schlib {
            return Err(AppError::Other(
                "--append currently requires --schlib or --full".to_string(),
            ));
        }

        Ok(Self { schlib, pcblib })
    }

    fn label(self) -> &'static str {
        match (self.schlib, self.pcblib) {
            (true, true) => "SchLib+PcbLib",
            (true, false) => "SchLib",
            (false, true) => "PcbLib",
            (false, false) => "none",
        }
    }
}

#[derive(Debug)]
struct MergeArtifacts {
    identity: String,
    component_name: String,
    schlib_record: Option<SchlibRecord>,
    /// The built part, for a merge that assembles the library from parts.
    pcblib_library: Option<PcbLibrary>,
    /// The same part already reduced to the streams a merged library holds, for an
    /// append merge. Only one of the two is ever set.
    pcblib_records: Option<PcblibRecordLibrary>,
}

/// Reduces a built part to its record form as soon as it is built.
///
/// An append merge only needs the streams, so keeping the structured library would
/// hold every exported part in memory until the whole batch had finished.
fn into_pcblib_records(mut artifacts: IndexedMergeArtifacts) -> Result<IndexedMergeArtifacts> {
    if let Some(library) = artifacts.artifacts.pcblib_library.take() {
        artifacts.artifacts.pcblib_records = Some(pcblib_records_from_library(&library)?);
    }
    Ok(artifacts)
}

#[derive(Debug)]
struct IndexedMergeArtifacts {
    index: usize,
    artifacts: MergeArtifacts,
}

#[derive(Debug)]
struct IndexedId {
    index: usize,
    id: String,
}

/// A fetched component payload, parked on disk until the component is built.
///
/// A merge fetches every component's symbol and footprint before it builds any of
/// them, so holding the parsed payloads put the whole batch's JSON in memory (a few
/// hundred parts is tens of megabytes). Writing each payload out as it arrives and
/// reading it back in the build keeps only the parts being worked on in memory. A
/// payload that cannot be written just stays in memory rather than failing the run.
#[derive(Debug)]
enum Payload {
    File(PathBuf),
    Inline(Box<Value>),
}

impl Payload {
    fn store(value: Value) -> Self {
        let path = scratch_payload_path();
        if let Ok(mut writer) = File::create(&path).map(BufWriter::new) {
            if serde_json::to_writer(&mut writer, &value).is_ok() && writer.flush().is_ok() {
                return Payload::File(path);
            }
            let _ = fs::remove_file(&path);
        }
        Payload::Inline(Box::new(value))
    }

    fn load(&self) -> Result<Value> {
        match self {
            Payload::Inline(value) => Ok((**value).clone()),
            Payload::File(path) => {
                let file = File::open(path)?;
                serde_json::from_reader(file).map_err(|err| {
                    AppError::InvalidResponse(format!("could not read a fetched payload: {err}"))
                })
            }
        }
    }
}

impl Drop for Payload {
    fn drop(&mut self) {
        if let Payload::File(path) = self {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(Debug)]
struct PreparedMergeComponent {
    index: usize,
    id: String,
    item: SearchItem,
    identity: String,
    component_name_base: String,
    footprint_name_base: String,
    step_candidates: Vec<String>,
    symbol_data: Option<Payload>,
    footprint_data: Option<Payload>,
    english_metadata: Option<LcscProduct>,
}

#[derive(Debug)]
struct NamedMergeComponent {
    index: usize,
    id: String,
    item: SearchItem,
    identity: String,
    component_name: String,
    footprint_name: String,
    step_candidates: Vec<String>,
    symbol_data: Option<Payload>,
    footprint_data: Option<Payload>,
    english_metadata: Option<LcscProduct>,
}

#[derive(Debug)]
struct ExportedComponent {
    display_name: String,
}

struct BatchProgress {
    total: usize,
    completed: usize,
    success: usize,
    skipped: usize,
    failed: usize,
    parallel: usize,
    started_at: Instant,
    last_subject: Option<String>,
    /// The ids this run will export, in the order they are handed out. The bar
    /// names the ones in flight from this and the counters, so nothing has to
    /// report each part individually for the label to stay current.
    queue: Vec<String>,
    last_render_width: usize,
    progress: Option<ExportProgressCallback>,
}

impl BatchProgress {
    fn new(
        total: usize,
        parallel: usize,
        mode_label: &str,
        targets: ExportTargets,
        output: &Path,
        queue: Vec<String>,
        progress_callback: Option<ExportProgressCallback>,
    ) -> Self {
        eprintln!(
            "Batch mode: {mode_label} | targets: {} | parallel: {}",
            targets.label(),
            parallel.max(1)
        );
        eprintln!("Output: {}", output.display());

        let mut progress = Self {
            total,
            completed: 0,
            success: 0,
            skipped: 0,
            failed: 0,
            parallel: parallel.max(1),
            started_at: Instant::now(),
            last_subject: None,
            queue,
            last_render_width: 0,
            progress: progress_callback,
        };
        progress.render();
        progress
    }

    fn seed_skipped(&mut self, count: usize, reason: &str) {
        if count == 0 {
            return;
        }
        self.completed = (self.completed + count).min(self.total);
        self.skipped += count;
        self.note(format!("Pre-skipped {count} item(s) from {reason}"));
    }

    fn note(&mut self, message: impl AsRef<str>) {
        self.clear_status_line();
        eprintln!("{}", message.as_ref());
        self.render();
    }

    fn record_success(&mut self, id: &str, detail: Option<&str>) {
        self.completed = (self.completed + 1).min(self.total);
        self.success += 1;
        self.last_subject = Some(crate::progress::exported(id, detail));
        self.render();
    }

    fn record_skip(&mut self, id: &str, reason: &str) {
        self.completed = (self.completed + 1).min(self.total);
        self.skipped += 1;
        self.last_subject = Some(crate::progress::skipped(id));
        self.note(format!("SKIP {id}: {reason}"));
    }

    fn record_failure(&mut self, id: &str, error: &AppError) {
        self.completed = (self.completed + 1).min(self.total);
        self.failed += 1;
        self.last_subject = Some(crate::progress::failed(id));
        self.note(format!("FAILED {id}: {error}"));
    }

    fn finish(&mut self) {
        if self.last_render_width > 0 {
            eprintln!();
            self.last_render_width = 0;
        }
    }

    fn render(&mut self) {
        let bar_width = 24usize;
        let filled = if self.total == 0 {
            bar_width
        } else {
            (self.completed * bar_width + self.total / 2) / self.total
        }
        .min(bar_width);
        let remaining = bar_width.saturating_sub(filled);
        let last = self.last_subject.as_deref().unwrap_or("-");
        let message = format!(
            "[{}{}] {}/{} | ok:{} skip:{} fail:{} | active:{} | last:{} | elapsed:{}",
            "#".repeat(filled),
            "-".repeat(remaining),
            self.completed,
            self.total,
            self.success,
            self.skipped,
            self.failed,
            self.active_count(),
            last,
            format_elapsed(self.started_at.elapsed().as_secs())
        );
        if let Some(progress) = &self.progress {
            progress(self.completed, self.total, &self.subject());
        }
        self.draw_status_line(&message);
    }

    /// What to say next to `current/total` in the window, in the words both
    /// targets use.
    ///
    /// The run used to pass the last *finished* component here, so the bar read
    /// `0/11: -` until the first one landed. A component being fetched is worth
    /// naming, and so is a run that has started but finished nothing yet.
    fn subject(&self) -> String {
        match self.queue.get(self.completed) {
            Some(id) => crate::progress::fetching(id),
            None => self
                .last_subject
                .clone()
                .unwrap_or_else(|| crate::progress::starting(self.total)),
        }
    }

    fn active_count(&self) -> usize {
        self.total.saturating_sub(self.completed).min(self.parallel)
    }

    fn clear_status_line(&mut self) {
        if self.last_render_width == 0 {
            return;
        }
        eprint!("\r{}\r", " ".repeat(self.last_render_width));
        let _ = io::stderr().flush();
        self.last_render_width = 0;
    }

    fn draw_status_line(&mut self, message: &str) {
        let current_width = message.chars().count();
        let padding = self.last_render_width.saturating_sub(current_width);
        eprint!("\r{}{}", message, " ".repeat(padding));
        let _ = io::stderr().flush();
        self.last_render_width = current_width;
    }
}

async fn export_batch_merged(
    client: &LcedaClient,
    options: BatchOptions,
    targets: ExportTargets,
) -> Result<BatchSummary> {
    fs::create_dir_all(&options.output)?;

    let input = fs::read_to_string(&options.input)?;
    let ids = parse_lcsc_ids(&input);
    if ids.is_empty() {
        return Err(AppError::Other(
            "no valid LCSC IDs found in batch input".to_string(),
        ));
    }

    let summary = BatchSummary {
        total: ids.len(),
        skipped: 0,
        success: 0,
        failed: 0,
        failed_ids: Vec::new(),
        output: options.output.clone(),
        generated_files: Vec::new(),
    };

    let actual_parallel = if options.parallel > 1 && ids.len() > 1 {
        options.parallel
    } else {
        1
    };
    let mut progress = BatchProgress::new(
        summary.total,
        actual_parallel,
        batch_mode_label(&options),
        targets,
        &options.output,
        ids.clone(),
        options.progress.clone(),
    );

    let result = if options.append {
        export_batch_merged_append(client, options, targets, ids, summary, &mut progress).await
    } else {
        export_batch_merged_fresh(client, options, targets, ids, summary, &mut progress).await
    };

    progress.finish();
    result
}

async fn export_batch_merged_fresh(
    client: &LcedaClient,
    options: BatchOptions,
    targets: ExportTargets,
    ids: Vec<String>,
    mut summary: BatchSummary,
    progress: &mut BatchProgress,
) -> Result<BatchSummary> {
    let library_name = resolve_library_name(&options);
    let merged_pcblib_file = format!("{}.PcbLib", sanitize_filename(&library_name));
    let schlib_path = options
        .output
        .join(format!("{}.SchLib", sanitize_filename(&library_name)));
    let pcblib_path = options
        .output
        .join(format!("{}.PcbLib", sanitize_filename(&library_name)));

    reject_existing_merged_outputs(targets, &schlib_path, &pcblib_path)?;
    progress.note(format!("Library: {library_name}"));

    let mut used_symbol_names = HashSet::new();
    let mut used_footprint_names = HashSet::new();
    let exported = export_merged_components_parallel(
        client,
        &options,
        targets,
        ids.into_iter()
            .enumerate()
            .map(|(index, id)| IndexedId { index, id })
            .collect(),
        &mut used_symbol_names,
        &mut used_footprint_names,
        &merged_pcblib_file,
        &mut summary,
        progress,
    )
    .await?;

    if summary.success == 0 {
        return Err(AppError::Other(
            "no components exported successfully for merged batch".to_string(),
        ));
    }

    let mut schlib_records = Vec::new();
    let mut pcblib_library = PcbLibrary::default();
    for exported in exported {
        if let Some(record) = exported.artifacts.schlib_record {
            schlib_records.push(record);
        }
        if let Some(library) = exported.artifacts.pcblib_library {
            append_pcblib_library_direct(&mut pcblib_library, library);
        }
    }

    progress.note("Writing merged library files...");
    if targets.schlib {
        if schlib_records.is_empty() {
            return Err(AppError::Other(
                "cannot write merged SchLib without any components".to_string(),
            ));
        }
        write_schlib_records(&schlib_records, &schlib_path)?;
        summary.generated_files.push(schlib_path);
    }
    if targets.pcblib {
        if pcblib_library.components.is_empty() {
            return Err(AppError::Other(
                "cannot write merged PcbLib without any components".to_string(),
            ));
        }
        write_pcblib(&pcblib_library, &pcblib_path)?;
        summary.generated_files.push(pcblib_path);
    }

    Ok(summary)
}

async fn export_batch_merged_append(
    client: &LcedaClient,
    options: BatchOptions,
    targets: ExportTargets,
    ids: Vec<String>,
    mut summary: BatchSummary,
    progress: &mut BatchProgress,
) -> Result<BatchSummary> {
    let library_name = resolve_library_name(&options);
    let merged_pcblib_file = format!("{}.PcbLib", sanitize_filename(&library_name));
    let schlib_path = options
        .output
        .join(format!("{}.SchLib", sanitize_filename(&library_name)));
    let pcblib_path = options
        .output
        .join(format!("{}.PcbLib", sanitize_filename(&library_name)));

    progress.note(format!("Library: {library_name}"));

    if targets.schlib && targets.pcblib {
        let sch_exists = schlib_path.exists();
        let pcb_exists = pcblib_path.exists();
        if sch_exists != pcb_exists {
            return Err(AppError::Other(
                "append mode requires both merged SchLib and PcbLib to exist already, or neither"
                    .to_string(),
            ));
        }
    }

    let mut schlib_records = if targets.schlib && schlib_path.exists() {
        read_schlib_records(&schlib_path)?
    } else {
        Vec::new()
    };
    let mut pcblib_library = if targets.pcblib && pcblib_path.exists() {
        read_pcblib_records(&pcblib_path)?
    } else {
        PcblibRecordLibrary::default()
    };

    if schlib_records.is_empty() && pcblib_library.components.is_empty() {
        progress.note("Append mode: no existing merged export library found, creating a new one");
    } else {
        progress.note(format!(
            "Loaded existing merged output: SchLib components: {} | PcbLib components: {}",
            schlib_records.len(),
            pcblib_library.components.len()
        ));
    }

    let mut known_identities = HashSet::new();
    let mut used_symbol_names = HashSet::new();
    let mut used_footprint_names = HashSet::new();
    for record in &schlib_records {
        used_symbol_names.insert(record.name.to_ascii_lowercase());
        if let Some(identity) = record.identity.as_deref().and_then(normalize_lcsc_id) {
            known_identities.insert(identity);
        }
    }
    for component in &pcblib_library.components {
        used_footprint_names.insert(component.name.to_ascii_lowercase());
    }

    let mut pending = Vec::new();
    for (index, id) in ids.into_iter().enumerate() {
        let normalized_id = normalize_lcsc_id(&id).unwrap_or_else(|| id.clone());
        if known_identities.contains(&normalized_id) {
            summary.skipped += 1;
            progress.record_skip(&id, "already present");
            continue;
        }
        pending.push(IndexedId { index, id });
    }

    let exported = export_merged_components_parallel(
        client,
        &options,
        targets,
        pending,
        &mut used_symbol_names,
        &mut used_footprint_names,
        &merged_pcblib_file,
        &mut summary,
        progress,
    )
    .await?;

    let added_any = !exported.is_empty();
    for exported in exported {
        known_identities.insert(exported.artifacts.identity);
        if let Some(record) = exported.artifacts.schlib_record {
            schlib_records.push(record);
        }
        // The part was already reduced to its streams by the task that built it.
        if let Some(records) = exported.artifacts.pcblib_records {
            append_pcblib_library(&mut pcblib_library, records);
        }
    }

    if added_any {
        progress.note("Writing merged library files...");
        write_merged_outputs(
            targets,
            &schlib_records,
            &pcblib_library,
            &schlib_path,
            &pcblib_path,
            &mut summary,
        )?;
    }

    Ok(summary)
}

/// Writes into a merged library that seex itself produced.
///
/// A library the user has marked read-only (or that a copy left read-only) cannot be
/// overwritten as it is, and an append into it would otherwise fail with a bare
/// "access is denied" that says nothing about which file is at fault.
fn make_writable(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    let permissions = metadata.permissions();
    if !permissions.readonly() {
        return Ok(());
    }
    fs::set_permissions(path, writable(permissions)).map_err(|err| {
        AppError::Other(format!(
            "the existing library {} is read-only and cannot be updated: {err}",
            path.display()
        ))
    })
}

/// The same permissions, with permission to write the file.
///
/// `set_readonly(false)` is only the right call on Windows. On Unix it clears
/// every write bit at once, which leaves the library world writable, so the
/// owner's bit is set instead and the rest of the mode is left alone.
#[cfg(unix)]
fn writable(permissions: fs::Permissions) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(permissions.mode() | 0o200)
}

#[cfg(not(unix))]
#[allow(clippy::permissions_set_readonly_false)] // the correct call on Windows
fn writable(mut permissions: fs::Permissions) -> fs::Permissions {
    permissions.set_readonly(false);
    permissions
}

fn write_merged_outputs(
    targets: ExportTargets,
    schlib_records: &[SchlibRecord],
    pcblib_library: &PcblibRecordLibrary,
    schlib_path: &Path,
    pcblib_path: &Path,
    summary: &mut BatchSummary,
) -> Result<()> {
    if targets.schlib {
        if schlib_records.is_empty() {
            return Err(AppError::Other(
                "cannot write merged SchLib without any components".to_string(),
            ));
        }
        make_writable(schlib_path)?;
        write_schlib_records(schlib_records, schlib_path)?;
        summary.generated_files.push(schlib_path.to_path_buf());
    }
    if targets.pcblib {
        if pcblib_library.components.is_empty() {
            return Err(AppError::Other(
                "cannot write merged PcbLib without any components".to_string(),
            ));
        }
        make_writable(pcblib_path)?;
        write_pcblib_records(pcblib_library, pcblib_path)?;
        summary.generated_files.push(pcblib_path.to_path_buf());
    }
    Ok(())
}

async fn export_merged_components_parallel(
    client: &LcedaClient,
    options: &BatchOptions,
    targets: ExportTargets,
    indexed_ids: Vec<IndexedId>,
    used_symbol_names: &mut HashSet<String>,
    used_footprint_names: &mut HashSet<String>,
    merged_pcblib_file: &str,
    summary: &mut BatchSummary,
    progress: &mut BatchProgress,
) -> Result<Vec<IndexedMergeArtifacts>> {
    if indexed_ids.is_empty() {
        return Ok(Vec::new());
    }

    let parallel = options.parallel.max(1).min(indexed_ids.len());
    let mut first_error = None;
    // Fetching stays continuous: every component is prepared at the batch's own
    // parallelism while the builds that consume them run behind it, rather than the
    // batch being cut into waves that each have to finish before the next starts.
    let mut prepared = prepare_merged_components_parallel(
        client,
        targets,
        indexed_ids,
        options.english_metadata,
        parallel,
        summary,
        progress,
        &mut first_error,
    )
    .await;

    if prepared.is_empty() {
        if let Some(err) = first_error {
            return Err(err);
        }
        return Ok(Vec::new());
    }
    if first_error.is_some() && !options.continue_on_error {
        return Err(first_error.unwrap());
    }

    prepared.sort_by_key(|prepared| prepared.index);
    let named = prepared
        .into_iter()
        .map(|prepared| {
            let component_name = merged_unique_name(
                &prepared.component_name_base,
                &prepared.id,
                used_symbol_names,
            );
            let footprint_name = merged_unique_name(
                &prepared.footprint_name_base,
                &prepared.id,
                used_footprint_names,
            );
            NamedMergeComponent {
                index: prepared.index,
                id: prepared.id,
                item: prepared.item,
                identity: prepared.identity,
                component_name,
                footprint_name,
                step_candidates: prepared.step_candidates,
                symbol_data: prepared.symbol_data,
                footprint_data: prepared.footprint_data,
                english_metadata: prepared.english_metadata,
            }
        })
        .collect();

    let mut exported = build_merged_components_parallel(
        client,
        targets,
        named,
        merged_pcblib_file,
        options.schematic_stroke_color,
        options.schematic_fill_color,
        &options.models,
        parallel,
        options.append,
        summary,
        progress,
        &mut first_error,
    )
    .await;

    if summary.failed > 0 && !options.continue_on_error {
        return Err(first_error.unwrap_or_else(|| AppError::Other("merge export failed".into())));
    }

    exported.sort_by_key(|exported| exported.index);
    Ok(exported)
}

async fn prepare_merged_components_parallel(
    client: &LcedaClient,
    targets: ExportTargets,
    indexed_ids: Vec<IndexedId>,
    english_metadata: bool,
    parallel: usize,
    summary: &mut BatchSummary,
    progress: &mut BatchProgress,
    first_error: &mut Option<AppError>,
) -> Vec<PreparedMergeComponent> {
    let semaphore = Arc::new(Semaphore::new(parallel));
    let mut join_set = JoinSet::new();

    for indexed in indexed_ids {
        let client = client.clone();
        let semaphore = semaphore.clone();
        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("merge prepare semaphore should remain open");
            let id = indexed.id;
            let result = prepare_merged_component(
                &client,
                targets,
                indexed.index,
                id.clone(),
                english_metadata,
            )
            .await;
            (id, result)
        });
    }

    let mut prepared = Vec::new();
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok((_id, Ok(item))) => prepared.push(item),
            Ok((id, Err(err))) => {
                summary.failed += 1;
                summary.failed_ids.push(id.clone());
                progress.record_failure(&id, &err);
                if first_error.is_none() {
                    *first_error = Some(err);
                }
            }
            Err(err) => {
                summary.failed += 1;
                let batch_err = AppError::Other(format!("merge prepare task join failed: {err}"));
                progress.record_failure("<join>", &batch_err);
                if first_error.is_none() {
                    *first_error = Some(batch_err);
                }
            }
        }
    }

    prepared
}

async fn prepare_merged_component(
    client: &LcedaClient,
    targets: ExportTargets,
    index: usize,
    id: String,
    english_metadata: bool,
) -> Result<PreparedMergeComponent> {
    let item = client.select_item(&id, 1).await?;
    let identity = item
        .lcsc_id()
        .as_deref()
        .and_then(normalize_lcsc_id)
        .unwrap_or_else(|| id.clone());
    let component_name_base = merged_base_name(item.display_name(), &id).to_string();

    let footprint_data = if targets.pcblib || item.footprint_uuid().is_some() {
        let footprint_uuid = item
            .footprint_uuid()
            .ok_or(AppError::MissingSymbolOrFootprint)?;
        Some(client.component_detail(&footprint_uuid).await?)
    } else {
        None
    };
    let footprint_name_base = footprint_data
        .as_ref()
        .map(|data| resolved_footprint_name(&item, data))
        .unwrap_or_else(|| component_name_base.clone());
    // The model ids are pulled out here, while the payload is in memory anyway, so the
    // build can start the download before reading the payload back.
    let step_candidates = footprint_data
        .as_ref()
        .map(|data| step_model_candidates(&item, data))
        .unwrap_or_default();
    let footprint_data = footprint_data.map(Payload::store);

    let symbol_data = if targets.schlib {
        let symbol_uuid = item
            .symbol_uuid()
            .ok_or(AppError::MissingSymbolOrFootprint)?;
        Some(Payload::store(client.component_detail(&symbol_uuid).await?))
    } else {
        None
    };

    let english_metadata = if targets.schlib && english_metadata {
        LcscClient::new().product_detail_optional(&identity).await
    } else {
        None
    };

    Ok(PreparedMergeComponent {
        index,
        id,
        item,
        identity,
        component_name_base,
        footprint_name_base,
        step_candidates,
        symbol_data,
        footprint_data,
        english_metadata,
    })
}

async fn build_merged_components_parallel(
    client: &LcedaClient,
    targets: ExportTargets,
    named: Vec<NamedMergeComponent>,
    merged_pcblib_file: &str,
    schematic_stroke_color: [u8; 3],
    schematic_fill_color: Option<[u8; 3]>,
    models: &Arc<ModelCleanup>,
    parallel: usize,
    as_records: bool,
    summary: &mut BatchSummary,
    progress: &mut BatchProgress,
    first_error: &mut Option<AppError>,
) -> Vec<IndexedMergeArtifacts> {
    let semaphore = Arc::new(Semaphore::new(parallel));
    let merged_pcblib_file = Arc::new(merged_pcblib_file.to_string());
    let mut join_set = JoinSet::new();

    for named in named {
        let client = client.clone();
        let semaphore = semaphore.clone();
        let merged_pcblib_file = merged_pcblib_file.clone();
        let models = Arc::clone(models);
        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("merge build semaphore should remain open");
            let id = named.id.clone();
            let result = build_named_merged_component(
                &client,
                targets,
                named,
                merged_pcblib_file.as_str(),
                schematic_stroke_color,
                schematic_fill_color,
                &models,
            )
            .await;
            // An append merge only needs the streams, and reducing the part here keeps
            // the structured library off the batch's peak.
            let result = if as_records {
                result.and_then(into_pcblib_records)
            } else {
                result
            };
            (id, result)
        });
    }

    let mut exported = Vec::new();
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok((id, Ok(item))) => {
                summary.success += 1;
                progress.record_success(&id, Some(&item.artifacts.component_name));
                exported.push(item);
            }
            Ok((id, Err(err))) => {
                summary.failed += 1;
                summary.failed_ids.push(id.clone());
                progress.record_failure(&id, &err);
                if first_error.is_none() {
                    *first_error = Some(err);
                }
            }
            Err(err) => {
                summary.failed += 1;
                let batch_err = AppError::Other(format!("merge build task join failed: {err}"));
                progress.record_failure("<join>", &batch_err);
                if first_error.is_none() {
                    *first_error = Some(batch_err);
                }
            }
        }
    }

    exported
}

async fn build_named_merged_component(
    client: &LcedaClient,
    targets: ExportTargets,
    named: NamedMergeComponent,
    merged_pcblib_file: &str,
    schematic_stroke_color: [u8; 3],
    schematic_fill_color: Option<[u8; 3]>,
    models: &ModelCleanup,
) -> Result<IndexedMergeArtifacts> {
    let schlib_record = if targets.schlib {
        let symbol_data = named
            .symbol_data
            .as_ref()
            .ok_or(AppError::MissingSymbolOrFootprint)?
            .load()?;
        let component = build_schlib_component_from_detail_with_color(
            &named.item,
            &symbol_data,
            &named.component_name,
            Some(&named.footprint_name),
            Some(merged_pcblib_file),
            named.english_metadata.as_ref(),
            color_bgr(schematic_stroke_color),
            fill_bgr(schematic_fill_color),
        )?;
        Some(schlib_record_from_component(&component)?)
    } else {
        None
    };

    let pcblib_library = if targets.pcblib {
        // The model download comes before the payload is read back: the payload is
        // only needed to build the footprint, and a download takes seconds while the
        // batch has one in flight per parallel part.
        let step_data =
            load_placed_step_for_pcblib(client, &named.item, &named.step_candidates, models).await;
        let footprint_data = named
            .footprint_data
            .as_ref()
            .ok_or(AppError::MissingSymbolOrFootprint)?
            .load()?;
        Some(build_pcblib_from_payload_with_model(
            &footprint_data,
            &named.footprint_name,
            step_data,
        )?)
    } else {
        None
    };

    Ok(IndexedMergeArtifacts {
        index: named.index,
        artifacts: MergeArtifacts {
            identity: named.identity,
            component_name: named.component_name,
            schlib_record,
            pcblib_library,
            pcblib_records: None,
        },
    })
}

fn reject_existing_merged_outputs(
    targets: ExportTargets,
    schlib_path: &Path,
    pcblib_path: &Path,
) -> Result<()> {
    let mut existing = Vec::new();
    if targets.schlib && schlib_path.exists() {
        existing.push(schlib_path.display().to_string());
    }
    if targets.pcblib && pcblib_path.exists() {
        existing.push(pcblib_path.display().to_string());
    }
    if existing.is_empty() {
        return Ok(());
    }

    Err(AppError::Other(format!(
        "merged output already exists: {}. Use --append to add components, or choose a new --library-name/delete the existing library first.",
        existing.join(", ")
    )))
}

fn append_pcblib_library_direct(target: &mut PcbLibrary, source: PcbLibrary) {
    target.components.extend(source.components);
    target.models.extend(source.models);
}

fn merged_base_name<'a>(base: &'a str, lcsc_id: &'a str) -> &'a str {
    let trimmed = base.trim();
    if trimmed.is_empty() { lcsc_id } else { trimmed }
}

fn merged_unique_name(base: &str, lcsc_id: &str, used_names: &mut HashSet<String>) -> String {
    let base = merged_base_name(base, lcsc_id);
    let normalized_base = base.to_ascii_lowercase();
    if used_names.insert(normalized_base) {
        return base.to_string();
    }

    let with_id = format!("{base}_{lcsc_id}");
    let normalized_with_id = with_id.to_ascii_lowercase();
    if used_names.insert(normalized_with_id) {
        return with_id;
    }

    let mut index = 2usize;
    loop {
        let candidate = format!("{base}_{lcsc_id}_{index}");
        if used_names.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
        index += 1;
    }
}

fn append_pcblib_library(target: &mut PcblibRecordLibrary, source: PcblibRecordLibrary) {
    target.components.extend(source.components);
    target.models.extend(source.models);
}

fn resolve_library_name(options: &BatchOptions) -> String {
    if let Some(name) = options.library_name.as_deref() {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    options
        .input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("MergedLib")
        .to_string()
}

async fn run_sequential(
    client: LcedaClient,
    options: Arc<BatchOptions>,
    targets: ExportTargets,
    checkpoint_path: &Path,
    pending: Vec<String>,
    summary: &mut BatchSummary,
    progress: &mut BatchProgress,
) -> Result<()> {
    for id in pending {
        match export_component(&client, &options, targets, &id).await {
            Ok(exported) => {
                append_checkpoint(checkpoint_path, &id)?;
                summary.success += 1;
                progress.record_success(&id, Some(&exported.display_name));
            }
            Err(err) => {
                summary.failed += 1;
                summary.failed_ids.push(id.clone());
                progress.record_failure(&id, &err);
                if !options.continue_on_error {
                    return Err(err);
                }
            }
        }
    }

    Ok(())
}

async fn run_parallel(
    client: LcedaClient,
    options: Arc<BatchOptions>,
    targets: ExportTargets,
    checkpoint_path: &Path,
    pending: Vec<String>,
    summary: &mut BatchSummary,
    progress: &mut BatchProgress,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(options.parallel));
    let mut join_set = JoinSet::new();

    for id in pending {
        let client = client.clone();
        let options = options.clone();
        let semaphore = semaphore.clone();
        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("batch semaphore should remain open");
            let result = export_component(&client, &options, targets, &id).await;
            (id, result)
        });
    }

    let mut first_error = None;
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok((id, Ok(exported))) => {
                append_checkpoint(checkpoint_path, &id)?;
                summary.success += 1;
                progress.record_success(&id, Some(&exported.display_name));
            }
            Ok((id, Err(err))) => {
                summary.failed += 1;
                summary.failed_ids.push(id.clone());
                progress.record_failure(&id, &err);
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
            Err(err) => {
                summary.failed += 1;
                let batch_err = AppError::Other(format!("batch task join failed: {err}"));
                progress.record_failure("<join>", &batch_err);
                if first_error.is_none() {
                    first_error = Some(batch_err);
                }
            }
        }
    }

    if summary.failed > 0 && !options.continue_on_error {
        return Err(first_error.unwrap_or_else(|| AppError::Other("batch export failed".into())));
    }

    Ok(())
}

async fn export_component(
    client: &LcedaClient,
    options: &BatchOptions,
    targets: ExportTargets,
    lcsc_id: &str,
) -> Result<ExportedComponent> {
    let item = client.select_item(lcsc_id, 1).await?;
    let display_name = item.display_name().to_string();

    if targets.schlib {
        let schlib_dir = options.output.join("schlib");
        export_schlib_with_options(
            client,
            &item,
            &schlib_dir,
            options.force,
            options.english_metadata,
            color_bgr(options.schematic_stroke_color),
            fill_bgr(options.schematic_fill_color),
        )
        .await?;
    }

    if targets.pcblib {
        let pcblib_dir = options.output.join("pcblib");
        export_pcblib(client, &item, &pcblib_dir, options.force, &options.models).await?;
    }

    Ok(ExportedComponent { display_name })
}

fn load_checkpoint(path: &Path) -> Result<HashSet<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(err) => Err(err.into()),
    }
}

fn append_checkpoint(path: &Path, id: &str) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{id}")?;
    Ok(())
}

pub(crate) fn parse_lcsc_ids(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'C' || byte == b'c' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }

            if end > start {
                let digits = std::str::from_utf8(&bytes[start..end])
                    .expect("ASCII digits must be valid UTF-8");
                let id = format!("C{digits}");
                if seen.insert(id.clone()) {
                    ids.push(id);
                }
                index = end;
                continue;
            }
        }

        index += 1;
    }

    ids
}

fn batch_mode_label(options: &BatchOptions) -> &'static str {
    match (options.merge, options.append) {
        (true, true) => "merge+append",
        (true, false) => "merge",
        (false, _) => "batch",
    }
}

fn format_elapsed(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BatchOptions, BatchProgress, ExportTargets, format_elapsed, make_writable, parse_lcsc_ids,
        reject_existing_merged_outputs, resolve_library_name,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn makes_a_read_only_library_writable_again() {
        let path = std::env::temp_dir().join(format!(
            "seex_readonly_{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"library").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        assert!(fs::metadata(&path).unwrap().permissions().readonly());

        // Whether a write is actually refused in this state depends on the
        // user: root writes to a read-only file regardless, and the containers
        // the distro builds run in are root. That is a property of the platform,
        // not of seex; what seex has to do is clear the flag itself, so the flag
        // is what is checked either side of the call.
        make_writable(&path).unwrap();
        assert!(!fs::metadata(&path).unwrap().permissions().readonly());
        fs::write(&path, b"again").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"again");
        fs::remove_file(path).ok();
    }

    #[test]
    fn parse_ids_deduplicates_and_preserves_order() {
        let ids = parse_lcsc_ids("C2040\nfoo C5676243 bar c2040 baz C42");
        assert_eq!(ids, vec!["C2040", "C5676243", "C42"]);
    }

    #[test]
    fn parse_ids_ignores_invalid_matches() {
        let ids = parse_lcsc_ids("C abc c-1 test");
        assert!(ids.is_empty());
    }

    #[test]
    fn resolve_library_name_defaults_to_input_stem() {
        let options = BatchOptions {
            input: PathBuf::from("ids.txt"),
            output: PathBuf::from("out"),
            schlib: true,
            pcblib: false,
            full: false,
            merge: true,
            append: false,
            library_name: None,
            parallel: 1,
            continue_on_error: false,
            english_metadata: false,
            force: false,
            schematic_stroke_color: [11, 56, 104],
            schematic_fill_color: Some([255, 255, 224]),
            models: Default::default(),
            progress: None,
        };

        assert_eq!(resolve_library_name(&options), "ids");
    }

    #[test]
    fn fresh_merge_rejects_existing_selected_output() {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let schlib_path =
            std::env::temp_dir().join(format!("export_merge_guard_{timestamp}.SchLib"));
        let pcblib_path =
            std::env::temp_dir().join(format!("export_merge_guard_{timestamp}.PcbLib"));
        fs::write(&schlib_path, b"old library").unwrap();

        let err = reject_existing_merged_outputs(
            ExportTargets {
                schlib: true,
                pcblib: false,
            },
            &schlib_path,
            &pcblib_path,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("already exists"));
        assert!(err.contains("--append"));
        fs::remove_file(schlib_path).ok();
    }

    #[test]
    fn formats_elapsed_time_without_hours() {
        assert_eq!(format_elapsed(65), "01:05");
    }

    fn progress_with(queue: Vec<&str>, parallel: usize) -> BatchProgress {
        BatchProgress::new(
            queue.len(),
            parallel,
            "test",
            ExportTargets {
                schlib: true,
                pcblib: true,
            },
            &PathBuf::from("out"),
            queue.iter().map(|id| id.to_string()).collect(),
            None,
        )
    }

    #[test]
    fn the_bar_names_the_component_being_fetched() {
        let mut progress = progress_with(vec!["C1", "C2", "C3"], 2);

        // This used to read `0/3: -`: the subject only ever held a finished
        // component, so the bar said nothing until the first one landed.
        assert_eq!(progress.subject(), "Fetching C1");

        progress.completed = 1;
        assert_eq!(progress.subject(), "Fetching C2");

        progress.completed = 2;
        assert_eq!(progress.subject(), "Fetching C3");

        // With nothing left in flight, the last component is the useful line,
        // in the words the KiCad batch uses for the same step.
        progress.completed = 3;
        progress.record_success("C3", Some("RP2040"));
        assert_eq!(progress.subject(), "Exported C3 RP2040");
    }

    #[test]
    fn a_lone_component_is_named_without_a_count() {
        let progress = progress_with(vec!["C1"], 1);

        assert_eq!(progress.subject(), "Fetching C1");
    }

    #[test]
    fn a_run_with_nothing_to_do_says_so() {
        let progress = progress_with(Vec::new(), 1);

        assert_eq!(progress.subject(), "Starting 0 components");
    }
}
