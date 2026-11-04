//! Offline merging of modern KiCad libraries. All parsing and collision checks
//! finish before staged outputs replace any destination.
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::sexpr::{self, Node};
use crate::error::{AppError, Result};

#[derive(Debug)]
pub struct DirectoryMergeSummary {
    pub symbol_files: usize,
    pub footprint_libraries: usize,
    pub model_libraries: usize,
    pub symbols: usize,
    pub footprints: usize,
    pub models: usize,
    pub outputs: Vec<PathBuf>,
}

#[derive(Default)]
struct Inputs {
    symbols: Vec<PathBuf>,
    footprints: Vec<PathBuf>,
    models: Vec<PathBuf>,
}

impl Inputs {
    fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.symbols
            .iter()
            .chain(&self.footprints)
            .chain(&self.models)
    }
}

pub fn directory_has_libraries(input: &Path) -> Result<bool> {
    Ok(discover(input)?.paths().next().is_some())
}

fn discover(input: &Path) -> Result<Inputs> {
    let mut result = Inputs::default();
    for path in entries(input)? {
        match extension(&path).as_str() {
            "kicad_sym" if path.is_file() => result.symbols.push(path),
            "pretty" if path.is_dir() => result.footprints.push(path),
            "3dshapes" if path.is_dir() => result.models.push(path),
            _ => (),
        }
    }
    Ok(result)
}

fn extension(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

fn entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            return Err(AppError::Other(format!(
                "merge does not follow symbolic links: {}",
                entry.path().display()
            )));
        }
        paths.push(entry.path());
    }
    paths.sort();
    Ok(paths)
}

fn name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| AppError::Other(format!("invalid library name: {}", path.display())))
}

fn reserve(
    seen: &mut HashMap<String, (PathBuf, Vec<u8>)>,
    key: &str,
    source: &Path,
    kind: &str,
    content: Vec<u8>,
) -> Result<bool> {
    if let Some((previous, previous_content)) = seen.get(&key.to_lowercase()) {
        if *previous_content == content {
            return Ok(false);
        }
        return Err(AppError::Other(format!(
            "conflicting duplicate {kind} '{key}' in {} and {}",
            previous.display(),
            source.display()
        )));
    }
    seen.insert(key.to_lowercase(), (source.to_owned(), content));
    Ok(true)
}

// Compare parsed content so harmless whitespace changes do not create conflicts.
fn semantic_content(text: &str) -> Result<Vec<u8>> {
    fn render(node: &Node, out: &mut String) {
        if let Some(atom) = &node.atom {
            out.push_str(&sexpr::quote(atom));
        } else {
            out.push('(');
            for child in &node.children {
                render(child, out);
                out.push(' ');
            }
            out.push(')');
        }
    }
    let mut out = String::new();
    render(&sexpr::parse(text)?, &mut out);
    Ok(out.into_bytes())
}

fn read_document(path: &Path, expected: &[&str]) -> Result<(String, Node)> {
    let text = fs::read_to_string(path)?;
    let parsed =
        sexpr::parse(&text).map_err(|e| AppError::Other(format!("{}: {e}", path.display())))?;
    if !expected.contains(&parsed.head().unwrap_or_default()) {
        return Err(AppError::Other(format!(
            "{}: expected {} document",
            path.display(),
            expected.join(" or ")
        )));
    }
    Ok((text, parsed))
}

/// `output_base` is a library base path, with no extension. A known KiCad
/// extension is also accepted. Other dots are retained in the library name.
pub fn merge_library_directory(
    input: &Path,
    output_base: &Path,
    force: bool,
) -> Result<DirectoryMergeSummary> {
    let mut inputs = discover(input)?;
    if inputs.paths().next().is_none() {
        return Err(AppError::Other("no KiCad libraries found".into()));
    }
    let base = if matches!(
        extension(output_base).as_str(),
        "kicad_sym" | "pretty" | "3dshapes"
    ) {
        output_base.with_extension("")
    } else {
        output_base.to_owned()
    };
    let library_name = name(&base)?;
    if library_name.contains([':', '"', '\\', '/', '$', '{', '}'])
        || library_name.chars().any(char::is_control)
    {
        return Err(AppError::Other(
            "output name contains invalid KiCad library characters".into(),
        ));
    }
    let parent = base
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let target = |ext: &str| parent.join(format!("{library_name}.{ext}"));
    // A previous destination is never also a source. Resolve aliases so ./ and
    // absolute spellings behave identically; nested overlap is still rejected.
    let destinations = ["kicad_sym", "pretty", "3dshapes"]
        .map(|ext| resolved_path(&target(ext)))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    for paths in [
        &mut inputs.symbols,
        &mut inputs.footprints,
        &mut inputs.models,
    ] {
        let mut retained = Vec::new();
        for path in paths.drain(..) {
            if !destinations.contains(&path.canonicalize()?) {
                retained.push(path);
            }
        }
        *paths = retained;
    }
    if inputs.paths().next().is_none() {
        return Err(AppError::Other(
            "no source KiCad libraries remain after excluding the output libraries".into(),
        ));
    }
    let mut outputs = Vec::new();
    if !inputs.symbols.is_empty() {
        outputs.push(target("kicad_sym"));
    }
    if !inputs.footprints.is_empty() {
        outputs.push(target("pretty"));
    }
    if !inputs.models.is_empty() {
        outputs.push(target("3dshapes"));
    }
    for output in &outputs {
        // Resolve existing ancestors too, preventing force from deleting a source
        // via '..', a relative spelling, or a directory alias.
        let resolved = resolved_path(output)?;
        for source in inputs.paths() {
            let source = source.canonicalize()?;
            if resolved.starts_with(&source) || source.starts_with(&resolved) {
                return Err(AppError::Other(format!(
                    "merge output overlaps an input library: {}",
                    output.display()
                )));
            }
        }
        if output.exists() && !force {
            return Err(AppError::Other(format!(
                "merge output already exists: {}. Pass --force to replace it",
                output.display()
            )));
        }
        if let Ok(meta) = fs::symlink_metadata(output) {
            if meta.file_type().is_symlink() || (extension(output) == "kicad_sym") != meta.is_file()
            {
                return Err(AppError::Other(format!(
                    "unexpected output file type: {}",
                    output.display()
                )));
            }
        }
    }

    let mut files: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    let mut model_refs = HashMap::new();
    let mut model_names = HashMap::new();
    for dir in &inputs.models {
        let folder = name(dir)?;
        let source_lib = dir.file_stem().unwrap().to_string_lossy();
        let mut pending = vec![dir.clone()];
        while let Some(current) = pending.pop() {
            for path in entries(&current)? {
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let relative = path.strip_prefix(dir).expect("child of model directory");
                let relative_text = relative.to_string_lossy().replace('\\', "/");
                let data = fs::read(&path)?;
                let added = reserve(
                    &mut model_names,
                    &relative_text,
                    &path,
                    "model path",
                    data.clone(),
                )?;
                let destination = target("3dshapes").join(relative);
                let absolute = model_path_text(&resolved_path(&destination)?);
                // Match only known files; references to standard/external libraries
                // stay exactly as they were. Absolute destinations need no new
                // environment variable and work regardless of the project folder.
                for reference in [
                    format!("${{{source_lib}}}/{folder}/{relative_text}"),
                    format!("${{KIPRJMOD}}/{folder}/{relative_text}"),
                    format!("{folder}/{relative_text}"),
                    format!("../{folder}/{relative_text}"),
                    model_path_text(&path.canonicalize()?),
                ] {
                    model_refs.insert(reference, absolute.clone());
                }
                if added {
                    files.insert(
                        PathBuf::from(format!("{library_name}.3dshapes")).join(relative),
                        data,
                    );
                }
            }
        }
    }

    let mut footprint_refs = HashMap::new();
    let mut footprint_names = HashMap::new();
    for dir in &inputs.footprints {
        let source_lib = dir.file_stem().unwrap().to_string_lossy();
        for path in entries(dir)? {
            if !path.is_file() || extension(&path) != "kicad_mod" {
                continue;
            }
            let filename = name(&path)?;
            let footprint_name = path.file_stem().unwrap().to_string_lossy();
            footprint_refs.insert(
                format!("{source_lib}:{footprint_name}"),
                format!("{library_name}:{footprint_name}"),
            );
            let (text, root) = read_document(&path, &["footprint", "module"])?;
            let mut edits = Vec::new();
            for child in &root.children {
                if child.head() == Some("model")
                    && let Some(reference) = child.value(1)
                {
                    let normalized = reference.replace('\\', "/");
                    let mapped = model_refs
                        .get(&normalized)
                        .or_else(|| {
                            // npnp 3.0 wrote _C123 references but __C123 model files.
                            // Repair only when that exact included model exists.
                            legacy_model_reference(&normalized)
                                .and_then(|fixed| model_refs.get(&fixed))
                        })
                        .or_else(|| {
                            // Resolve relative and absolute local paths when possible.
                            let candidate = dir.join(reference).canonicalize().ok()?;
                            model_refs.get(&model_path_text(&candidate))
                        });
                    if let Some(replacement) = mapped {
                        edits.push((child.children[1].span.clone(), sexpr::quote(replacement)));
                    }
                }
            }
            let data = sexpr::replace(&text, edits);
            if reserve(
                &mut footprint_names,
                &filename,
                &path,
                "footprint filename",
                semantic_content(&data)?,
            )? {
                files.insert(
                    PathBuf::from(format!("{library_name}.pretty")).join(filename),
                    data.into_bytes(),
                );
            }
        }
    }

    let mut symbol_names = HashMap::new();
    let mut inheritance = HashMap::new();
    let mut symbols = Vec::new();
    let mut version = 0u32;
    for path in &inputs.symbols {
        let (text, root) = read_document(path, &["kicad_symbol_lib"])?;
        let source_version = root
            .children
            .iter()
            .find(|n| n.head() == Some("version"))
            .and_then(|n| n.value(1))
            .and_then(|v| v.parse::<u32>().ok())
            .ok_or_else(|| {
                AppError::Other(format!("{}: missing KiCad library version", path.display()))
            })?;
        version = version.max(source_version);
        for child in root.children.iter().skip(1) {
            match child.head() {
                Some("version" | "generator" | "generator_version") => (),
                Some("symbol") => {
                    let symbol_name = child.value(1).ok_or_else(|| {
                        AppError::Other(format!("{}: unnamed symbol", path.display()))
                    })?;
                    let mut edits = Vec::new();
                    for field in &child.children {
                        if field.head() == Some("property")
                            && field.value(1) == Some("Footprint")
                            && let Some(reference) = field.value(2)
                            && let Some(replacement) = footprint_refs.get(reference)
                        {
                            let span = field.children[2].span.clone();
                            edits.push((
                                span.start - child.span.start..span.end - child.span.start,
                                sexpr::quote(replacement),
                            ));
                        }
                        if field.head() == Some("extends")
                            && let Some(parent) = field.value(1)
                        {
                            inheritance.insert(symbol_name.to_owned(), parent.to_owned());
                        }
                    }
                    let data = sexpr::replace(&text[child.span.clone()], edits);
                    if reserve(
                        &mut symbol_names,
                        symbol_name,
                        path,
                        "symbol name",
                        semantic_content(&data)?,
                    )? {
                        symbols.push(data);
                    }
                }
                _ => {
                    return Err(AppError::Other(format!(
                        "{}: unsupported library-level field {:?}; merge stopped to avoid losing data",
                        path.display(),
                        child.head()
                    )));
                }
            }
        }
    }
    for symbol in inheritance.keys() {
        let mut seen = std::collections::HashSet::new();
        let mut current = symbol.as_str();
        while let Some(parent) = inheritance.get(current) {
            if !seen.insert(current) || !symbol_names.contains_key(&parent.to_lowercase()) {
                return Err(AppError::Other(format!(
                    "invalid symbol inheritance for {symbol}: missing parent or cycle"
                )));
            }
            current = parent;
        }
    }
    if !inputs.symbols.is_empty() {
        let content = format!(
            "(kicad_symbol_lib\n  (version {version})\n  (generator npnp)\n{}\n)\n",
            symbols.join("\n")
        );
        files.insert(
            PathBuf::from(format!("{library_name}.kicad_sym")),
            content.into_bytes(),
        );
    }

    publish(parent, &outputs, files, force)?;
    Ok(DirectoryMergeSummary {
        symbol_files: inputs.symbols.len(),
        footprint_libraries: inputs.footprints.len(),
        model_libraries: inputs.models.len(),
        symbols: symbols.len(),
        footprints: footprint_names.len(),
        models: model_names.len(),
        outputs,
    })
}

fn legacy_model_reference(reference: &str) -> Option<String> {
    let (stem, extension) = reference.rsplit_once('.')?;
    if !extension.eq_ignore_ascii_case("step") {
        return None;
    }
    let (prefix, digits) = stem.rsplit_once("_C")?;
    if prefix.ends_with('_') || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{prefix}__C{digits}.{extension}"))
}

fn model_path_text(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if let Some(unc) = text.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else {
        text.strip_prefix("//?/").unwrap_or(&text).to_owned()
    }
}

fn resolved_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if absolute.exists() {
        return Ok(absolute.canonicalize()?);
    }
    let parent = absolute
        .parent()
        .ok_or_else(|| AppError::Other("invalid output path".into()))?;
    Ok(resolved_path(parent)?.join(
        absolute
            .file_name()
            .ok_or_else(|| AppError::Other("invalid output name".into()))?,
    ))
}

static STAGE_ID: AtomicU64 = AtomicU64::new(0);

fn publish(
    parent: &Path,
    outputs: &[PathBuf],
    files: BTreeMap<PathBuf, Vec<u8>>,
    force: bool,
) -> Result<()> {
    fs::create_dir_all(parent)?;
    let stage = loop {
        let id = STAGE_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".seex-merge-{}-{id}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    let prepared = (|| -> Result<()> {
        for output in outputs {
            if extension(output) != "kicad_sym" {
                fs::create_dir(stage.join(name(output)?))?;
            }
        }
        for (relative, data) in files {
            let path = stage.join(relative);
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, data)?;
        }
        Ok(())
    })();
    if let Err(e) = prepared {
        fs::remove_dir_all(&stage).ok();
        return Err(e);
    }
    let mut backups = Vec::new();
    let mut installed = Vec::new();
    let committed = (|| -> Result<()> {
        for (index, output) in outputs.iter().enumerate() {
            if output.exists() {
                if !force {
                    return Err(AppError::Other(format!(
                        "Output appeared during merge: {}",
                        output.display()
                    )));
                }
                let backup = stage.join(format!("backup-{index}"));
                fs::rename(output, &backup)?;
                backups.push((output, backup));
            }
            fs::rename(stage.join(name(output)?), output)?;
            installed.push(output);
        }
        Ok(())
    })();
    if let Err(error) = committed {
        let mut rollback_errors = Vec::new();
        for output in installed.into_iter().rev() {
            if let Err(e) = fs::rename(output, stage.join(name(output)?)) {
                rollback_errors.push(e.to_string());
            }
        }
        for (output, backup) in backups.into_iter().rev() {
            if let Err(e) = fs::rename(backup, output) {
                rollback_errors.push(e.to_string());
            }
        }
        if !rollback_errors.is_empty() {
            return Err(AppError::Other(format!(
                "{error}; rollback failed: {}. Recovery files: {}",
                rollback_errors.join(", "),
                stage.display()
            )));
        }
        fs::remove_dir_all(&stage).ok();
        return Err(error);
    }
    fs::remove_dir_all(&stage).map_err(|e| {
        AppError::Other(format!(
            "merge succeeded but could not remove staging directory {}: {e}",
            stage.display()
        ))
    })?;
    Ok(())
}
