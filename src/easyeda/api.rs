use crate::easyeda::models::{ApiResponse, ComponentData, Model3dInfo, SymbolPartData};
use crate::error::{AppError, EasyedaError, Result};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OnceCell};

const MAX_REQUEST_ATTEMPTS: u32 = 4;
const RETRY_BASE_DELAY_MS: u64 = 250;
const STEP_DOWNLOAD_ATTEMPTS: u32 = 2;
/// Detailed models reach 10 MB; through the easyeda.com mirror one can take well
/// over a minute.
const STEP_DOWNLOAD_TOTAL_TIMEOUT_SECS: u64 = 180;
const STEP_DOWNLOAD_STALL_TIMEOUT_SECS: u64 = 20;
const UPSTREAM_BLOCK_RETRY_DELAY_MS: u64 = 10_000;
const UPSTREAM_BLOCK_COOLDOWN_MS: u64 = 45_000;
const COMPONENT_REQUEST_INTERVAL_MS: u64 = 150;
const MODEL_REQUEST_INTERVAL_MS: u64 = 150;
const MODEL_DOWNLOAD_BASE_URL: &str = "https://modules.lceda.cn/qAxj6KHrDKw4blvCG8QJPs7Y";
/// The same models on EasyEDA's international host, used when lceda.cn cannot be
/// reached from the user's network (seen as a failed TLS handshake).
const MODEL_DOWNLOAD_MIRROR_URL: &str = "https://modules.easyeda.com/qAxj6KHrDKw4blvCG8QJPs7Y";
/// How many downloaded models the cache keeps.
///
/// The models are the largest thing a run holds: a 25-part batch of them was
/// 40MB on disk, and every one of them stayed in memory until the batch ended,
/// so the peak followed the size of the batch rather than the handful being
/// worked on at once. A model is asked for once, or twice when a bundle writes
/// it as a file and embeds it as well, so a few entries are enough to keep that
/// from downloading twice, and the rest are released as they are finished with.
const CACHED_MODELS: usize = 4;
type DownloadCell = Arc<OnceCell<std::result::Result<Arc<Vec<u8>>, String>>>;
type DownloadCache = Arc<Mutex<HashMap<String, DownloadCell>>>;

pub struct EasyedaApi {
    client: Client,
    model_client: Client,
    step_cache: DownloadCache,
    component_gate: Arc<Mutex<RequestGate>>,
    model_gate: Arc<Mutex<RequestGate>>,
}

/// Where a downloaded model goes.
///
/// A merge asks for one model per parallel part, so a batch of a few hundred parts
/// held dozens of them in memory while they arrived. Writing the download straight
/// into a file keeps only the chunk being received in memory; the caller reads the
/// model back when it has to look at the geometry.
#[derive(Debug, Clone, Copy)]
enum ModelSink<'a> {
    Memory,
    File(&'a Path),
}

/// One HTTP client per kind of request for the whole process.
///
/// An [`EasyedaApi`] is built per call in a few places, and a fresh client would
/// open a fresh connection for every one of them — a batch makes hundreds of these
/// calls, so the connections are worth keeping. Cloning a `Client` shares its pool.
fn shared_client(for_models: bool) -> Client {
    static COMPONENT_CLIENT: OnceLock<Client> = OnceLock::new();
    static MODEL_CLIENT: OnceLock<Client> = OnceLock::new();
    let cell = if for_models {
        &MODEL_CLIENT
    } else {
        &COMPONENT_CLIENT
    };
    cell.get_or_init(|| {
        let idle_per_host = if for_models { 32 } else { 10 };
        Client::builder()
            .user_agent(format!("seex/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(5))
            .read_timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(idle_per_host)
            .http2_adaptive_window(true)
            .build()
            .expect("Failed to create HTTP client")
    })
    .clone()
}

#[derive(Debug)]
struct RequestGate {
    next_ready: Instant,
}

impl RequestGate {
    fn new() -> Self {
        Self {
            next_ready: Instant::now(),
        }
    }
}

impl EasyedaApi {
    pub fn new() -> Self {
        Self {
            client: shared_client(false),
            model_client: shared_client(true),
            step_cache: Arc::new(Mutex::new(HashMap::new())),
            component_gate: Arc::new(Mutex::new(RequestGate::new())),
            model_gate: Arc::new(Mutex::new(RequestGate::new())),
        }
    }

    pub async fn get_component_data(&self, lcsc_id: &str) -> Result<ComponentData> {
        let mut last_error = None;

        for attempt in 0..MAX_REQUEST_ATTEMPTS {
            match self.get_component_data_once(lcsc_id).await {
                Ok(data) => return Ok(data),
                Err(e) if attempt + 1 < MAX_REQUEST_ATTEMPTS && should_retry_app_error(&e) => {
                    log::debug!(
                        "Failed to fetch component data for {} (retry {}/{}): {}",
                        lcsc_id,
                        attempt + 1,
                        MAX_REQUEST_ATTEMPTS,
                        e
                    );
                    let delay = retry_delay_for_error(&e, attempt);
                    last_error = Some(e);
                    tokio::time::sleep(delay).await;
                }
                Err(e) => return Err(e),
            }
        }

        Err(last_error.expect("retry loop should preserve the last error"))
    }

    async fn get_component_data_once(&self, lcsc_id: &str) -> Result<ComponentData> {
        wait_for_request_slot(
            &self.component_gate,
            Duration::from_millis(COMPONENT_REQUEST_INTERVAL_MS),
        )
        .await;

        let url = format!(
            "https://easyeda.com/api/products/{}/components?version=6.4.19.5",
            lcsc_id
        );

        log::debug!("Fetching component data for {}", lcsc_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(EasyedaError::ApiRequest)?;

        let status = response.status();
        if !status.is_success() {
            let message = response
                .text()
                .await
                .unwrap_or_default()
                .trim()
                .chars()
                .take(200)
                .collect::<String>();
            if status == StatusCode::NOT_FOUND {
                return Err(EasyedaError::ComponentNotFound(lcsc_id.to_string()).into());
            }
            let message = summarize_http_error_body(message);
            if is_upstream_block_status(status, &message) {
                extend_request_cooldown(
                    &self.component_gate,
                    Duration::from_millis(UPSTREAM_BLOCK_COOLDOWN_MS),
                )
                .await;
            }
            return Err(EasyedaError::HttpStatus {
                lcsc_id: lcsc_id.to_string(),
                status,
                message,
            }
            .into());
        }

        let api_response: ApiResponse = response
            .json()
            .await
            .map_err(|e| EasyedaError::InvalidData(format!("Failed to parse JSON: {}", e)))?;

        if !api_response.success {
            let message = api_response_message(&api_response);
            if is_not_found_api_message(&message) {
                return Err(EasyedaError::ComponentNotFound(lcsc_id.to_string()).into());
            }
            return Err(EasyedaError::ApiResponse {
                lcsc_id: lcsc_id.to_string(),
                message,
            }
            .into());
        }

        let result = api_response
            .result
            .ok_or_else(|| EasyedaError::InvalidData("Missing result field".to_string()))?;

        let data_str_obj = result
            .data_str
            .as_ref()
            .ok_or_else(|| EasyedaError::InvalidData("Missing dataStr field".to_string()))?;

        log::debug!("data_str_obj type: {:?}", data_str_obj);

        let bbox_x = data_str_obj
            .get("head")
            .and_then(|h| h.get("x"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let bbox_y = data_str_obj
            .get("head")
            .and_then(|h| h.get("y"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        log::debug!("Extracted bbox: x={}, y={}", bbox_x, bbox_y);

        let data_str =
            if let Some(shape_array) = data_str_obj.get("shape").and_then(|v| v.as_array()) {
                log::debug!("Found shape array with {} elements", shape_array.len());
                shape_array
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            } else {
                log::warn!("data_str_obj doesn't have shape array");
                vec![]
            };

        log::debug!("Final data_str has {} shapes", data_str.len());

        log::debug!("API result has {} raw subparts", result.subparts.len());

        let symbol_parts = result
            .subparts
            .iter()
            .filter_map(|subpart| {
                let data_str_obj = subpart.data_str.as_ref()?;
                let shapes = extract_shape_strings(data_str_obj);
                if shapes.is_empty() {
                    return None;
                }

                Some(SymbolPartData {
                    title: subpart.title.clone().unwrap_or_default(),
                    data_str: shapes,
                    bbox_x: symbol_bbox_x(data_str_obj),
                    bbox_y: symbol_bbox_y(data_str_obj),
                })
            })
            .collect::<Vec<_>>();

        if !symbol_parts.is_empty() {
            log::debug!("Found {} symbol subparts", symbol_parts.len());
        }

        let title = result
            .title
            .ok_or_else(|| EasyedaError::InvalidData("Missing title field".to_string()))?;

        let designator = data_str_obj
            .get("head")
            .and_then(|head| {
                head.get("c_para")
                    .and_then(|params| params.get("pre"))
                    .and_then(|value| value.as_str())
                    .or_else(|| head.get("pre").and_then(|value| value.as_str()))
            })
            .unwrap_or_default()
            .trim()
            .to_string();

        let manufacturer = data_str_obj
            .get("head")
            .and_then(|h| h.get("c_para"))
            .and_then(|cp| cp.get("BOM_Manufacturer"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let package = data_str_obj
            .get("head")
            .and_then(|h| h.get("c_para"))
            .and_then(|cp| cp.get("package"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let szlcsc_id = result
            .lcsc
            .as_ref()
            .and_then(|lcsc| lcsc.get("id"))
            .and_then(|v| v.as_u64());

        let datasheet = result
            .lcsc
            .as_ref()
            .and_then(|lcsc| lcsc.get("url"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                if let Some(id) = szlcsc_id {
                    format!("https://item.szlcsc.com/datasheet/{}/{}.html", title, id)
                } else {
                    String::new()
                }
            });

        let jlc_id = data_str_obj
            .get("head")
            .and_then(|h| h.get("c_para"))
            .and_then(|cp| cp.get("BOM_JLCPCB Part Class"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Build description: use API description if available, otherwise generate from metadata
        let description = match result.description {
            Some(ref d) if !d.is_empty() => d.clone(),
            _ => {
                let mut parts = Vec::new();
                if !manufacturer.is_empty() {
                    parts.push(manufacturer.clone());
                }
                if !title.is_empty() {
                    parts.push(title.clone());
                }
                if !package.is_empty() {
                    parts.push(package.clone());
                }
                parts.join(" ")
            }
        };

        log::debug!(
            "Extracted metadata: manufacturer={}, datasheet={}, jlc_id={}, description={}",
            manufacturer,
            datasheet,
            jlc_id,
            description
        );

        let (package_detail, package_bbox_x, package_bbox_y, model_3d) = if let Some(pkg) =
            result.package_detail
        {
            let pkg_bbox_x = pkg
                .get("dataStr")
                .and_then(|ds| ds.get("head"))
                .and_then(|h| h.get("x"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let pkg_bbox_y = pkg
                .get("dataStr")
                .and_then(|ds| ds.get("head"))
                .and_then(|h| h.get("y"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            log::debug!("Extracted package bbox: x={}, y={}", pkg_bbox_x, pkg_bbox_y);

            let shapes = if let Some(pkg_data_str) = pkg.get("dataStr") {
                if let Some(shape_array) = pkg_data_str.get("shape").and_then(|v| v.as_array()) {
                    shape_array
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                } else {
                    vec![]
                }
            } else if pkg.is_array() {
                pkg.as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            } else {
                vec![]
            };

            let model_3d = Self::extract_3d_model_from_svgnode(&shapes);
            (shapes, pkg_bbox_x, pkg_bbox_y, model_3d)
        } else {
            (vec![], 0.0, 0.0, None)
        };

        Ok(ComponentData {
            lcsc_id: lcsc_id.to_string(),
            title,
            designator,
            description,
            data_str,
            bbox_x,
            bbox_y,
            symbol_parts,
            package_detail,
            package_bbox_x,
            package_bbox_y,
            model_3d,
            manufacturer,
            datasheet,
            jlc_id,
            package,
        })
    }

    fn extract_3d_model_from_svgnode(shapes: &[String]) -> Option<Model3dInfo> {
        for shape in shapes {
            if shape.starts_with("SVGNODE~") {
                let parts: Vec<&str> = shape.split('~').collect();
                if parts.len() > 1
                    && let Ok(svg_data) = serde_json::from_str::<serde_json::Value>(parts[1])
                    && let Some(attrs) = svg_data.get("attrs")
                    && attrs.get("c_etype").and_then(|v| v.as_str()) == Some("outline3D")
                {
                    let uuid = attrs
                        .get("uuid")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let title = attrs
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    // The model is drawn in the footprint only after applying `c_rotation`;
                    // dropping it leaves every footprint whose model is rotated at 90
                    // degrees with the 3D body across the pads instead of on them.
                    let rotation_z = attrs
                        .get("c_rotation")
                        .and_then(|v| v.as_str())
                        .and_then(|text| text.split(',').nth(2))
                        .and_then(|value| value.trim().parse::<f64>().ok())
                        .unwrap_or(0.0);

                    if let (Some(uuid), Some(title)) = (uuid, title) {
                        return Some(Model3dInfo {
                            uuid,
                            title,
                            rotation_z,
                            placement: model_placement(attrs),
                        });
                    }
                }
            }
        }
        None
    }

    pub async fn download_3d_step(&self, uuid: &str) -> Result<Arc<Vec<u8>>> {
        let url = format!("{MODEL_DOWNLOAD_BASE_URL}/{uuid}");
        let mirror = format!("{MODEL_DOWNLOAD_MIRROR_URL}/{uuid}");
        let cell = {
            let mut cache = self.step_cache.lock().await;
            cache
                .entry(uuid.to_string())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let result = cell
            .get_or_init(|| async {
                let bytes = match self
                    .download_with_retry(&url, "STEP", uuid, ModelSink::Memory)
                    .await
                {
                    Ok(bytes) => Ok(bytes),
                    Err(primary) => {
                        log::warn!(
                            "STEP download from lceda.cn failed ({primary}); trying easyeda.com"
                        );
                        self.download_with_retry(&mirror, "STEP", uuid, ModelSink::Memory)
                            .await
                            .map_err(|mirror_err| format!("{primary}; mirror: {mirror_err}"))
                    }
                };
                bytes.map(|bytes| Arc::new(bytes.expect("a memory download returns its bytes")))
            })
            .await
            .clone();

        match result {
            // Handing back the cached buffer rather than a copy of it: a model is a
            // megabyte or so, and a batch has one in flight per parallel part.
            Ok(bytes) => {
                self.trim_step_cache(uuid).await;
                Ok(bytes)
            }
            Err(message) => {
                self.step_cache.lock().await.remove(uuid);
                Err(EasyedaError::InvalidData(message).into())
            }
        }
    }

    /// Downloads a model straight into `path`, for a batch that would otherwise
    /// hold one model in memory per parallel part while they are in flight.
    pub async fn download_3d_step_to_file(&self, uuid: &str, path: &Path) -> Result<()> {
        let url = format!("{MODEL_DOWNLOAD_BASE_URL}/{uuid}");
        if let Err(primary) = self
            .download_with_retry(&url, "STEP", uuid, ModelSink::File(path))
            .await
        {
            log::warn!("STEP download from lceda.cn failed ({primary}); trying easyeda.com");
            let mirror = format!("{MODEL_DOWNLOAD_MIRROR_URL}/{uuid}");
            self.download_with_retry(&mirror, "STEP", uuid, ModelSink::File(path))
                .await?;
        }
        Ok(())
    }

    /// Keeps the cache to [`CACHED_MODELS`] finished downloads, oldest first, so
    /// a long batch does not hold every model it has seen. `keep` is the one that
    /// was just used, and anything still downloading is left alone.
    async fn trim_step_cache(&self, keep: &str) {
        let mut cache = self.step_cache.lock().await;
        if cache.len() <= CACHED_MODELS {
            return;
        }
        let stale: Vec<String> = cache
            .iter()
            .filter(|(uuid, cell)| {
                uuid.as_str() != keep && cell.get().is_some_and(|done| done.is_ok())
            })
            .map(|(uuid, _)| uuid.clone())
            .take(cache.len() - CACHED_MODELS)
            .collect();
        for uuid in stale {
            cache.remove(&uuid);
        }
    }

    async fn download_with_retry(
        &self,
        url: &str,
        model_type: &str,
        uuid: &str,
        sink: ModelSink<'_>,
    ) -> Result<Option<Vec<u8>>> {
        for attempt in 1..=STEP_DOWNLOAD_ATTEMPTS {
            log::info!(
                "Downloading 3D {} model: {}{}",
                model_type,
                uuid,
                if attempt > 1 {
                    format!(" (retry {}/{})", attempt, STEP_DOWNLOAD_ATTEMPTS)
                } else {
                    String::new()
                }
            );

            let result = tokio::time::timeout(
                Duration::from_secs(STEP_DOWNLOAD_TOTAL_TIMEOUT_SECS),
                self.download_once(url, model_type, uuid, sink),
            )
            .await;

            match result {
                Ok(Ok(bytes)) => return Ok(bytes),
                Ok(Err(error)) => {
                    if attempt == STEP_DOWNLOAD_ATTEMPTS {
                        return Err(error);
                    }
                    log::warn!("Failed to download {} model, retrying...", model_type);
                    Self::backoff_delay(attempt).await;
                }
                Err(_) => {
                    if attempt == STEP_DOWNLOAD_ATTEMPTS {
                        return Err(EasyedaError::InvalidData(format!(
                            "{} download timed out after {}s: {}",
                            model_type, STEP_DOWNLOAD_TOTAL_TIMEOUT_SECS, uuid
                        ))
                        .into());
                    }
                    log::warn!("{} download timed out, retrying...", model_type);
                    Self::backoff_delay(attempt).await;
                }
            }
        }
        unreachable!()
    }

    async fn download_once(
        &self,
        url: &str,
        model_type: &str,
        uuid: &str,
        sink: ModelSink<'_>,
    ) -> Result<Option<Vec<u8>>> {
        wait_for_request_slot(
            &self.model_gate,
            Duration::from_millis(MODEL_REQUEST_INTERVAL_MS),
        )
        .await;

        // Models are STEP text, which compresses several times over, so the client is
        // left to ask for a compressed body (`gzip` is enabled for reqwest and is
        // decoded transparently).
        let response = self
            .model_client
            .get(url)
            .send()
            .await
            .map_err(EasyedaError::ApiRequest)?;

        let status = response.status();
        if !status.is_success() {
            let message = response
                .text()
                .await
                .unwrap_or_default()
                .trim()
                .chars()
                .take(200)
                .collect::<String>();
            let message = summarize_http_error_body(message);
            if is_upstream_block_status(status, &message) {
                extend_request_cooldown(
                    &self.model_gate,
                    Duration::from_millis(UPSTREAM_BLOCK_COOLDOWN_MS),
                )
                .await;
            }
            return Err(EasyedaError::HttpStatus {
                lcsc_id: uuid.to_string(),
                status,
                message: format!("Failed to download {model_type}: {message}"),
            }
            .into());
        }

        match sink {
            ModelSink::Memory => read_response_body(response, STEP_DOWNLOAD_STALL_TIMEOUT_SECS)
                .await
                .map(Some)
                .map_err(EasyedaError::InvalidData)
                .map_err(Into::into),
            ModelSink::File(path) => {
                let mut writer = BufWriter::new(File::create(path)?);
                match read_response_body_into(
                    response,
                    STEP_DOWNLOAD_STALL_TIMEOUT_SECS,
                    &mut writer,
                )
                .await
                {
                    Ok(()) => {
                        writer.flush()?;
                        Ok(None)
                    }
                    Err(message) => {
                        // A half written model must not be mistaken for a model.
                        drop(writer);
                        let _ = fs::remove_file(path);
                        Err(EasyedaError::InvalidData(message).into())
                    }
                }
            }
        }
    }

    async fn backoff_delay(attempt: u32) {
        tokio::time::sleep(retry_delay_for_attempt(attempt)).await;
    }
}

fn symbol_bbox_x(data_str_obj: &serde_json::Value) -> f64 {
    data_str_obj
        .get("head")
        .and_then(|h| h.get("x"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}

fn symbol_bbox_y(data_str_obj: &serde_json::Value) -> f64 {
    data_str_obj
        .get("head")
        .and_then(|h| h.get("y"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}

fn extract_shape_strings(data_str_obj: &serde_json::Value) -> Vec<String> {
    data_str_obj
        .get("shape")
        .and_then(|v| v.as_array())
        .map(|shape_array| {
            shape_array
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

async fn wait_for_request_slot(gate: &Mutex<RequestGate>, interval: Duration) {
    loop {
        let wait = {
            let mut gate = gate.lock().await;
            let now = Instant::now();
            if gate.next_ready <= now {
                gate.next_ready = now + interval;
                None
            } else {
                Some(gate.next_ready.saturating_duration_since(now))
            }
        };

        let Some(wait) = wait else {
            return;
        };
        tokio::time::sleep(wait).await;
    }
}

async fn extend_request_cooldown(gate: &Mutex<RequestGate>, cooldown: Duration) {
    let mut gate = gate.lock().await;
    let until = Instant::now() + cooldown;
    if gate.next_ready < until {
        gate.next_ready = until;
    }
}

impl Default for EasyedaApi {
    fn default() -> Self {
        Self::new()
    }
}

fn should_retry_app_error(error: &AppError) -> bool {
    match error {
        AppError::Easyeda(EasyedaError::ApiRequest(error)) => {
            error.is_timeout()
                || error.is_connect()
                || error.status().is_some_and(is_retryable_status)
        }
        AppError::Easyeda(EasyedaError::HttpStatus {
            status, message, ..
        }) => is_upstream_block_status(*status, message) || is_retryable_status(*status),
        AppError::Easyeda(EasyedaError::ApiResponse { message, .. }) => {
            is_retryable_api_message(message)
        }
        _ => false,
    }
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn is_upstream_block_status(status: StatusCode, message: &str) -> bool {
    status == StatusCode::FORBIDDEN && message.contains("HTML error page")
}

fn is_retryable_api_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "busy",
        "rate",
        "timeout",
        "temporar",
        "try again",
        "too many",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn retry_delay_for_attempt(attempt: u32) -> Duration {
    Duration::from_millis(RETRY_BASE_DELAY_MS.saturating_mul(1u64 << attempt.min(8)))
}

fn retry_delay_for_error(error: &AppError, attempt: u32) -> Duration {
    match error {
        AppError::Easyeda(EasyedaError::HttpStatus {
            status, message, ..
        }) if is_upstream_block_status(*status, message) => {
            Duration::from_millis(UPSTREAM_BLOCK_RETRY_DELAY_MS.saturating_mul(attempt as u64 + 1))
        }
        _ => retry_delay_for_attempt(attempt),
    }
}

async fn read_response_body(
    response: reqwest::Response,
    stall_timeout_secs: u64,
) -> std::result::Result<Vec<u8>, String> {
    // A model is a megabyte or so and the length is known up front, so reserving
    // it once keeps the buffer from being reallocated and copied as it grows.
    let expected = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0);
    let mut body = Vec::with_capacity(expected);
    read_response_body_into(response, stall_timeout_secs, &mut body).await?;
    Ok(body)
}

async fn read_response_body_into<W: Write>(
    response: reqwest::Response,
    stall_timeout_secs: u64,
    sink: &mut W,
) -> std::result::Result<(), String> {
    let mut stream = response.bytes_stream();
    let stall_timeout = Duration::from_secs(stall_timeout_secs);

    loop {
        match tokio::time::timeout(stall_timeout, stream.next()).await {
            Ok(Some(Ok(chunk))) => sink.write_all(&chunk).map_err(|error| error.to_string())?,
            Ok(Some(Err(error))) => return Err(error.to_string()),
            Ok(None) => return Ok(()),
            Err(_) => {
                return Err(format!(
                    "STEP download stalled for more than {}s",
                    stall_timeout_secs
                ));
            }
        }
    }
}

fn api_response_message(response: &ApiResponse) -> String {
    if let Some(message) = response.message.as_deref().map(str::trim)
        && !message.is_empty()
    {
        return message.to_string();
    }

    if let Some(code) = &response.code {
        return format!("code={code}");
    }

    "success=false without message".to_string()
}

fn summarize_http_error_body(message: String) -> String {
    let message = message.trim();
    if message.is_empty() {
        "empty response body".to_string()
    } else if looks_like_html(message) {
        "upstream returned an HTML error page".to_string()
    } else {
        message.chars().take(120).collect()
    }
}

fn looks_like_html(message: &str) -> bool {
    let lower = message
        .chars()
        .take(80)
        .collect::<String>()
        .to_ascii_lowercase();
    lower.contains("<!doctype html") || lower.contains("<html") || lower.contains("<head")
}

fn is_not_found_api_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("not found")
        || message.contains("not exist")
        || message.contains("no data")
        || message.contains("component does not")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_3d_model_rotation_from_the_footprint_outline() {
        // Real shape row of an LCSC SOP-8 footprint (trimmed): the model is stored rotated by 90
        // degrees and has to keep that rotation, or the exported 3D body lands across the pads.
        let shapes = vec![
            r#"SVGNODE~{"attrs":{"uuid":"765d86d5","title":"SOP-8_L5.0-W4.0-H1.8-LS6.2-P1.27","c_etype":"outline3D","c_origin":"4001,2998","c_width":"24.4094","c_height":"19.685","c_rotation":"0,0,90"}}"#
                .to_string(),
        ];

        let model = EasyedaApi::extract_3d_model_from_svgnode(&shapes).expect("model metadata");

        assert_eq!(model.uuid, "765d86d5");
        assert_eq!(model.title, "SOP-8_L5.0-W4.0-H1.8-LS6.2-P1.27");
        assert_eq!(model.rotation_z, 90.0);
    }

    #[test]
    fn defaults_to_no_rotation_without_outline_data() {
        let shapes = vec![
            r#"SVGNODE~{"attrs":{"uuid":"abc","title":"BODY","c_etype":"outline3D"}}"#.to_string(),
        ];

        let model = EasyedaApi::extract_3d_model_from_svgnode(&shapes).expect("model metadata");

        assert_eq!(model.rotation_z, 0.0);
    }

    #[test]
    fn recognizes_html_forbidden_as_upstream_block() {
        let message =
            summarize_http_error_body("<html><head><title>Forbidden</title></head>".to_string());

        assert_eq!(message, "upstream returned an HTML error page");
        assert!(is_upstream_block_status(StatusCode::FORBIDDEN, &message));
        assert!(!is_upstream_block_status(StatusCode::NOT_FOUND, &message));
    }

    #[test]
    fn upstream_block_retry_delay_is_slow_path() {
        let error = AppError::Easyeda(EasyedaError::HttpStatus {
            lcsc_id: "C1".to_string(),
            status: StatusCode::FORBIDDEN,
            message: "upstream returned an HTML error page".to_string(),
        });

        assert_eq!(
            retry_delay_for_error(&error, 1),
            Duration::from_millis(UPSTREAM_BLOCK_RETRY_DELAY_MS * 2)
        );
    }
}

/// `c_origin` / `z` / `c_rotation` of an EasyEDA `outline3D` node.
fn model_placement(attrs: &serde_json::Value) -> Option<crate::easyeda::models::ModelPlacement> {
    let numbers = |key: &str| -> Vec<f64> {
        let text = match attrs.get(key) {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(serde_json::Value::Number(number)) => number.to_string(),
            _ => String::new(),
        };
        text.split(',')
            .filter_map(|part| part.trim().parse::<f64>().ok())
            .collect()
    };
    let origin = numbers("c_origin");
    if origin.len() < 2 {
        return None;
    }
    let rotation = numbers("c_rotation");
    Some(crate::easyeda::models::ModelPlacement {
        origin_x: origin[0],
        origin_y: origin[1],
        z: numbers("z").first().copied().unwrap_or(0.0),
        rotation: [
            rotation.first().copied().unwrap_or(0.0),
            rotation.get(1).copied().unwrap_or(0.0),
            rotation.get(2).copied().unwrap_or(0.0),
        ],
    })
}
