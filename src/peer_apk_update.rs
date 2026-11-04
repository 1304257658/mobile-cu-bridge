//! Check and cover-install the Android peer APK.
//!
//! Discovery matches the desktop updater: a GitHub Release tag URL, then the
//! signed `rustdesk-update.json` sidecar. The APK itself is HTTPS plus the
//! sidecar sha256. A bad hash, a bad signature, or a size overrun deletes the
//! file and does not install.
//!
//! The install is a same-certificate replacement. There is no uninstall step,
//! so the on-disk `enc_id` and device id stay. A different certificate is
//! `signature`. A device id or `enc_id` that does not match afterwards is
//! `id_changed`.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use crate::{
    hbbs_http::create_http_client_with_url_strict,
    semantic_action::{
        self, UPDATE_CHECKSUM, UPDATE_ID_CHANGED, UPDATE_INSTALL, UPDATE_NETWORK, UPDATE_SIGNATURE,
    },
    update_hash::verify_sha256_reader,
};
use hbb_common::{get_version_number, log};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

const APK_MAX_BYTES: u64 = 96 * 1024 * 1024;
const METADATA_MAX_BYTES: u64 = 1024 * 1024;
const VERSION_BODY_MAX_BYTES: u64 = 64 * 1024;
const ID_FILE_MAX_BYTES: u64 = 256 * 1024;
const ENC_ID_MAX: usize = 4096;
const DEVICE_ID_MAX: usize = 64;
const CHUNK: usize = 8192;

const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const SIDECAR_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(100);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(40);

/// Download, verify, and install must all finish inside this window.
const INNER_BUDGET: Duration = Duration::from_secs(200);

/// Connection-task backstop. The inner budget returns first.
pub(crate) const BLOCKING_TIMEOUT_MS: u64 = 210_000;

const APK_FILE_NAME: &str = "rustdesk-peer-update.apk";

struct Admitted {
    url: String,
    size: u64,
    sha256: String,
}

#[derive(Debug)]
struct IdMaterial {
    device_id: String,
    enc_id: String,
    plain_id: String,
}

impl IdMaterial {
    fn blank() -> Self {
        Self {
            device_id: String::new(),
            enc_id: String::new(),
            plain_id: String::new(),
        }
    }
}

pub(crate) async fn run_with_deadline(arg: String) -> &'static str {
    match hbb_common::timeout(
        BLOCKING_TIMEOUT_MS,
        tokio::task::spawn_blocking(move || execute(&arg)),
    )
    .await
    {
        Ok(Ok(code)) => code,
        Ok(Err(e)) => {
            log::error!("peer apk update task failed: {e}");
            UPDATE_INSTALL
        }
        Err(_) => {
            log::error!("peer apk update exceeded {BLOCKING_TIMEOUT_MS} ms");
            UPDATE_NETWORK
        }
    }
}

pub(crate) fn execute(arg: &str) -> &'static str {
    if semantic_action::validate(semantic_action::CHECK_UPDATE, arg) != semantic_action::OK {
        return semantic_action::UNSUPPORTED;
    }
    #[cfg(target_os = "android")]
    {
        return execute_android(arg);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = arg;
        semantic_action::UNSUPPORTED
    }
}

#[cfg(target_os = "android")]
fn execute_android(arg: &str) -> &'static str {
    let deadline = Instant::now() + INNER_BUDGET;
    let release = match resolve_release(arg, deadline) {
        Ok(Some(url)) => url,
        Ok(None) => return semantic_action::OK,
        Err(code) => return code,
    };
    let artifact = match load_artifact(&release, deadline) {
        Ok(Some(artifact)) => artifact,
        Ok(None) => return semantic_action::OK,
        Err(code) => return code,
    };
    let before = match snapshot_id() {
        Ok(id) => id,
        Err(code) => return code,
    };
    let dest = apk_dest();
    let downloaded = download_apk(
        &artifact.url,
        artifact.size,
        &artifact.sha256,
        &dest,
        deadline,
    );
    let code = if downloaded == semantic_action::OK && remaining(deadline) < INSTALL_TIMEOUT {
        UPDATE_NETWORK
    } else if downloaded != semantic_action::OK {
        downloaded
    } else {
        match jni_certs(&dest) {
            Ok((installed, candidate)) => after_download(
                downloaded,
                &installed,
                &candidate,
                &before,
                || jni_cover(&dest),
                || snapshot_id().unwrap_or_else(|_| IdMaterial::blank()),
            ),
            Err(code) => code,
        }
    };
    let _ = remove_regular(&dest);
    let _ = remove_regular(&partial_path(&dest));
    if code != semantic_action::OK {
        log::error!("peer apk check_update -> {code}");
    }
    code
}

#[cfg(target_os = "android")]
fn resolve_release(arg: &str, deadline: Instant) -> Result<Option<String>, &'static str> {
    if arg.is_empty() {
        let body = fetch_version(deadline)?;
        return release_page_from_version_body(&body, crate::VERSION);
    }
    explicit_release(arg)
}

#[cfg(target_os = "android")]
fn explicit_release(arg: &str) -> Result<Option<String>, &'static str> {
    let release_id =
        crate::common::release_id_from_update_url(arg).map_err(|_| semantic_action::UNSUPPORTED)?;
    let display = crate::common::display_version_from_release_id(&release_id)
        .map_err(|_| semantic_action::UNSUPPORTED)?;
    if get_version_number(&display) <= get_version_number(crate::VERSION) {
        Ok(None)
    } else {
        Ok(Some(arg.to_owned()))
    }
}

#[cfg(target_os = "android")]
fn fetch_version(deadline: Instant) -> Result<Vec<u8>, &'static str> {
    let (request, url) =
        hbb_common::version_check_request(hbb_common::VER_TYPE_RUSTDESK_CLIENT.to_owned());
    let client = create_http_client_with_url_strict(&url).map_err(|e| {
        log::error!("peer apk version client: {e}");
        UPDATE_NETWORK
    })?;
    let timeout = remaining(deadline).min(VERSION_TIMEOUT);
    if timeout.is_zero() {
        return Err(UPDATE_NETWORK);
    }
    let response = client
        .post(&url)
        .timeout(timeout)
        .json(&request)
        .send()
        .map_err(|e| {
            log::error!("peer apk version check: {e}");
            UPDATE_NETWORK
        })?;
    read_http(response, VERSION_BODY_MAX_BYTES, deadline)
}

#[cfg(target_os = "android")]
fn load_artifact(release_page: &str, deadline: Instant) -> Result<Option<Admitted>, &'static str> {
    let release_id =
        crate::common::release_id_from_update_url(release_page).map_err(|_| UPDATE_NETWORK)?;
    let display =
        crate::common::display_version_from_release_id(&release_id).map_err(|_| UPDATE_NETWORK)?;
    let arch = android_update_arch();
    if arch == "unknown" {
        return Err(semantic_action::UNSUPPORTED);
    }
    let prefix = format!("https://github.com/rustdesk/rustdesk/releases/download/{release_id}/");
    let metadata_url = format!("{prefix}rustdesk-update.json");
    let signature_url = format!("{metadata_url}.sig");
    let metadata = fetch_limited(&metadata_url, METADATA_MAX_BYTES, deadline)?;
    let signature = fetch_limited(&signature_url, METADATA_MAX_BYTES, deadline)?;
    let requirements = crate::update_metadata::UpdateMetadataRequirements {
        expected_version: display.as_str(),
        expected_release_id: release_id.as_str(),
        expected_artifact_url_prefix: prefix.as_str(),
        artifact: crate::update_metadata::UpdateArtifactQuery {
            platform: "android",
            arch,
            format: "apk",
        },
    };
    match crate::update_metadata::verify_update_metadata(&metadata, &signature, requirements) {
        Ok(artifact) => {
            admit_apk_artifact(&artifact.url, artifact.size, &artifact.sha256)?;
            if get_version_number(&artifact.version) <= get_version_number(crate::VERSION) {
                return Ok(None);
            }
            Ok(Some(Admitted {
                url: artifact.url,
                size: artifact.size,
                sha256: artifact.sha256,
            }))
        }
        Err(e) => {
            log::error!("peer apk metadata rejected: {e}");
            Err(classify_metadata(&e.to_string()))
        }
    }
}

#[cfg(target_os = "android")]
fn fetch_limited(url: &str, max: u64, deadline: Instant) -> Result<Vec<u8>, &'static str> {
    if max > METADATA_MAX_BYTES {
        return Err(UPDATE_CHECKSUM);
    }
    let client = create_http_client_with_url_strict(url).map_err(|e| {
        log::error!("peer apk sidecar client: {e}");
        UPDATE_NETWORK
    })?;
    let timeout = remaining(deadline).min(SIDECAR_TIMEOUT);
    if timeout.is_zero() {
        return Err(UPDATE_NETWORK);
    }
    let response = client.get(url).timeout(timeout).send().map_err(|e| {
        log::error!("peer apk sidecar: {e}");
        UPDATE_NETWORK
    })?;
    read_http(response, max, deadline)
}

#[cfg(target_os = "android")]
fn read_http(
    mut response: reqwest::blocking::Response,
    max: u64,
    deadline: Instant,
) -> Result<Vec<u8>, &'static str> {
    if response.url().scheme() != "https" || !response.status().is_success() {
        log::error!("peer apk http status {}", response.status());
        return Err(UPDATE_NETWORK);
    }
    if let Some(len) = response.content_length() {
        if len > max {
            return Err(UPDATE_CHECKSUM);
        }
    }
    let mut body = (&mut response).take(max.saturating_add(1));
    let mut out = Vec::new();
    let mut buf = [0u8; CHUNK];
    loop {
        if Instant::now() >= deadline {
            return Err(UPDATE_NETWORK);
        }
        let n = body.read(&mut buf).map_err(|e| {
            log::error!("peer apk body: {e}");
            UPDATE_NETWORK
        })?;
        if n == 0 {
            break;
        }
        if out.len() as u64 + n as u64 > max {
            return Err(UPDATE_CHECKSUM);
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

#[cfg(target_os = "android")]
fn download_apk(
    url: &str,
    expected_size: u64,
    expected_sha: &str,
    dest: &Path,
    deadline: Instant,
) -> &'static str {
    if let Err(code) = admit_apk_artifact(url, expected_size, expected_sha) {
        return code;
    }
    let timeout = remaining(deadline).min(DOWNLOAD_TIMEOUT);
    if timeout.is_zero() {
        return UPDATE_NETWORK;
    }
    let client = match create_http_client_with_url_strict(url) {
        Ok(client) => client,
        Err(e) => {
            log::error!("peer apk download client: {e}");
            return UPDATE_NETWORK;
        }
    };
    let response = match client.get(url).timeout(timeout).send() {
        Ok(response) => response,
        Err(e) => {
            log::error!("peer apk download: {e}");
            return UPDATE_NETWORK;
        }
    };
    if response.url().scheme() != "https" || !response.status().is_success() {
        log::error!("peer apk download status {}", response.status());
        return UPDATE_NETWORK;
    }
    if let Some(len) = response.content_length() {
        if len != expected_size || len > APK_MAX_BYTES {
            return UPDATE_CHECKSUM;
        }
    }
    let mut body = response.take(expected_size.saturating_add(1));
    write_and_verify(&mut body, expected_size, expected_sha, dest, deadline)
}

#[cfg(target_os = "android")]
fn snapshot_id() -> Result<IdMaterial, &'static str> {
    let device_id = hbb_common::config::Config::get_id_or(String::new());
    let bytes = read_capped(&hbb_common::config::Config::file(), ID_FILE_MAX_BYTES)?;
    id_material(&device_id, &bytes)
}

#[cfg(target_os = "android")]
fn apk_dest() -> PathBuf {
    hbb_common::config::Config::path(APK_FILE_NAME)
}

#[cfg(target_os = "android")]
fn jni_certs(path: &Path) -> Result<(Vec<u8>, Vec<u8>), &'static str> {
    let path = path_str(path)?;
    match scrap::android::call_main_service_peer_apk_certs(&path) {
        Ok(raw) => parse_cert_wire(&raw),
        Err(e) => {
            log::error!("peer apk cert jni: {e}");
            Err(UPDATE_SIGNATURE)
        }
    }
}

#[cfg(target_os = "android")]
fn jni_cover(path: &Path) -> &'static str {
    let Ok(path) = path_str(path) else {
        return UPDATE_INSTALL;
    };
    match scrap::android::call_main_service_peer_apk_cover_install(&path) {
        Ok(code) => known_update_code(&code),
        Err(e) => {
            log::error!("peer apk install jni: {e}");
            UPDATE_INSTALL
        }
    }
}

#[cfg(target_os = "android")]
fn path_str(path: &Path) -> Result<String, &'static str> {
    path.to_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or(UPDATE_INSTALL)
}

fn android_update_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "arm" => "armv7",
        "x86_64" => "x86_64",
        "x86" => "x86",
        _ => "unknown",
    }
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// `Ok(None)` means this release is not newer than `current_version`.
fn release_page_from_version_body(
    bytes: &[u8],
    current_version: &str,
) -> Result<Option<String>, &'static str> {
    if bytes.len() as u64 > VERSION_BODY_MAX_BYTES {
        return Err(UPDATE_CHECKSUM);
    }
    let resp: hbb_common::VersionCheckResponse =
        serde_json::from_slice(bytes).map_err(|_| UPDATE_NETWORK)?;
    if resp.url.is_empty() {
        return Err(UPDATE_NETWORK);
    }
    let release_id =
        crate::common::release_id_from_update_url(&resp.url).map_err(|_| UPDATE_NETWORK)?;
    let display =
        crate::common::display_version_from_release_id(&release_id).map_err(|_| UPDATE_NETWORK)?;
    if get_version_number(&display) > get_version_number(current_version) {
        Ok(Some(resp.url))
    } else {
        Ok(None)
    }
}

fn admit_apk_artifact(url: &str, size: u64, sha256: &str) -> Result<(), &'static str> {
    if size == 0 || size > APK_MAX_BYTES || !sha256_hex(sha256) || !github_release_apk_url(url) {
        return Err(UPDATE_CHECKSUM);
    }
    Ok(())
}

fn classify_metadata(msg: &str) -> &'static str {
    let msg = msg.to_ascii_lowercase();
    if msg.contains("signature") {
        UPDATE_SIGNATURE
    } else if msg.contains("not found") || msg.contains("multiple matching") {
        semantic_action::UNSUPPORTED
    } else {
        UPDATE_CHECKSUM
    }
}

fn github_release_apk_url(url: &str) -> bool {
    if !url.starts_with("https://github.com/") {
        return false;
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return false;
    }
    let Some(mut segments) = parsed.path_segments() else {
        return false;
    };
    let owner = segments.next();
    let repo = segments.next();
    let releases = segments.next();
    let download = segments.next();
    let tag = segments.next();
    let filename = segments.next();
    matches!(
        (owner, repo, releases, download, tag, filename, segments.next()),
        (
            Some("rustdesk"),
            Some("rustdesk"),
            Some("releases"),
            Some("download"),
            Some(tag),
            Some(filename),
            None
        ) if !tag.is_empty() && plain_apk_name(filename)
    )
}

fn plain_apk_name(filename: &str) -> bool {
    if !filename.ends_with(".apk") || filename.len() <= 4 || filename.len() > 180 {
        return false;
    }
    if !filename
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return false;
    }
    let mut components = Path::new(filename).components();
    matches!(
        components.next(),
        Some(Component::Normal(name)) if name.to_str() == Some(filename)
    ) && components.next().is_none()
}

fn sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn copy_capped<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    max_bytes: u64,
    deadline: Instant,
    mut now: impl FnMut() -> Instant,
) -> Result<u64, &'static str> {
    let mut buf = [0u8; CHUNK];
    let mut total = 0u64;
    loop {
        if now() >= deadline {
            return Err(UPDATE_NETWORK);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::TimedOut => return Err(UPDATE_NETWORK),
            Err(_) => return Err(UPDATE_NETWORK),
        };
        let n64 = n as u64;
        if total.saturating_add(n64) > max_bytes {
            return Err(UPDATE_CHECKSUM);
        }
        writer.write_all(&buf[..n]).map_err(|_| UPDATE_NETWORK)?;
        total += n64;
    }
    Ok(total)
}

fn write_and_verify<R: Read>(
    reader: &mut R,
    expected_size: u64,
    expected_sha: &str,
    dest: &Path,
    deadline: Instant,
) -> &'static str {
    if expected_size == 0 || expected_size > APK_MAX_BYTES || !sha256_hex(expected_sha) {
        return UPDATE_CHECKSUM;
    }
    let partial = partial_path(dest);
    let result = (|| {
        let mut file = create_new_file(&partial)?;
        let n = copy_capped(reader, &mut file, expected_size, deadline, Instant::now)?;
        file.flush().map_err(|_| UPDATE_NETWORK)?;
        drop(file);
        if n != expected_size || Instant::now() >= deadline {
            return Err(if n != expected_size {
                UPDATE_CHECKSUM
            } else {
                UPDATE_NETWORK
            });
        }
        let code = verify_capped(&partial, expected_sha, expected_size, deadline);
        if code != semantic_action::OK {
            return Err(code);
        }
        replace_file(&partial, dest)?;
        Ok(())
    })();
    match result {
        Ok(()) => semantic_action::OK,
        Err(code) => {
            let _ = remove_regular(&partial);
            let _ = remove_regular(dest);
            code
        }
    }
}

fn verify_capped(
    path: &Path,
    expected_sha: &str,
    expected_size: u64,
    deadline: Instant,
) -> &'static str {
    if !sha256_hex(expected_sha) || expected_size == 0 || expected_size > APK_MAX_BYTES {
        return UPDATE_CHECKSUM;
    }
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(_) => return UPDATE_CHECKSUM,
    };
    if !meta.file_type().is_file() || meta.len() != expected_size {
        return UPDATE_CHECKSUM;
    }
    if Instant::now() >= deadline {
        return UPDATE_CHECKSUM;
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return UPDATE_CHECKSUM,
    };
    let mut reader = DeadlineRead {
        inner: &mut file,
        deadline,
        limit: expected_size,
        left: expected_size,
    };
    match verify_sha256_reader(&mut reader, expected_sha) {
        Ok(()) => semantic_action::OK,
        Err(_) => UPDATE_CHECKSUM,
    }
}

struct DeadlineRead<R> {
    inner: R,
    deadline: Instant,
    limit: u64,
    left: u64,
}

impl<R: Read> Read for DeadlineRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "verify timeout"));
        }
        if self.left == 0 {
            return Ok(0);
        }
        let max = usize::min(buf.len(), self.left as usize);
        let n = self.inner.read(&mut buf[..max])?;
        self.left -= n as u64;
        Ok(n)
    }
}

impl<R: Seek> Seek for DeadlineRead<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "verify timeout"));
        }
        let at = self.inner.seek(pos)?;
        self.left = self.limit.saturating_sub(at);
        Ok(at)
    }
}

fn create_new_file(path: &Path) -> Result<File, &'static str> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| UPDATE_NETWORK)?;
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_file() {
            return Err(UPDATE_INSTALL);
        }
        std::fs::remove_file(path).map_err(|_| UPDATE_INSTALL)?;
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| UPDATE_NETWORK)
}

fn replace_file(partial: &Path, dest: &Path) -> Result<(), &'static str> {
    if let Ok(meta) = std::fs::symlink_metadata(dest) {
        if !meta.file_type().is_file() {
            return Err(UPDATE_INSTALL);
        }
        std::fs::remove_file(dest).map_err(|_| UPDATE_INSTALL)?;
    }
    std::fs::rename(partial, dest).map_err(|_| UPDATE_INSTALL)
}

fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".partial");
    dest.with_file_name(name)
}

fn remove_regular(path: &Path) -> Result<(), ()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => std::fs::remove_file(path).map_err(|_| ()),
        Ok(_) => Err(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>, &'static str> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| UPDATE_INSTALL)?;
    if !meta.file_type().is_file() || meta.len() > max {
        return Err(UPDATE_INSTALL);
    }
    let mut file = File::open(path).map_err(|_| UPDATE_INSTALL)?;
    let mut out = Vec::new();
    let mut buf = [0u8; CHUNK];
    loop {
        let n = file.read(&mut buf).map_err(|_| UPDATE_INSTALL)?;
        if n == 0 {
            break;
        }
        if out.len() as u64 + n as u64 > max {
            return Err(UPDATE_INSTALL);
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

fn id_material(device_id: &str, file: &[u8]) -> Result<IdMaterial, &'static str> {
    if file.len() as u64 > ID_FILE_MAX_BYTES {
        return Err(UPDATE_INSTALL);
    }
    if device_id.is_empty() || device_id.len() > DEVICE_ID_MAX {
        return Err(UPDATE_INSTALL);
    }
    let enc_id = toml_field(file, "enc_id").unwrap_or_default();
    let plain_id = toml_field(file, "id").unwrap_or_default();
    if enc_id.len() > ENC_ID_MAX || plain_id.len() > DEVICE_ID_MAX {
        return Err(UPDATE_ID_CHANGED);
    }
    if enc_id.is_empty() && plain_id != device_id {
        return Err(UPDATE_INSTALL);
    }
    Ok(IdMaterial {
        device_id: device_id.to_owned(),
        enc_id,
        plain_id,
    })
}

fn id_after(before: &IdMaterial, after: &IdMaterial) -> &'static str {
    if before.device_id.is_empty()
        || before.device_id.len() > DEVICE_ID_MAX
        || after.device_id != before.device_id
    {
        return UPDATE_ID_CHANGED;
    }
    if !before.enc_id.is_empty() {
        return if after.enc_id == before.enc_id {
            semantic_action::OK
        } else {
            UPDATE_ID_CHANGED
        };
    }
    if before.plain_id == before.device_id && after.plain_id == before.device_id {
        semantic_action::OK
    } else {
        UPDATE_ID_CHANGED
    }
}

fn toml_field(file: &[u8], key: &str) -> Option<String> {
    let text = std::str::from_utf8(file).ok()?;
    for line in text.lines() {
        let Some(value) = field_line(line, key) else {
            continue;
        };
        if let Some(parsed) = unquote(value) {
            return Some(parsed);
        }
    }
    None
}

fn field_line<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let line = line.trim();
    let rest = line.strip_prefix(key)?;
    let rest = if let Some(rest) = rest.strip_prefix('=') {
        rest
    } else if rest.starts_with(char::is_whitespace) {
        rest.trim_start().strip_prefix('=')?
    } else {
        return None;
    };
    Some(rest.trim())
}

fn unquote(value: &str) -> Option<String> {
    let mut chars = value.chars();
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == quote {
            return if chars.next().is_none() {
                Some(out)
            } else {
                None
            };
        }
        if c == '\\' {
            match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                _ => return None,
            }
        } else {
            out.push(c);
        }
        if out.len() > ENC_ID_MAX {
            return None;
        }
    }
    None
}

fn authorize_cover(installed: &[u8], candidate: &[u8]) -> &'static str {
    if installed.len() == 32 && installed == candidate {
        semantic_action::OK
    } else {
        UPDATE_SIGNATURE
    }
}

fn after_download(
    downloaded: &'static str,
    installed: &[u8],
    candidate: &[u8],
    before: &IdMaterial,
    cover: impl FnOnce() -> &'static str,
    after: impl FnOnce() -> IdMaterial,
) -> &'static str {
    if downloaded != semantic_action::OK {
        return downloaded;
    }
    finish_cover(installed, candidate, before, cover, after)
}

fn finish_cover(
    installed: &[u8],
    candidate: &[u8],
    before: &IdMaterial,
    cover: impl FnOnce() -> &'static str,
    after: impl FnOnce() -> IdMaterial,
) -> &'static str {
    if authorize_cover(installed, candidate) != semantic_action::OK {
        return UPDATE_SIGNATURE;
    }
    let code = known_update_code(&cover());
    if code != semantic_action::OK {
        return code;
    }
    id_after(before, &after())
}

fn known_update_code(code: &str) -> &'static str {
    match code {
        semantic_action::OK => semantic_action::OK,
        UPDATE_NETWORK => UPDATE_NETWORK,
        UPDATE_CHECKSUM => UPDATE_CHECKSUM,
        UPDATE_SIGNATURE => UPDATE_SIGNATURE,
        UPDATE_INSTALL => UPDATE_INSTALL,
        UPDATE_ID_CHANGED => UPDATE_ID_CHANGED,
        semantic_action::UNSUPPORTED => semantic_action::UNSUPPORTED,
        _ => UPDATE_INSTALL,
    }
}

fn parse_cert_wire(raw: &str) -> Result<(Vec<u8>, Vec<u8>), &'static str> {
    let Some(rest) = raw.strip_prefix("ok\n") else {
        return Err(known_update_code(raw.trim()));
    };
    let mut lines = rest.split('\n');
    let installed = decode_sha256_hex(lines.next().unwrap_or(""))?;
    let candidate = decode_sha256_hex(lines.next().unwrap_or(""))?;
    if lines.next().is_some() {
        return Err(UPDATE_SIGNATURE);
    }
    Ok((installed.to_vec(), candidate.to_vec()))
}

fn decode_sha256_hex(value: &str) -> Result<[u8; 32], &'static str> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(UPDATE_SIGNATURE);
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|_| UPDATE_SIGNATURE)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SHA_RUSTDESK: &str = "304ca1638c5effa6832e0e15b958a8f74847efe4df9c3f3187216e921c168fed";
    const RELEASE_APK: &str =
        "https://github.com/rustdesk/rustdesk/releases/download/1.4.6/rustdesk-1.4.6-aarch64.apk";

    struct PanicRead;

    impl Read for PanicRead {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            panic!("read past the cap or deadline");
        }
    }

    fn cert(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn material(enc: &str) -> IdMaterial {
        IdMaterial {
            device_id: "123456789".to_owned(),
            enc_id: enc.to_owned(),
            plain_id: String::new(),
        }
    }

    #[test]
    fn download_is_capped_and_fails_loud() {
        assert_eq!(APK_MAX_BYTES, 96 * 1024 * 1024);
        assert_eq!(CHUNK, 8192);
        assert_eq!(DOWNLOAD_TIMEOUT, Duration::from_secs(100));
        assert!(INNER_BUDGET.as_millis() < BLOCKING_TIMEOUT_MS as u128);
        assert!(admit_apk_artifact(RELEASE_APK, 8, SHA_RUSTDESK).is_ok());
        assert_eq!(
            admit_apk_artifact(RELEASE_APK, 0, SHA_RUSTDESK),
            Err(UPDATE_CHECKSUM)
        );
        assert_eq!(
            admit_apk_artifact(RELEASE_APK, APK_MAX_BYTES + 1, SHA_RUSTDESK),
            Err(UPDATE_CHECKSUM)
        );
        assert_eq!(
            admit_apk_artifact(RELEASE_APK, 8, "zz"),
            Err(UPDATE_CHECKSUM)
        );
        assert_eq!(
            admit_apk_artifact(
                "http://github.com/rustdesk/rustdesk/releases/download/1.4.6/a.apk",
                8,
                SHA_RUSTDESK
            ),
            Err(UPDATE_CHECKSUM)
        );
        assert_eq!(
            admit_apk_artifact("https://example.com/a.apk", 8, SHA_RUSTDESK),
            Err(UPDATE_CHECKSUM)
        );

        let mut sink = Vec::new();
        let deadline = Instant::now();
        assert_eq!(
            copy_capped(&mut PanicRead, &mut sink, 100, deadline, Instant::now),
            Err(UPDATE_NETWORK)
        );
        assert!(sink.is_empty());

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut once = Cursor::new(vec![1u8; 8]);
        let mut sink = Vec::new();
        assert_eq!(
            copy_capped(&mut once, &mut sink, 4, deadline, Instant::now),
            Err(UPDATE_CHECKSUM)
        );
        assert!(sink.is_empty());
        assert_eq!(once.position(), 8);

        let dir = std::env::temp_dir().join(format!(
            "peer-apk-update-{}-{}",
            std::process::id(),
            hbb_common::rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join(APK_FILE_NAME);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut body = Cursor::new(b"rustdesk".to_vec());
        assert_eq!(
            write_and_verify(&mut body, 8, SHA_RUSTDESK, &dest, deadline),
            semantic_action::OK
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"rustdesk");
        let mut body = Cursor::new(b"rustdesk".to_vec());
        assert_eq!(
            write_and_verify(&mut body, 8, &"ab".repeat(32), &dest, deadline),
            UPDATE_CHECKSUM
        );
        assert!(!dest.exists());
        assert!(!partial_path(&dest).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signature_or_checksum_does_not_install() {
        let before = material("enc-same");
        let mut calls = 0u32;
        let code = finish_cover(
            &cert(1),
            &cert(2),
            &before,
            || {
                calls += 1;
                semantic_action::OK
            },
            || before.clone_like(),
        );
        assert_eq!(code, UPDATE_SIGNATURE);
        assert_eq!(calls, 0);
        assert_ne!(code, semantic_action::OK);

        let code = finish_cover(
            &cert(1),
            &[],
            &before,
            || panic!("cover"),
            || before.clone_like(),
        );
        assert_eq!(code, UPDATE_SIGNATURE);

        let mut calls = 0u32;
        let code = after_download(
            UPDATE_CHECKSUM,
            &cert(1),
            &cert(1),
            &before,
            || {
                calls += 1;
                semantic_action::OK
            },
            || before.clone_like(),
        );
        assert_eq!(code, UPDATE_CHECKSUM);
        assert_eq!(calls, 0);

        assert_eq!(
            classify_metadata("invalid update metadata signature"),
            UPDATE_SIGNATURE
        );
        assert_eq!(
            classify_metadata("matching update artifact not found"),
            semantic_action::UNSUPPORTED
        );
        assert_eq!(
            classify_metadata("invalid update artifact sha256"),
            UPDATE_CHECKSUM
        );
        assert_eq!(parse_cert_wire("signature").unwrap_err(), UPDATE_SIGNATURE);
        let hex = "ab".repeat(32);
        let (a, b) = parse_cert_wire(&format!("ok\n{hex}\n{hex}")).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn cover_install_keeps_the_same_device_id() {
        let before = material("enc-v1");
        let mut calls = 0u32;
        let code = finish_cover(
            &cert(7),
            &cert(7),
            &before,
            || {
                calls += 1;
                semantic_action::OK
            },
            || material("enc-v1"),
        );
        assert_eq!(calls, 1);
        assert_eq!(code, semantic_action::OK);

        let code = finish_cover(
            &cert(7),
            &cert(7),
            &before,
            || semantic_action::OK,
            || material("enc-other"),
        );
        assert_eq!(code, UPDATE_ID_CHANGED);
        assert!(!semantic_action::is_peer_success(code));

        let code = finish_cover(
            &cert(7),
            &cert(7),
            &before,
            || semantic_action::OK,
            IdMaterial::blank,
        );
        assert_eq!(code, UPDATE_ID_CHANGED);

        let code = finish_cover(
            &cert(7),
            &cert(7),
            &before,
            || UPDATE_INSTALL,
            || panic!("id read after a failed install"),
        );
        assert_eq!(code, UPDATE_INSTALL);

        let file = b"id = \"\"\nenc_id = \"enc-v1\"\n";
        let parsed = id_material("123456789", file).unwrap();
        assert_eq!(parsed.enc_id, "enc-v1");
        assert_eq!(id_after(&parsed, &parsed), semantic_action::OK);
        let wiped = id_material("123456789", b"id = \"123456789\"\nenc_id = \"\"\n").unwrap();
        assert_eq!(id_after(&parsed, &wiped), UPDATE_ID_CHANGED);
        assert_eq!(id_material("", file).unwrap_err(), UPDATE_INSTALL);
        assert_eq!(
            id_material("123456789", &vec![b'x'; ID_FILE_MAX_BYTES as usize + 1]).unwrap_err(),
            UPDATE_INSTALL
        );
        assert_eq!(INSTALL_TIMEOUT, Duration::from_secs(40));
    }

    #[test]
    fn version_body_picks_a_newer_github_release() {
        let newer = br#"{"url":"https://github.com/rustdesk/rustdesk/releases/tag/1.4.7"}"#;
        assert_eq!(
            release_page_from_version_body(newer, "1.4.6")
                .unwrap()
                .as_deref(),
            Some("https://github.com/rustdesk/rustdesk/releases/tag/1.4.7")
        );
        let same = br#"{"url":"https://github.com/rustdesk/rustdesk/releases/tag/1.4.6"}"#;
        assert_eq!(release_page_from_version_body(same, "1.4.6").unwrap(), None);
        assert_eq!(
            release_page_from_version_body(br#"{"url":"https://example.com/x"}"#, "1.4.6"),
            Err(UPDATE_NETWORK)
        );
        assert_eq!(
            release_page_from_version_body(
                &vec![b'{'; VERSION_BODY_MAX_BYTES as usize + 1],
                "1.4.6"
            ),
            Err(UPDATE_CHECKSUM)
        );
        assert_eq!(android_update_arch() == "unknown", false);
        #[cfg(not(target_os = "android"))]
        {
            assert_eq!(execute(""), semantic_action::UNSUPPORTED);
            assert_eq!(
                execute("https://example.com/a.apk"),
                semantic_action::UNSUPPORTED
            );
        }
    }
}

#[cfg(test)]
impl IdMaterial {
    fn clone_like(&self) -> Self {
        Self {
            device_id: self.device_id.clone(),
            enc_id: self.enc_id.clone(),
            plain_id: self.plain_id.clone(),
        }
    }
}
