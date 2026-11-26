//! Local static site publishing, Astro static-only checks, 5-tier path security validator,
//! and atomic deployment registry (`StaticSiteService`, `OB-F12`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chrono::{Duration, SecondsFormat, Utc};
use openbot_storage::write_json_file_atomically;
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{OpenBotRuntimeError, RuntimeResult};

pub const MAX_FILES: usize = 20;
pub const MAX_FILE_BYTES: u64 = 1_048_576;
pub const MAX_TOTAL_BYTES: u64 = 2_097_152;
pub const MAX_PATH_LEN: usize = 240;
pub const EXPIRY_DAYS: i64 = 30;

const UNSAFE_SEGMENTS: [&str; 5] = [".env", ".git", "node_modules", "server", "api"];
const ALLOWED_MIME_TYPES: [(&str, &str); 14] = [
    ("html", "text/html"),
    ("css", "text/css"),
    ("js", "text/javascript"),
    ("mjs", "text/javascript"),
    ("json", "application/json"),
    ("svg", "image/svg+xml"),
    ("webp", "image/webp"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("ico", "image/x-icon"),
    ("woff2", "font/woff2"),
    ("txt", "text/plain"),
    ("webmanifest", "application/manifest+json"),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticSitePublishRequest {
    pub slug: String,
    pub source_dir: PathBuf,
    pub title: Option<String>,
    pub description: Option<String>,
    pub replace: bool,
    pub spa_fallback: bool,
    pub allowed_roots: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticSiteRecord {
    pub site_id: String,
    pub slug: String,
    pub hostname: String,
    pub url: String,
    pub title: String,
    pub description: String,
    pub framework: String,
    pub status: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub spa_fallback: bool,
    pub expires_in_days: i64,
    pub expires_at: String,
    pub updated_at: String,
    pub replaced: bool,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticSiteDeleteRequest {
    pub slug: Option<String>,
    pub site_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticSiteDeleteOutcome {
    pub site_id: String,
    pub slug: String,
    pub status: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SiteRegistryFile {
    version: u32,
    sites: BTreeMap<String, StaticSiteRecord>,
}

struct PreparedFile {
    rel_path: String,
    bytes: Vec<u8>,
}

pub struct StaticSiteService {
    sites_root: PathBuf,
    deployments_dir: PathBuf,
    registry_path: PathBuf,
    registry: Mutex<SiteRegistryFile>,
}

impl StaticSiteService {
    pub fn open(data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let sites_root = data_dir.as_ref().join("hosted-sites");
        let deployments_dir = sites_root.join("deployments");
        let registry_path = sites_root.join("sites.json");
        let loaded = load_site_registry(&registry_path)?;
        Ok(Arc::new(Self {
            sites_root,
            deployments_dir,
            registry_path,
            registry: Mutex::new(loaded),
        }))
    }

    pub fn validate_relative_path(path: &str) -> RuntimeResult<String> {
        let replaced = path.replace('\\', "/");
        let normalized = replaced.strip_prefix("./").unwrap_or(&replaced);
        if normalized.is_empty()
            || normalized.len() > MAX_PATH_LEN
            || normalized.starts_with('/')
            || normalized.ends_with('/')
            || normalized.contains("//")
            || normalized.contains('\0')
        {
            return Err(OpenBotRuntimeError::Validation(format!(
                "error.site.unsafePath: Invalid relative path '{path}'."
            )));
        }

        let segments: Vec<&str> = normalized.split('/').collect();
        for seg in &segments {
            if *seg == ".." || *seg == "." {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "error.site.unsafePath: Path traversal via parent or relative segment '{seg}' is not allowed in '{path}'."
                )));
            }
        }
        for seg in &segments {
            let lower = seg.to_ascii_lowercase();
            if UNSAFE_SEGMENTS.contains(&lower.as_str()) {
                if seg.starts_with('.') {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "error.site.hiddenFile: Blocked unsafe hidden dotfile segment '{seg}' in '{normalized}'."
                    )));
                }
                return Err(OpenBotRuntimeError::Validation(format!(
                    "error.site.unsafePath: Blocked unsafe directory segment '{seg}' in '{normalized}'."
                )));
            }
            if seg.starts_with('.') {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "error.site.hiddenFile: Hidden dotfile segment '{seg}' is disallowed in '{normalized}'."
                )));
            }
        }

        let file_name = segments.last().copied().unwrap_or("");
        if secret_file_regex().is_match(file_name) || server_source_regex().is_match(file_name) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "error.site.secretFile: Blocked sensitive/secret or server source file '{normalized}'."
            )));
        }
        if archive_ext_regex().is_match(normalized) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "error.site.fileType: Blocked archive or sourcemap file '{normalized}'."
            )));
        }

        let ext = file_name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        if mime_for_ext(&ext).is_none() {
            return Err(OpenBotRuntimeError::Validation(format!(
                "error.site.fileType: Disallowed file type '.{ext}' for '{normalized}'."
            )));
        }

        Ok(normalized.to_string())
    }

    pub fn publish_site(&self, req: StaticSitePublishRequest) -> RuntimeResult<StaticSiteRecord> {
        let slug = req.slug.trim();
        if !slug_regex().is_match(slug) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Invalid static site slug '{slug}'; must match ^[a-z0-9][a-z0-9-]{{0,62}}$."
            )));
        }

        let meta = fs::symlink_metadata(&req.source_dir).map_err(|err| {
            OpenBotRuntimeError::Validation(format!(
                "error.site.notDirectory: Cannot read source directory '{}': {err}",
                req.source_dir.display()
            ))
        })?;
        if meta.file_type().is_symlink() {
            return Err(OpenBotRuntimeError::Validation(
                "error.site.rootSymlink: Source directory must not be a symlink.".to_string(),
            ));
        }
        if !meta.is_dir() {
            return Err(OpenBotRuntimeError::Validation(
                "error.site.notDirectory: Source path must be a directory.".to_string(),
            ));
        }

        let canonical_root = fs::canonicalize(&req.source_dir)?;
        if let Some(roots) = req.allowed_roots.as_ref()
            && !roots.is_empty()
        {
            let mut allowed_match = false;
            for candidate in roots {
                if let Ok(canon_allowed) = fs::canonicalize(candidate)
                    && canonical_root.starts_with(&canon_allowed)
                {
                    allowed_match = true;
                    break;
                }
            }
            if !allowed_match {
                return Err(OpenBotRuntimeError::Forbidden(
                    "error.site.outsideWorkspace: Source directory is outside allowed workspace roots."
                        .to_string(),
                ));
            }
        }

        let framework = detect_framework(&canonical_root)?;
        let output_dir = if framework == "astro" {
            validate_static_astro_output(&canonical_root)?
        } else {
            canonical_root.clone()
        };

        let mut files = Vec::new();
        let mut total_bytes: u64 = 0;
        collect_site_files(&output_dir, &output_dir, &mut files, &mut total_bytes)?;

        if !files.iter().any(|f| f.rel_path == "index.html") {
            return Err(OpenBotRuntimeError::Validation(
                "error.site.missingIndex: Static site must include a root index.html.".to_string(),
            ));
        }

        let mut guard = self.registry.lock();
        let existing_active = guard
            .sites
            .values()
            .find(|rec| rec.slug == slug && !rec.deleted && rec.status == "active")
            .cloned();

        let (site_id, hostname, url, replaced) = match existing_active {
            Some(prev) if !req.replace => {
                return Err(OpenBotRuntimeError::Conflict(format!(
                    "Static site slug '{slug}' already exists (siteId '{}'); set replace=true to replace it.",
                    prev.site_id
                )));
            }
            Some(prev) => (prev.site_id, prev.hostname, prev.url, true),
            None => {
                let id = format!("site-{}", Uuid::new_v4().simple());
                let host = format!("{slug}.openbot.site");
                let site_url = format!("https://{host}/");
                (id, host, site_url, false)
            }
        };

        self.deploy_files_atomically(&site_id, &files)?;

        let now = Utc::now();
        let expires_at = (now + Duration::days(EXPIRY_DAYS))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let updated_at = now.to_rfc3339_opts(SecondsFormat::Millis, true);
        let title = req
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(slug)
            .to_string();
        let description = req
            .description
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .to_string();

        let record = StaticSiteRecord {
            site_id: site_id.clone(),
            slug: slug.to_string(),
            hostname,
            url,
            title,
            description,
            framework: framework.to_string(),
            status: "active".to_string(),
            file_count: files.len(),
            total_bytes,
            spa_fallback: req.spa_fallback,
            expires_in_days: EXPIRY_DAYS,
            expires_at,
            updated_at,
            replaced,
            deleted: false,
        };

        guard.version = 1;
        guard.sites.insert(site_id, record.clone());
        write_json_file_atomically(&self.registry_path, &*guard, true)?;
        Ok(record)
    }

    pub fn delete_site(
        &self,
        req: StaticSiteDeleteRequest,
    ) -> RuntimeResult<StaticSiteDeleteOutcome> {
        let wanted_id = req
            .site_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let wanted_slug = req
            .slug
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if wanted_id.is_none() && wanted_slug.is_none() {
            return Err(OpenBotRuntimeError::Validation(
                "delete_site requires either siteId or slug.".to_string(),
            ));
        }

        let mut guard = self.registry.lock();
        let found_key = guard
            .sites
            .iter()
            .find(|(_, rec)| {
                !rec.deleted
                    && rec.status == "active"
                    && wanted_id.is_none_or(|id| rec.site_id == id)
                    && wanted_slug.is_none_or(|sl| rec.slug == sl)
            })
            .map(|(k, _)| k.clone())
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(
                    "error.team.hostedSiteNotFound: Hosted static site not found.".to_string(),
                )
            })?;

        let deploy_target = self.deployments_dir.join(&found_key);
        if deploy_target.exists() {
            let _ = fs::remove_dir_all(&deploy_target);
        }

        let now_iso = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let rec = guard.sites.get_mut(&found_key).expect("key found above");
        rec.status = "deleted".to_string();
        rec.deleted = true;
        rec.updated_at = now_iso;
        let outcome = StaticSiteDeleteOutcome {
            site_id: rec.site_id.clone(),
            slug: rec.slug.clone(),
            status: "deleted".to_string(),
            deleted: true,
        };
        guard.version = 1;
        write_json_file_atomically(&self.registry_path, &*guard, true)?;
        Ok(outcome)
    }

    pub fn list_sites(&self) -> RuntimeResult<Vec<StaticSiteRecord>> {
        let guard = self.registry.lock();
        let mut records: Vec<StaticSiteRecord> = guard.sites.values().cloned().collect();
        records.sort_by(|a, b| a.slug.cmp(&b.slug).then_with(|| a.site_id.cmp(&b.site_id)));
        Ok(records)
    }

    fn deploy_files_atomically(&self, site_id: &str, files: &[PreparedFile]) -> RuntimeResult<()> {
        create_private_dir(&self.sites_root)?;
        create_private_dir(&self.deployments_dir)?;

        let stage_dir = self
            .deployments_dir
            .join(format!("{site_id}.stage-{}", Uuid::new_v4().simple()));
        create_private_dir(&stage_dir)?;

        let write_result = (|| -> RuntimeResult<()> {
            for file in files {
                let dest = stage_dir.join(&file.rel_path);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&dest, &file.bytes)?;
            }
            Ok(())
        })();

        if let Err(err) = write_result {
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(err);
        }

        let target_dir = self.deployments_dir.join(site_id);
        let backup_dir = self
            .deployments_dir
            .join(format!("{site_id}.old-{}", Uuid::new_v4().simple()));
        let had_existing = target_dir.exists();
        if had_existing {
            fs::rename(&target_dir, &backup_dir)?;
        }
        if let Err(err) = fs::rename(&stage_dir, &target_dir) {
            if had_existing {
                let _ = fs::rename(&backup_dir, &target_dir);
            }
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(OpenBotRuntimeError::Io(err));
        }
        if had_existing {
            let _ = fs::remove_dir_all(&backup_dir);
        }
        Ok(())
    }
}

fn detect_framework(root: &Path) -> RuntimeResult<&'static str> {
    let pkg_path = root.join("package.json");
    if pkg_path.exists() {
        let raw = fs::read_to_string(&pkg_path)?;
        let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|_| {
            OpenBotRuntimeError::Validation(
                "error.site.packageJsonInvalid: package.json is not valid JSON.".to_string(),
            )
        })?;
        if let Some(obj) = parsed.as_object() {
            for group_key in ["dependencies", "devDependencies"] {
                if obj
                    .get(group_key)
                    .and_then(serde_json::Value::as_object)
                    .and_then(|deps| deps.get("astro"))
                    .is_some_and(serde_json::Value::is_string)
                {
                    return Ok("astro");
                }
            }
        }
    }
    for cfg in ["astro.config.mjs", "astro.config.js", "astro.config.ts"] {
        if fs::symlink_metadata(root.join(cfg)).is_ok() {
            return Ok("astro");
        }
    }
    Ok("vanilla")
}

fn validate_static_astro_output(root: &Path) -> RuntimeResult<PathBuf> {
    for cfg_name in ["astro.config.mjs", "astro.config.js", "astro.config.ts"] {
        let cfg_path = root.join(cfg_name);
        if cfg_path.is_file() {
            let content = fs::read_to_string(&cfg_path)?;
            if astro_server_output_regex().is_match(&content) {
                return Err(OpenBotRuntimeError::Validation(
                    "error.site.astroServerOutput: Astro sites must be static-only; output 'server' or 'hybrid' is not supported."
                        .to_string(),
                ));
            }
            if astro_adapter_regex().is_match(&content) {
                return Err(OpenBotRuntimeError::Validation(
                    "error.site.astroAdapter: Astro server adapters and React SSR integrations are not supported for static sites."
                        .to_string(),
                ));
            }
            break;
        }
    }

    for forbidden in [root.join("src").join("pages").join("api"), root.join("src").join("actions")] {
        if fs::symlink_metadata(&forbidden).is_ok() {
            return Err(OpenBotRuntimeError::Validation(
                "error.site.astroApiRoutes: Astro API routes and actions are not supported in static sites."
                    .to_string(),
            ));
        }
    }

    let src_dir = root.join("src");
    if src_dir.is_dir() && has_astro_server_or_middleware(&src_dir, &src_dir)? {
        return Err(OpenBotRuntimeError::Validation(
            "error.site.astroMiddleware: Astro middleware or *.server.* files are not supported in static sites."
                .to_string(),
        ));
    }

    let dist_path = root.join("dist");
    let dist_meta = fs::symlink_metadata(&dist_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            OpenBotRuntimeError::Validation(
                "error.site.astroNotBuilt: Astro site must be built into dist/ before publishing."
                    .to_string(),
            )
        } else {
            OpenBotRuntimeError::Io(err)
        }
    })?;
    if dist_meta.file_type().is_symlink() || !dist_meta.is_dir() {
        return Err(OpenBotRuntimeError::Validation(
            "error.site.astroDistNotDirectory: Astro dist/ must be a real directory.".to_string(),
        ));
    }
    let canonical_dist = fs::canonicalize(&dist_path)?;
    if !canonical_dist.starts_with(root) {
        return Err(OpenBotRuntimeError::Validation(
            "error.site.astroDistOutside: Astro dist/ resolves outside the site root.".to_string(),
        ));
    }
    Ok(canonical_dist)
}

fn has_astro_server_or_middleware(src_root: &Path, dir: &Path) -> RuntimeResult<bool> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let abs = entry.path();
        let meta = fs::symlink_metadata(&abs)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if has_astro_server_or_middleware(src_root, &abs)? {
                return Ok(true);
            }
        } else if let Ok(rel) = abs.strip_prefix(src_root) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if astro_middleware_regex().is_match(&rel_str) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn collect_site_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PreparedFile>,
    total_bytes: &mut u64,
) -> RuntimeResult<()> {
    let canonical_dir = fs::canonicalize(dir)?;
    if !canonical_dir.starts_with(root) {
        return Err(OpenBotRuntimeError::Validation(
            "error.site.directoryOutsideRoot: Directory resolves outside site root.".to_string(),
        ));
    }

    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name().to_string_lossy().to_string());

    for entry in entries {
        let abs = entry.path();
        let meta = fs::symlink_metadata(&abs)?;
        let name = entry.file_name().to_string_lossy().to_string();
        if meta.file_type().is_symlink() {
            return Err(OpenBotRuntimeError::Validation(format!(
                "error.site.symlink: Symlink '{name}' is not allowed in static sites."
            )));
        }
        if meta.is_dir() {
            let lower = name.to_ascii_lowercase();
            if UNSAFE_SEGMENTS.contains(&lower.as_str()) {
                if name.starts_with('.') {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "error.site.hiddenFile: Blocked unsafe hidden directory '{name}'."
                    )));
                }
                return Err(OpenBotRuntimeError::Validation(format!(
                    "error.site.unsafePath: Blocked unsafe directory '{name}'."
                )));
            }
            if name.starts_with('.') {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "error.site.hiddenFile: Blocked hidden directory '{name}'."
                )));
            }
            collect_site_files(root, &abs, out, total_bytes)?;
            continue;
        }
        if !meta.is_file() {
            return Err(OpenBotRuntimeError::Validation(format!(
                "error.site.unsupportedEntry: Unsupported filesystem entry '{name}'."
            )));
        }
        if out.len() >= MAX_FILES {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "error.site.tooManyFiles: Static site exceeds maximum limit of {MAX_FILES} files."
            )));
        }
        let rel = abs
            .strip_prefix(root)
            .map_err(|_| {
                OpenBotRuntimeError::Validation(format!(
                    "error.site.fileOutsideRoot: File '{name}' is outside site root."
                ))
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let validated_path = StaticSiteService::validate_relative_path(&rel)?;

        let canonical_file = fs::canonicalize(&abs)?;
        if !canonical_file.starts_with(root) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "error.site.fileOutsideRoot: File '{validated_path}' resolves outside site root."
            )));
        }

        let size = meta.len();
        if size > MAX_FILE_BYTES {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "error.site.fileTooLarge: File '{validated_path}' ({size} bytes) exceeds maximum limit of {MAX_FILE_BYTES} bytes (1 MiB)."
            )));
        }
        let bytes = fs::read(&abs)?;
        let actual_size = bytes.len() as u64;
        if actual_size > MAX_FILE_BYTES {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "error.site.fileTooLarge: File '{validated_path}' ({actual_size} bytes) exceeds maximum limit of {MAX_FILE_BYTES} bytes (1 MiB)."
            )));
        }
        *total_bytes = total_bytes.saturating_add(actual_size);
        if *total_bytes > MAX_TOTAL_BYTES {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "error.site.siteTooLarge: Total site size ({} bytes) exceeds maximum limit of {MAX_TOTAL_BYTES} bytes (2 MiB).",
                *total_bytes
            )));
        }
        out.push(PreparedFile {
            rel_path: validated_path,
            bytes,
        });
    }
    Ok(())
}

fn load_site_registry(path: &Path) -> RuntimeResult<SiteRegistryFile> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            if raw.trim().is_empty() {
                return Ok(SiteRegistryFile {
                    version: 1,
                    sites: BTreeMap::new(),
                });
            }
            let parsed: SiteRegistryFile = serde_json::from_str(&raw)?;
            Ok(parsed)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SiteRegistryFile {
            version: 1,
            sites: BTreeMap::new(),
        }),
        Err(err) => Err(OpenBotRuntimeError::Io(err)),
    }
}

fn create_private_dir(dir: &Path) -> RuntimeResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let existed = dir.exists();
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir)?;
        if !existed {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn mime_for_ext(ext: &str) -> Option<&'static str> {
    ALLOWED_MIME_TYPES
        .iter()
        .find(|(k, _)| *k == ext)
        .map(|(_, mime)| *mime)
}

fn slug_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9-]{0,62}$").expect("valid slug regex"))
}

fn secret_file_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[-_.])(?:credentials?|private[-_]?key|secret|service[-_]?account)(?:[-_.]|$)")
            .expect("valid secret filename regex")
    })
}

fn server_source_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(?:server|worker)\.[cm]?[jt]s$").expect("valid server source regex"))
}

fn archive_ext_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\.(?:map|zip|tar|gz)$").expect("valid archive ext regex"))
}

fn astro_server_output_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"output\s*:\s*["'](?:server|hybrid)["']"#).expect("valid astro output regex")
    })
}

fn astro_adapter_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?is)adapter|@astrojs/react|integrations\s*:\s*\[[^\]]*react")
            .expect("valid astro adapter regex")
    })
}

fn astro_middleware_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:^|/)(?:middleware|[^/]+\.server)\.[cm]?[jt]s$")
            .expect("valid astro middleware regex")
    })
}
