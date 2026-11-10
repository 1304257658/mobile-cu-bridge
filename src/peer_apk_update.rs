//! Check and cover-install the Android peer APK from this fork's GitHub Releases.
//!
//! Discovery is `https://github.com/1304257658/mobile-cu-bridge` only. The sha256
//! comes from the release `SHA256SUMS` file, checked against the asset digest
//! when GitHub sends one. `api.rustdesk.com` and `github.com/rustdesk/rustdesk`
//! are refused.
//!
//! Cover-install does not uninstall. The OS kills this process when the
//! package is replaced, so the in-process path never reports `ok`. The id and
//! `enc_id` are written to disk first. The next process compares that snapshot
//! and the controller hears `ok` or `id_changed` when it reconnects.
//!
//! When the installed APK sha256 equals the `SHA256SUMS` entry for that file,
//! the peer returns `ok` with detail `already_latest` and does not download or
//! cover-install. Asset digest alone does not skip. A missing `SHA256SUMS`, or
//! one with no entry for the package, is logged and the same-signature
//! cover-install continues. The local digest is cached by path, mtime, length,
//! and versionCode.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use crate::{
    hbbs_http::create_http_client_with_url_strict,
    semantic_action::{
        self, UPDATE_CHECKSUM, UPDATE_ID_CHANGED, UPDATE_INSTALL, UPDATE_NETWORK, UPDATE_SIGNATURE,
    },
    update_hash::{sha256_reader_hex, verify_sha256_reader},
};
use hbb_common::log;
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

const APK_MAX_BYTES: u64 = 96 * 1024 * 1024;
const RELEASES_JSON_MAX: u64 = 512 * 1024;
const SUMS_MAX_BYTES: u64 = 64 * 1024;
const ID_FILE_MAX_BYTES: u64 = 256 * 1024;
const SNAPSHOT_MAX_BYTES: u64 = 8 * 1024;
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
const SNAPSHOT_FILE: &str = "peer-update-id-snapshot";
const VERDICT_FILE: &str = "peer-update-id-verdict";
const SHA_CACHE_FILE: &str = "peer-update-apk-sha";
const SHA_CACHE_MAX: u64 = 8 * 1024;
const FORK_OWNER: &str = "1304257658";
const FORK_REPO: &str = "mobile-cu-bridge";
const RELEASES_API: &str =
    "https://api.github.com/repos/1304257658/mobile-cu-bridge/releases?per_page=20";

/// Install was handed to the OS. Not a wire result and not `ok`.
/// The process is expected to die. `ok` / `id_changed` are reported after restart.
pub(crate) const COMMITTED: &str = "committed";

pub(crate) struct CheckUpdateReply {
    pub code: &'static str,
    pub detail: &'static str,
}

impl CheckUpdateReply {
    fn bare(code: &'static str) -> Self {
        Self { code, detail: "" }
    }

    fn skipped() -> Self {
        Self {
            code: semantic_action::OK,
            detail: semantic_action::CHECK_UPDATE_ALREADY_LATEST,
        }
    }
}

#[derive(Debug, PartialEq)]
struct Admitted {
    url: String,
    size: u64,
    sha256: String,
    /// Set only when `SHA256SUMS` names this file. Absent means do not skip.
    sums_sha: Option<String>,
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

pub(crate) async fn run_with_deadline(arg: String) -> CheckUpdateReply {
    match hbb_common::timeout(
        BLOCKING_TIMEOUT_MS,
        hbb_common::tokio::task::spawn_blocking(move || execute(&arg)),
    )
    .await
    {
        Ok(Ok(reply)) => reply,
        Ok(Err(e)) => {
            log::error!("peer apk update task failed: {e}");
            CheckUpdateReply::bare(UPDATE_INSTALL)
        }
        Err(_) => {
            log::error!("peer apk update exceeded {BLOCKING_TIMEOUT_MS} ms");
            CheckUpdateReply::bare(UPDATE_NETWORK)
        }
    }
}

pub(crate) fn execute(arg: &str) -> CheckUpdateReply {
    if semantic_action::validate(semantic_action::CHECK_UPDATE, arg) != semantic_action::OK {
        return CheckUpdateReply::bare(semantic_action::UNSUPPORTED);
    }
    #[cfg(target_os = "android")]
    {
        return execute_android(arg);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = arg;
        CheckUpdateReply::bare(semantic_action::UNSUPPORTED)
    }
}

#[cfg(target_os = "android")]
fn execute_android(arg: &str) -> CheckUpdateReply {
    let deadline = Instant::now() + INNER_BUDGET;
    let Some(dir) = config_dir() else {
        return CheckUpdateReply::bare(UPDATE_INSTALL);
    };
    if let Some(code) = take_verdict(&dir) {
        return CheckUpdateReply::bare(code);
    }
    let artifact = match discover(arg, deadline) {
        Ok(artifact) => artifact,
        Err(code) => return CheckUpdateReply::bare(code),
    };
    if skip_same_sums(
        local_installed_sha(&dir).as_deref(),
        artifact.sums_sha.as_deref(),
    ) {
        log::info!("peer apk already_latest; skip download and cover-install");
        return CheckUpdateReply::skipped();
    }
    let before = match snapshot_id() {
        Ok(id) => id,
        Err(code) => return CheckUpdateReply::bare(code),
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
        let _ = remove_snapshot(&dir);
        UPDATE_NETWORK
    } else if downloaded != semantic_action::OK {
        let _ = remove_snapshot(&dir);
        downloaded
    } else {
        match jni_certs(&dest) {
            Ok((installed, candidate)) => {
                if authorize_cover(&installed, &candidate) != semantic_action::OK {
                    UPDATE_SIGNATURE
                } else if write_snapshot(&dir, &before).is_err() {
                    UPDATE_INSTALL
                } else {
                    finish_committed_or_fail(&dir, &jni_cover(&dest))
                }
            }
            Err(code) => code,
        }
    };
    let _ = remove_regular(&dest);
    let _ = remove_regular(&partial_path(&dest));
    if code != semantic_action::OK && code != COMMITTED {
        log::error!("peer apk check_update -> {code}");
    }
    CheckUpdateReply::bare(code)
}

#[cfg(target_os = "android")]
fn discover(arg: &str, deadline: Instant) -> Result<Admitted, &'static str> {
    let arch = android_update_arch();
    if arch == "unknown" {
        return Err(semantic_action::UNSUPPORTED);
    }
    let (only_tag, only_name) = if arg.is_empty() {
        (None, None)
    } else {
        match fork_release_ref(arg) {
            Some(ForkRef::Tag(tag)) => (Some(tag), None),
            Some(ForkRef::Asset { tag, name }) => (Some(tag), Some(name)),
            None => return Err(semantic_action::UNSUPPORTED),
        }
    };
    if let Some(tag) = only_tag.as_deref() {
        if device_tag_arch(tag) != Some(arch) {
            return Err(semantic_action::UNSUPPORTED);
        }
    }
    let url = match only_tag.as_deref() {
        None => RELEASES_API.to_owned(),
        Some(tag) => {
            format!("https://api.github.com/repos/{FORK_OWNER}/{FORK_REPO}/releases/tags/{tag}")
        }
    };
    let body = fetch_limited(&url, RELEASES_JSON_MAX, deadline)?;
    let choice = choose_release(&body, arch, only_tag.as_deref(), only_name.as_deref())?;
    let sums_text = if choice.has_sums {
        let sums_url = format!(
            "https://github.com/{FORK_OWNER}/{FORK_REPO}/releases/download/{}/SHA256SUMS",
            choice.tag
        );
        match fetch_limited(&sums_url, SUMS_MAX_BYTES, deadline) {
            Ok(bytes) => match std::str::from_utf8(&bytes) {
                Ok(text) => Some(text.to_owned()),
                Err(_) => {
                    log::info!("peer apk SHA256SUMS is not utf-8; continue cover-install");
                    None
                }
            },
            Err(code) => {
                log::info!("peer apk SHA256SUMS fetch failed ({code}); continue cover-install");
                None
            }
        }
    } else {
        None
    };
    lock_sha(&choice, sums_text.as_deref())
}

#[cfg(target_os = "android")]
fn fetch_limited(url: &str, max: u64, deadline: Instant) -> Result<Vec<u8>, &'static str> {
    if banned_upstream(url) || !fork_fetch_url(url) || max == 0 || max > RELEASES_JSON_MAX {
        log::error!("peer apk refused fetch url");
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
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, "mobile-cu-bridge")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(timeout)
        .send()
        .map_err(|e| {
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
        Ok(code) if code.trim() == "committed" => "committed",
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
        "aarch64" => "arm64",
        "arm" => "arm",
        "x86_64" => "x86_64",
        "x86" => "x86",
        _ => "unknown",
    }
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// True only when `sums_sha` came from `SHA256SUMS` and matches the installed file.
fn skip_same_sums(local: Result<&str, &'static str>, sums_sha: Option<&str>) -> bool {
    let Some(release_sha) = sums_sha else {
        return false;
    };
    match local {
        Ok(local) if same_apk_sha(local, release_sha) => true,
        Ok(local) if sha256_hex(local) && sha256_hex(release_sha) => {
            log::info!("peer apk sha differs from SHA256SUMS; cover-install");
            false
        }
        Ok(_) => {
            log::info!("peer apk local sha unavailable (bad_sha); continue cover-install");
            false
        }
        Err(reason) => {
            log::info!("peer apk local sha unavailable ({reason}); continue cover-install");
            false
        }
    }
}

fn same_apk_sha(local: &str, release: &str) -> bool {
    sha256_hex(local) && sha256_hex(release) && local.eq_ignore_ascii_case(release)
}

#[cfg(target_os = "android")]
fn local_installed_sha(dir: &Path) -> Result<String, &'static str> {
    cached_apk_sha(dir, &installed_identity()?)
}

#[cfg(target_os = "android")]
fn installed_identity() -> Result<InstalledApk, &'static str> {
    let wire = match scrap::android::call_main_service_peer_apk_installed() {
        Ok(wire) => wire,
        Err(e) => {
            log::info!("peer apk installed identity jni: {e}");
            return Err("jni");
        }
    };
    parse_installed_wire(&wire)
}

struct InstalledApk {
    path: PathBuf,
    version_code: u64,
}

struct ApkStamp {
    path: String,
    mtime_secs: u64,
    mtime_nanos: u32,
    version_code: u64,
    len: u64,
}

fn parse_installed_wire(raw: &str) -> Result<InstalledApk, &'static str> {
    let mut lines = raw.split('\n');
    let head = lines.next().unwrap_or("").trim();
    if head != "ok" {
        return Err(known_identity_reason(head));
    }
    let path = lines.next().unwrap_or("");
    let version = lines.next().unwrap_or("").trim();
    if lines.next().is_some() {
        return Err("bad_wire");
    }
    if !installed_path_ok(path) {
        return Err("no_source");
    }
    if version.is_empty()
        || version.len() > 20
        || !version.bytes().all(|b| b.is_ascii_digit())
        || (version.len() > 1 && version.as_bytes()[0] == b'0')
    {
        return Err("bad_version");
    }
    let version_code = version.parse().map_err(|_| "bad_version")?;
    Ok(InstalledApk {
        path: PathBuf::from(path),
        version_code,
    })
}

fn known_identity_reason(token: &str) -> &'static str {
    match token {
        "not_file" => "not_file",
        "bad_size" => "bad_size",
        "bad_version" => "bad_version",
        "no_source" => "no_source",
        "jni" => "jni",
        "no_mtime" => "no_mtime",
        "read_apk" => "read_apk",
        "not_android" => "not_android",
        _ => {
            if token.len() <= 64
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                log::info!("peer apk installed identity rejected ({token})");
            } else {
                log::info!("peer apk installed identity rejected");
            }
            "no_source"
        }
    }
}

fn installed_path_ok(path: &str) -> bool {
    if path.is_empty() || path.len() > 4096 || !path.ends_with(".apk") {
        return false;
    }
    if path.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return false;
    }
    let parsed = Path::new(path);
    if !parsed.is_absolute() {
        return false;
    }
    parsed.components().all(|component| {
        matches!(
            component,
            Component::RootDir | Component::Normal(_) | Component::Prefix(_)
        )
    })
}

fn cached_apk_sha(dir: &Path, installed: &InstalledApk) -> Result<String, &'static str> {
    let stamp = file_stamp(&installed.path, installed.version_code)?;
    if let Some(sha) = read_sha_cache(dir, &stamp) {
        return Ok(sha);
    }
    let sha = hash_regular_apk(&installed.path, stamp.len)?;
    if let Err(code) = write_sha_cache(dir, &stamp, &sha) {
        log::info!("peer apk sha cache write failed ({code})");
    }
    Ok(sha)
}

fn file_stamp(path: &Path, version_code: u64) -> Result<ApkStamp, &'static str> {
    let path_str = path.to_str().ok_or("no_source")?;
    if !installed_path_ok(path_str) {
        return Err("no_source");
    }
    let meta = std::fs::symlink_metadata(path).map_err(|_| "no_source")?;
    if !meta.file_type().is_file() {
        return Err("not_file");
    }
    let len = meta.len();
    if len == 0 || len > APK_MAX_BYTES {
        return Err("bad_size");
    }
    let modified = meta.modified().map_err(|_| "no_mtime")?;
    let dur = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "no_mtime")?;
    Ok(ApkStamp {
        path: path_str.to_owned(),
        mtime_secs: dur.as_secs(),
        mtime_nanos: dur.subsec_nanos(),
        version_code,
        len,
    })
}

fn read_sha_cache(dir: &Path, stamp: &ApkStamp) -> Option<String> {
    let bytes = read_capped(&sha_cache_path(dir), SHA_CACHE_MAX).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    parse_sha_cache(text, stamp)
}

fn parse_sha_cache(text: &str, stamp: &ApkStamp) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()? != "v1" {
        return None;
    }
    let path = lines.next()?;
    let secs: u64 = lines.next()?.parse().ok()?;
    let nanos: u32 = lines.next()?.parse().ok()?;
    let version: u64 = lines.next()?.parse().ok()?;
    let len: u64 = lines.next()?.parse().ok()?;
    let sha = lines.next()?;
    if lines.next().is_some()
        || path != stamp.path
        || secs != stamp.mtime_secs
        || nanos != stamp.mtime_nanos
        || version != stamp.version_code
        || len != stamp.len
        || !sha256_hex(sha)
    {
        return None;
    }
    Some(sha.to_ascii_lowercase())
}

fn write_sha_cache(dir: &Path, stamp: &ApkStamp, sha: &str) -> Result<(), &'static str> {
    if !sha256_hex(sha) || stamp.path.contains('\n') || stamp.path.contains('\r') {
        return Err(UPDATE_INSTALL);
    }
    let body = format!(
        "v1\n{}\n{}\n{}\n{}\n{}\n{}\n",
        stamp.path, stamp.mtime_secs, stamp.mtime_nanos, stamp.version_code, stamp.len, sha
    );
    if body.len() as u64 > SHA_CACHE_MAX {
        return Err(UPDATE_INSTALL);
    }
    write_regular(&sha_cache_path(dir), body.as_bytes())
}

fn sha_cache_path(dir: &Path) -> PathBuf {
    dir.join(SHA_CACHE_FILE)
}

fn hash_regular_apk(path: &Path, len: u64) -> Result<String, &'static str> {
    let file = File::open(path).map_err(|_| "read_apk")?;
    let mut counted = CountRead {
        inner: file.take(len),
        read: 0,
    };
    let sha = sha256_reader_hex(&mut counted).map_err(|_| "read_apk")?;
    if counted.read != len || !sha256_hex(&sha) {
        return Err("read_apk");
    }
    Ok(sha)
}

struct CountRead<R> {
    inner: R,
    read: u64,
}

impl<R: Read> Read for CountRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read = self.read.saturating_add(n as u64);
        Ok(n)
    }
}

fn admit_apk_artifact(url: &str, size: u64, sha256: &str) -> Result<(), &'static str> {
    if size == 0 || size > APK_MAX_BYTES || !sha256_hex(sha256) || !github_release_apk_url(url) {
        return Err(UPDATE_CHECKSUM);
    }
    Ok(())
}

fn github_release_apk_url(url: &str) -> bool {
    if banned_upstream(url) {
        return false;
    }
    match fork_release_ref(url) {
        Some(ForkRef::Asset { tag, name }) => {
            device_tag_arch(&tag).is_some() && plain_apk_name(&name)
        }
        _ => false,
    }
}

pub(crate) fn release_tag_arg_ok(arg: &str) -> bool {
    match fork_release_ref(arg) {
        Some(ForkRef::Tag(tag) | ForkRef::Asset { tag, .. }) => device_tag_arch(&tag).is_some(),
        None => false,
    }
}

fn banned_upstream(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    url.contains("://api.rustdesk.com")
        || url.contains("://github.com/rustdesk/")
        || url.contains("://www.github.com/rustdesk/")
}

fn fork_fetch_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed
            .query()
            .map(|q| q.contains("rustdesk.com"))
            .unwrap_or(false)
    {
        return false;
    }
    let host = parsed.host_str().unwrap_or("");
    let path = parsed.path();
    let prefix = format!("/repos/{FORK_OWNER}/{FORK_REPO}/releases");
    let download = format!("/{FORK_OWNER}/{FORK_REPO}/releases/download/");
    (host == "api.github.com" && path.starts_with(&prefix))
        || (host == "github.com" && path.starts_with(&download))
}

enum ForkRef {
    Tag(String),
    Asset { tag: String, name: String },
}

fn fork_release_ref(url: &str) -> Option<ForkRef> {
    if banned_upstream(url) || !url.starts_with("https://github.com/") {
        return None;
    }
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let mut segments = parsed.path_segments()?;
    if segments.next()? != FORK_OWNER
        || segments.next()? != FORK_REPO
        || segments.next()? != "releases"
    {
        return None;
    }
    match segments.next()? {
        "tag" => {
            let tag = segments.next()?;
            if segments.next().is_some() || device_tag_arch(tag).is_none() {
                return None;
            }
            Some(ForkRef::Tag(tag.to_owned()))
        }
        "download" => {
            let tag = segments.next()?;
            let name = segments.next()?;
            if segments.next().is_some() || !plain_apk_name(name) || device_tag_arch(tag).is_none()
            {
                return None;
            }
            Some(ForkRef::Asset {
                tag: tag.to_owned(),
                name: name.to_owned(),
            })
        }
        _ => None,
    }
}

fn device_tag_arch(tag: &str) -> Option<&str> {
    let rest = tag.strip_prefix("device-")?;
    let (mid, date) = rest.rsplit_once('-')?;
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (name, arch) = mid.rsplit_once('-')?;
    if name.is_empty() || !token_ok(name) || !matches!(arch, "arm64" | "arm" | "x86_64" | "x86") {
        return None;
    }
    Some(arch)
}

fn token_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 180
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[derive(Debug, PartialEq)]
struct ReleaseChoice {
    tag: String,
    name: String,
    size: u64,
    digest_sha: Option<String>,
    has_sums: bool,
}

fn choose_release(
    bytes: &[u8],
    arch: &str,
    only_tag: Option<&str>,
    only_name: Option<&str>,
) -> Result<ReleaseChoice, &'static str> {
    if bytes.len() as u64 > RELEASES_JSON_MAX {
        return Err(UPDATE_CHECKSUM);
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| UPDATE_NETWORK)?;
    let list: Vec<&serde_json::Value> = if let Some(items) = value.as_array() {
        items.iter().collect()
    } else if value.is_object() {
        vec![&value]
    } else {
        return Err(UPDATE_NETWORK);
    };
    if list.len() > 20 {
        return Err(UPDATE_CHECKSUM);
    }
    for item in list {
        if let Some(choice) = release_choice(item, arch, only_tag, only_name)? {
            return Ok(choice);
        }
    }
    Err(semantic_action::UNSUPPORTED)
}

fn release_choice(
    item: &serde_json::Value,
    arch: &str,
    only_tag: Option<&str>,
    only_name: Option<&str>,
) -> Result<Option<ReleaseChoice>, &'static str> {
    let tag = item.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    if device_tag_arch(tag) != Some(arch) {
        return Ok(None);
    }
    if let Some(want) = only_tag {
        if tag != want {
            return Ok(None);
        }
    }
    let assets = item
        .get("assets")
        .and_then(|v| v.as_array())
        .ok_or(UPDATE_NETWORK)?;
    if assets.len() > 32 {
        return Err(UPDATE_CHECKSUM);
    }
    let mut apk = None;
    let mut has_sums = false;
    for asset in assets {
        let name = asset.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name == "SHA256SUMS" {
            has_sums = true;
            continue;
        }
        if !plain_apk_name(name) {
            continue;
        }
        if apk.is_some() {
            return Err(UPDATE_CHECKSUM);
        }
        let size = asset.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        let digest = asset.get("digest").and_then(|v| v.as_str()).unwrap_or("");
        apk = Some((name.to_owned(), size, digest.to_owned()));
    }
    let Some((name, size, digest)) = apk else {
        return Ok(None);
    };
    if let Some(want) = only_name {
        if name != want {
            return Err(UPDATE_CHECKSUM);
        }
    }
    if size == 0 || size > APK_MAX_BYTES {
        return Err(UPDATE_CHECKSUM);
    }
    Ok(Some(ReleaseChoice {
        tag: tag.to_owned(),
        name,
        size,
        digest_sha: digest_sha(&digest),
        has_sums,
    }))
}

fn digest_sha(digest: &str) -> Option<String> {
    let rest = digest
        .strip_prefix("sha256:")
        .or_else(|| digest.strip_prefix("SHA256:"))?;
    if sha256_hex(rest) {
        Some(rest.to_ascii_lowercase())
    } else {
        None
    }
}

fn lock_sha(choice: &ReleaseChoice, sums_text: Option<&str>) -> Result<Admitted, &'static str> {
    let url = format!(
        "https://github.com/{FORK_OWNER}/{FORK_REPO}/releases/download/{}/{}",
        choice.tag, choice.name
    );
    let sums_sha = if choice.has_sums {
        match sums_text {
            Some(text) => {
                if text.len() as u64 > SUMS_MAX_BYTES {
                    return Err(UPDATE_CHECKSUM);
                }
                match sha_from_sums(text, &choice.name)? {
                    Some(from_sums) => {
                        if let Some(digest) = &choice.digest_sha {
                            if !digest.eq_ignore_ascii_case(&from_sums) {
                                return Err(UPDATE_CHECKSUM);
                            }
                        }
                        Some(from_sums)
                    }
                    None => {
                        log::info!(
                            "peer apk SHA256SUMS has no entry for {}; continue cover-install",
                            choice.name
                        );
                        None
                    }
                }
            }
            None => {
                log::info!("peer apk SHA256SUMS unavailable; continue cover-install");
                None
            }
        }
    } else {
        log::info!("peer apk release has no SHA256SUMS; continue cover-install");
        None
    };
    let sha = if let Some(sha) = &sums_sha {
        sha.clone()
    } else if let Some(digest) = &choice.digest_sha {
        digest.clone()
    } else {
        return Err(UPDATE_CHECKSUM);
    };
    admit_apk_artifact(&url, choice.size, &sha)?;
    Ok(Admitted {
        url,
        size: choice.size,
        sha256: sha,
        sums_sha,
    })
}

fn sha_from_sums(text: &str, filename: &str) -> Result<Option<String>, &'static str> {
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(sha) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            return Err(UPDATE_CHECKSUM);
        };
        if parts.next().is_some() {
            return Err(UPDATE_CHECKSUM);
        }
        if name != filename {
            continue;
        }
        if !sha256_hex(sha) {
            return Err(UPDATE_CHECKSUM);
        }
        let sha = sha.to_ascii_lowercase();
        if let Some(prev) = &found {
            if prev != &sha {
                return Err(UPDATE_CHECKSUM);
            }
        }
        found = Some(sha);
    }
    Ok(found)
}

fn finish_committed_or_fail(dir: &Path, cover_code: &str) -> &'static str {
    // A commit that the OS accepts kills this process. Never report ok here.
    if cover_code == "committed" || cover_code == semantic_action::OK {
        return COMMITTED;
    }
    let _ = remove_snapshot(dir);
    let code = known_update_code(cover_code);
    if code == semantic_action::OK {
        COMMITTED
    } else {
        code
    }
}

fn write_snapshot(dir: &Path, id: &IdMaterial) -> Result<(), &'static str> {
    let _ = remove_regular(&verdict_path(dir));
    if id.device_id.is_empty()
        || id.device_id.len() > DEVICE_ID_MAX
        || id.enc_id.len() > ENC_ID_MAX
        || id.plain_id.len() > DEVICE_ID_MAX
        || field_has_break(&id.device_id)
        || field_has_break(&id.enc_id)
        || field_has_break(&id.plain_id)
    {
        return Err(UPDATE_INSTALL);
    }
    let body = format!("v1\n{}\n{}\n{}\n", id.device_id, id.enc_id, id.plain_id);
    if body.len() as u64 > SNAPSHOT_MAX_BYTES {
        return Err(UPDATE_INSTALL);
    }
    write_regular(&snap_path(dir), body.as_bytes())
}

fn settle_dir(dir: &Path) -> Result<Option<&'static str>, &'static str> {
    let Some(before) = read_snapshot(dir)? else {
        return Ok(None);
    };
    let code = match read_capped(&config_file(dir), ID_FILE_MAX_BYTES) {
        Ok(bytes) => id_preserved_on_disk(&before, &bytes),
        Err(_) => UPDATE_ID_CHANGED,
    };
    let _ = remove_snapshot(dir);
    write_regular(&verdict_path(dir), code.as_bytes())?;
    Ok(Some(code))
}

fn id_preserved_on_disk(before: &IdMaterial, file: &[u8]) -> &'static str {
    if before.device_id.is_empty() || file.len() as u64 > ID_FILE_MAX_BYTES {
        return UPDATE_ID_CHANGED;
    }
    let enc = toml_field(file, "enc_id").unwrap_or_default();
    let plain = toml_field(file, "id").unwrap_or_default();
    if !before.enc_id.is_empty() {
        return if enc == before.enc_id {
            semantic_action::OK
        } else {
            UPDATE_ID_CHANGED
        };
    }
    if plain == before.device_id {
        semantic_action::OK
    } else {
        UPDATE_ID_CHANGED
    }
}

fn read_snapshot(dir: &Path) -> Result<Option<IdMaterial>, &'static str> {
    let path = snap_path(dir);
    match read_capped(&path, SNAPSHOT_MAX_BYTES) {
        Err(_) if !path.exists() => Ok(None),
        Err(_) => {
            let _ = remove_snapshot(dir);
            write_regular(&verdict_path(dir), UPDATE_ID_CHANGED.as_bytes())?;
            Ok(None)
        }
        Ok(bytes) => match parse_snapshot(&bytes) {
            Some(id) => Ok(Some(id)),
            None => {
                let _ = remove_snapshot(dir);
                write_regular(&verdict_path(dir), UPDATE_ID_CHANGED.as_bytes())?;
                Ok(None)
            }
        },
    }
}

fn parse_snapshot(bytes: &[u8]) -> Option<IdMaterial> {
    if bytes.len() as u64 > SNAPSHOT_MAX_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != "v1" {
        return None;
    }
    let device_id = lines.next()?.to_owned();
    let enc_id = lines.next()?.to_owned();
    let plain_id = lines.next()?.to_owned();
    if lines.next().is_some()
        || device_id.is_empty()
        || device_id.len() > DEVICE_ID_MAX
        || enc_id.len() > ENC_ID_MAX
        || plain_id.len() > DEVICE_ID_MAX
    {
        return None;
    }
    Some(IdMaterial {
        device_id,
        enc_id,
        plain_id,
    })
}

fn peek_verdict(dir: &Path) -> Option<&'static str> {
    let bytes = read_capped(&verdict_path(dir), 64).ok()?;
    match std::str::from_utf8(&bytes).ok()?.trim() {
        semantic_action::OK => Some(semantic_action::OK),
        _ => Some(UPDATE_ID_CHANGED),
    }
}

fn take_verdict(dir: &Path) -> Option<&'static str> {
    let code = peek_verdict(dir)?;
    let _ = remove_regular(&verdict_path(dir));
    Some(code)
}

fn field_has_break(value: &str) -> bool {
    value.chars().any(|c| c == '\n' || c == '\r')
}

fn snap_path(dir: &Path) -> PathBuf {
    dir.join(SNAPSHOT_FILE)
}

fn verdict_path(dir: &Path) -> PathBuf {
    dir.join(VERDICT_FILE)
}

fn config_file(dir: &Path) -> PathBuf {
    dir.join("RustDesk.toml")
}

fn write_regular(path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| UPDATE_INSTALL)?;
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_file() {
            return Err(UPDATE_INSTALL);
        }
        std::fs::remove_file(path).map_err(|_| UPDATE_INSTALL)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| UPDATE_INSTALL)?;
    file.write_all(bytes).map_err(|_| UPDATE_INSTALL)?;
    file.flush().map_err(|_| UPDATE_INSTALL)?;
    Ok(())
}

fn remove_snapshot(dir: &Path) -> Result<(), ()> {
    remove_regular(&snap_path(dir))
}

#[cfg(target_os = "android")]
fn config_dir() -> Option<PathBuf> {
    hbb_common::config::Config::file()
        .parent()
        .map(|p| p.to_path_buf())
}

pub(crate) fn settle_persisted_snapshot() {
    #[cfg(target_os = "android")]
    {
        let Some(dir) = config_dir() else {
            return;
        };
        match settle_dir(&dir) {
            Ok(Some(code)) => log::info!("peer apk post-replace -> {code}"),
            Ok(None) => {}
            Err(code) => log::error!("peer apk post-replace -> {code}"),
        }
    }
}

pub(crate) fn reconnect_result() -> Option<base::message_proto::Message> {
    #[cfg(not(target_os = "android"))]
    {
        None
    }
    #[cfg(target_os = "android")]
    {
        let dir = config_dir()?;
        let code = peek_verdict(&dir)?;
        Some(semantic_action::result_message(
            semantic_action::CHECK_UPDATE,
            code,
        ))
    }
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
    const RELEASE_APK: &str = "https://github.com/1304257658/mobile-cu-bridge/releases/download/device-e1e2e5e8e912-arm64-20261103/mobile-cu-bridge-e1e2e5e8e912-arm64-release.apk";
    const DEVICE_SHA: &str = "9b235b5e831ab748f790ca1a6d18b02084ee9d59fe96fdd2c73105f73cc14d17";

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
                "https://github.com/rustdesk/rustdesk/releases/download/1.4.6/a.apk",
                8,
                SHA_RUSTDESK
            ),
            Err(UPDATE_CHECKSUM)
        );
        assert_eq!(
            admit_apk_artifact("https://api.rustdesk.com/version/latest", 8, SHA_RUSTDESK),
            Err(UPDATE_CHECKSUM)
        );
        assert_eq!(
            admit_apk_artifact("https://example.com/a.apk", 8, SHA_RUSTDESK),
            Err(UPDATE_CHECKSUM)
        );
        assert!(!release_tag_arg_ok(
            "https://github.com/rustdesk/rustdesk/releases/tag/1.4.6"
        ));
        assert!(!release_tag_arg_ok(
            "https://api.rustdesk.com/version/latest"
        ));
        assert!(release_tag_arg_ok(
            "https://github.com/1304257658/mobile-cu-bridge/releases/tag/device-e1e2e5e8e912-arm64-20261103"
        ));
        assert!(release_tag_arg_ok(RELEASE_APK));

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
            finish_committed_or_fail(std::path::Path::new("."), "committed"),
            COMMITTED
        );
        assert_ne!(COMMITTED, semantic_action::OK);
        assert_ne!(
            finish_committed_or_fail(std::path::Path::new("unused-fail"), "install"),
            semantic_action::OK
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
    fn fork_release_is_the_only_source() {
        let list = format!(
            r#"[{{"tag_name":"host-e2-linux-x86_64-20261103","assets":[{{"name":"host.tar.gz","size":10}}]}},{{"tag_name":"device-e1e2e5e8e912-arm64-20261103","assets":[{{"name":"mobile-cu-bridge-e1e2e5e8e912-arm64-release.apk","size":31,"digest":"sha256:{DEVICE_SHA}"}},{{"name":"SHA256SUMS","size":80}}]}}]"#
        );
        let choice = choose_release(list.as_bytes(), "arm64", None, None).unwrap();
        assert_eq!(choice.tag, "device-e1e2e5e8e912-arm64-20261103");
        assert!(choice.name.ends_with(".apk"));
        assert_eq!(choice.has_sums, true);
        let sums = format!("{DEVICE_SHA}  mobile-cu-bridge-e1e2e5e8e912-arm64-release.apk\n");
        let admitted = lock_sha(&choice, Some(&sums)).unwrap();
        assert!(admitted.url.starts_with(
            "https://github.com/1304257658/mobile-cu-bridge/releases/download/device-e1e2e5e8e912-arm64-20261103/"
        ));
        assert!(!admitted.url.contains("rustdesk/rustdesk"));
        assert!(!admitted.url.contains("api.rustdesk.com"));
        assert_eq!(admitted.sha256, DEVICE_SHA);
        assert_eq!(admitted.sums_sha.as_deref(), Some(DEVICE_SHA));
        assert_eq!(
            lock_sha(&choice, Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  mobile-cu-bridge-e1e2e5e8e912-arm64-release.apk\n")),
            Err(UPDATE_CHECKSUM)
        );
        assert_eq!(
            choose_release(list.as_bytes(), "x86_64", None, None),
            Err(semantic_action::UNSUPPORTED)
        );
        assert_eq!(
            choose_release(br#"{"message":"not-a-release"}"#, "arm64", None, None),
            Err(semantic_action::UNSUPPORTED)
        );
        assert!(banned_upstream("https://api.rustdesk.com/version/latest"));
        assert!(banned_upstream(
            "https://github.com/rustdesk/rustdesk/releases/tag/1.4.6"
        ));
        assert!(!banned_upstream(RELEASE_APK));
        assert_eq!(android_update_arch() == "unknown", false);
        #[cfg(not(target_os = "android"))]
        {
            assert_eq!(execute("").code, semantic_action::UNSUPPORTED);
            assert_eq!(
                execute("https://github.com/rustdesk/rustdesk/releases/tag/1.4.6").code,
                semantic_action::UNSUPPORTED
            );
            assert_eq!(
                execute("https://api.rustdesk.com/version/latest").code,
                semantic_action::UNSUPPORTED
            );
        }
    }

    #[test]
    fn post_replace_reports_the_id_from_disk() {
        let dir = std::env::temp_dir().join(format!(
            "peer-apk-id-{}-{}",
            std::process::id(),
            hbb_common::rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let before = material("enc-v1");
        write_snapshot(&dir, &before).unwrap();
        assert!(snap_path(&dir).is_file());
        assert_eq!(finish_committed_or_fail(&dir, "committed"), COMMITTED);
        assert!(snap_path(&dir).is_file(), "commit must keep the snapshot");
        assert_ne!(COMMITTED, semantic_action::OK);

        let kept = b"id = \"\"\nenc_id = \"enc-v1\"\n";
        std::fs::write(config_file(&dir), kept).unwrap();
        assert_eq!(settle_dir(&dir).unwrap(), Some(semantic_action::OK));
        assert!(!snap_path(&dir).exists());
        assert_eq!(take_verdict(&dir), Some(semantic_action::OK));
        assert_eq!(take_verdict(&dir), None);

        write_snapshot(&dir, &before).unwrap();
        std::fs::write(config_file(&dir), b"id = \"999\"\nenc_id = \"enc-other\"\n").unwrap();
        assert_eq!(settle_dir(&dir).unwrap(), Some(UPDATE_ID_CHANGED));
        assert_eq!(take_verdict(&dir), Some(UPDATE_ID_CHANGED));
        assert!(!semantic_action::is_peer_success(UPDATE_ID_CHANGED));

        write_snapshot(&dir, &before).unwrap();
        let _ = std::fs::remove_file(config_file(&dir));
        assert_eq!(settle_dir(&dir).unwrap(), Some(UPDATE_ID_CHANGED));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn release_choice(digest: Option<&str>, has_sums: bool) -> ReleaseChoice {
        ReleaseChoice {
            tag: "device-e1e2e5e8e912-arm64-20261103".to_owned(),
            name: "mobile-cu-bridge-e1e2e5e8e912-arm64-release.apk".to_owned(),
            size: 31,
            digest_sha: digest.map(str::to_owned),
            has_sums,
        }
    }

    fn apply_skip(skip: bool, install: impl FnOnce() -> &'static str) -> &'static str {
        if skip {
            semantic_action::OK
        } else {
            install()
        }
    }

    fn sums_line(sha: &str) -> String {
        format!("{sha}  mobile-cu-bridge-e1e2e5e8e912-arm64-release.apk\n")
    }

    #[test]
    fn same_sha_skips_cover_install() {
        let sums = sums_line(DEVICE_SHA);
        let from_sums = lock_sha(&release_choice(Some(DEVICE_SHA), true), Some(&sums)).unwrap();
        assert_eq!(from_sums.sums_sha.as_deref(), Some(DEVICE_SHA));
        assert!(!from_sums.url.contains("rustdesk/rustdesk"));
        assert!(!from_sums.url.contains("api.rustdesk.com"));
        let code = apply_skip(
            skip_same_sums(Ok(&from_sums.sha256), from_sums.sums_sha.as_deref()),
            || panic!("download and cover-install"),
        );
        assert_eq!(code, semantic_action::OK);
        assert!(semantic_action::is_peer_success(code));
        assert_ne!(code, COMMITTED);
        let reply = semantic_action::result_message_with_detail(
            semantic_action::CHECK_UPDATE,
            code,
            semantic_action::CHECK_UPDATE_ALREADY_LATEST,
        );
        match reply.union {
            Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                assert_eq!(result.code, semantic_action::OK);
                assert_eq!(result.detail, "already_latest");
            }
            other => panic!("expected SemanticActionResult, got {other:?}"),
        }
        assert!(skip_same_sums(
            Ok(&DEVICE_SHA.to_ascii_uppercase()),
            Some(&DEVICE_SHA.to_ascii_lowercase()),
        ));

        let mut installs = 0u32;
        let other = sums_line(SHA_RUSTDESK);
        let different = lock_sha(&release_choice(None, true), Some(&other)).unwrap();
        assert!(different.sums_sha.is_some());
        let code = apply_skip(
            skip_same_sums(Ok(DEVICE_SHA), different.sums_sha.as_deref()),
            || {
                installs += 1;
                COMMITTED
            },
        );
        assert_eq!(installs, 1);
        assert_eq!(code, COMMITTED);

        let digest_only = lock_sha(&release_choice(Some(DEVICE_SHA), false), None).unwrap();
        assert!(digest_only.sums_sha.is_none());
        assert_eq!(digest_only.sha256, DEVICE_SHA);
        installs = 0;
        let code = apply_skip(
            skip_same_sums(Ok(DEVICE_SHA), digest_only.sums_sha.as_deref()),
            || {
                installs += 1;
                COMMITTED
            },
        );
        assert_eq!(installs, 1);
        assert_eq!(code, COMMITTED);

        let missing_name = format!("{DEVICE_SHA}  other.apk\n");
        let no_entry = lock_sha(
            &release_choice(Some(SHA_RUSTDESK), true),
            Some(&missing_name),
        )
        .unwrap();
        assert!(no_entry.sums_sha.is_none());
        assert_eq!(no_entry.sha256, SHA_RUSTDESK);
        installs = 0;
        let code = apply_skip(
            skip_same_sums(Ok(SHA_RUSTDESK), no_entry.sums_sha.as_deref()),
            || {
                installs += 1;
                COMMITTED
            },
        );
        assert_eq!(installs, 1);
        assert_eq!(code, COMMITTED);
        assert_eq!(
            lock_sha(&release_choice(None, true), Some(&missing_name)),
            Err(UPDATE_CHECKSUM)
        );
        let unavailable = lock_sha(&release_choice(Some(DEVICE_SHA), true), None).unwrap();
        assert!(unavailable.sums_sha.is_none());
        installs = 0;
        let code = apply_skip(
            skip_same_sums(Ok(DEVICE_SHA), unavailable.sums_sha.as_deref()),
            || {
                installs += 1;
                COMMITTED
            },
        );
        assert_eq!(installs, 1);

        installs = 0;
        let code = apply_skip(skip_same_sums(Err("no_source"), Some(DEVICE_SHA)), || {
            installs += 1;
            COMMITTED
        });
        assert_eq!(installs, 1);
        assert_eq!(code, COMMITTED);
        let code = apply_skip(skip_same_sums(Err("read_apk"), Some(DEVICE_SHA)), || {
            installs += 1;
            COMMITTED
        });
        assert_eq!(installs, 2);
        assert_eq!(code, COMMITTED);

        assert_eq!(
            lock_sha(&release_choice(None, false), None),
            Err(UPDATE_CHECKSUM)
        );
        assert!(!skip_same_sums(Ok("zz"), Some(DEVICE_SHA)));
        assert!(!skip_same_sums(Ok(DEVICE_SHA), Some("")));
    }

    #[test]
    fn installed_sha_is_cached_until_the_apk_changes() {
        let dir = std::env::temp_dir().join(format!(
            "peer-apk-sha-{}-{}",
            std::process::id(),
            hbb_common::rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("base.apk");
        std::fs::write(&path, b"rustdesk").unwrap();
        let installed = InstalledApk {
            path: path.clone(),
            version_code: 7,
        };
        let stamp = file_stamp(&path, 7).unwrap();
        let sentinel = "ab".repeat(32);
        write_sha_cache(&dir, &stamp, &sentinel).unwrap();
        assert_eq!(cached_apk_sha(&dir, &installed).unwrap(), sentinel);

        let stamped = ApkStamp {
            path: stamp.path.clone(),
            mtime_secs: stamp.mtime_secs,
            mtime_nanos: stamp.mtime_nanos,
            version_code: stamp.version_code,
            len: stamp.len + 1,
        };
        write_sha_cache(&dir, &stamped, &sentinel).unwrap();
        assert_eq!(cached_apk_sha(&dir, &installed).unwrap(), SHA_RUSTDESK);

        let other = dir.join("other.apk");
        std::fs::write(&other, b"rustdesk").unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&other)
            .unwrap();
        let modified = std::fs::symlink_metadata(&path)
            .unwrap()
            .modified()
            .unwrap();
        file.set_modified(modified).unwrap();
        drop(file);
        let stamp_other = file_stamp(&other, 7).unwrap();
        assert_eq!(stamp_other.mtime_secs, stamp.mtime_secs);
        assert_eq!(stamp_other.mtime_nanos, stamp.mtime_nanos);
        assert_eq!(stamp_other.len, stamp.len);
        assert_ne!(stamp_other.path, stamp.path);
        write_sha_cache(&dir, &stamp, &sentinel).unwrap();
        let other_apk = InstalledApk {
            path: other.clone(),
            version_code: 7,
        };
        assert_eq!(cached_apk_sha(&dir, &other_apk).unwrap(), SHA_RUSTDESK);

        let bumped = InstalledApk {
            path: path.clone(),
            version_code: 8,
        };
        assert_eq!(cached_apk_sha(&dir, &bumped).unwrap(), SHA_RUSTDESK);
        std::fs::write(&path, b"changed!").unwrap();
        let moved = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let later = std::fs::symlink_metadata(&path)
            .unwrap()
            .modified()
            .unwrap()
            + Duration::from_secs(2);
        moved.set_modified(later).unwrap();
        drop(moved);
        let again = cached_apk_sha(&dir, &bumped).unwrap();
        assert_ne!(again, SHA_RUSTDESK);
        assert_ne!(again, sentinel);
        assert_eq!(cached_apk_sha(&dir, &bumped).unwrap(), again);
        let _ = std::fs::remove_dir_all(&dir);
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
