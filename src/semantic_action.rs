//! Semantic shortcuts sent as a session control message.
//!
//! The controller admits the action and sends [`SemanticAction`]. The
//! controlled Android peer executes it (accessibility global action, a
//! package intent, or the focused field). The peer answers with
//! [`SemanticActionResult`]. Host input injection, the clipboard, and
//! pointer hits are not this path.
//!
//! `open_app` starts a launcher intent and is `ok` only after that package
//! is in the foreground. It does not tap the desktop.
//!
//! `list_apps` returns MAIN/LAUNCHER packages only, 40 per page, and stops at
//! 256. An empty launcher set is `ok` with an empty item list.
//!
//! `check_update` downloads the Android peer APK from a GitHub Release,
//! checks sha256, and cover-installs it only when the signing cert matches.
//! It does not uninstall. `network`, `checksum`, `signature`, `install`, and
//! `id_changed` are peer failures, not `inject_failed`.
//!
//! `snapshot` reads the foreground accessibility tree and returns refs.
//! `click` uses those refs. Limits live in `a11y_snapshot_caps.json`.
//! `snapshot_failed` and `ref_stale` are peer failures, not `inject_failed`.
//! A ref expires after its TTL. A later snapshot drops the previous refs.

use base::message_proto::{Message, SemanticAction, SemanticActionResult};
use hbb_common::{log, protobuf::Message as _};

pub const SYS_BACK: &str = "sys.back";
pub const SYS_HOME: &str = "sys.home";
pub const SYS_RECENTS: &str = "sys.recents";
pub const OPEN_APP: &str = "open_app";
pub const LIST_APPS: &str = "list_apps";
pub const IME: &str = "ime";
pub const SET_TEXT: &str = "set_text";
pub const CHECK_UPDATE: &str = "check_update";
pub const SNAPSHOT: &str = "snapshot";
pub const CLICK: &str = "click";

/// Peer snapshot / click failures. Passed through by [`normalize_peer_code`].
/// They are not [`INJECT_FAILED`]. `ref_stale` is an unknown or expired ref.
pub const SNAPSHOT_FAILED: &str = "snapshot_failed";
pub const REF_STALE: &str = "ref_stale";

/// Peer APK update failures. Passed through by [`normalize_peer_code`].
/// They are not [`INJECT_FAILED`]. `id_changed` means the device id or
/// `enc_id` differed after cover-install.
pub const UPDATE_NETWORK: &str = "network";
pub const UPDATE_CHECKSUM: &str = "checksum";
pub const UPDATE_SIGNATURE: &str = "signature";
pub const UPDATE_INSTALL: &str = "install";
pub const UPDATE_ID_CHANGED: &str = "id_changed";

pub const KINDS: [&str; 10] = [
    SYS_BACK,
    SYS_HOME,
    SYS_RECENTS,
    OPEN_APP,
    LIST_APPS,
    IME,
    SET_TEXT,
    CHECK_UPDATE,
    SNAPSHOT,
    CLICK,
];

/// `list_apps` page size and the most items a peer will keep.
pub const LIST_APPS_PAGE: usize = 40;
pub const LIST_APPS_CAP: usize = 256;
pub const LIST_APPS_LABEL_MAX: usize = 512;
/// Bytes. A larger `list_apps` detail is `inject_failed`.
pub const LIST_APPS_DETAIL_MAX: usize = 64 * 1024;

/// Hard bound for the connection task waiting on `list_apps`.
pub const LIST_APPS_BLOCKING_TIMEOUT_MS: u64 = 5_000;

/// Enqueue acknowledgement from `session_semantic_action`, and the peer
/// completion code from `session_semantic_result`. The enqueue value is not
/// peer success. Success is only this string when `session_semantic_result`
/// returns it.
pub const OK: &str = "ok";
pub const NO_SESSION: &str = "no_session";
pub const UNSUPPORTED: &str = "unsupported";
pub const INJECT_FAILED: &str = "inject_failed";

/// `session_semantic_result` when the peer has not answered. Not success.
pub const RESULT_MISSING: &str = "inject_failed: no peer result";

/// Second `session_semantic_action` while the previous one has no peer result.
/// The call is not sent.
pub const IN_FLIGHT: &str = "inject_failed: action in flight";

/// Bytes, not chars. Larger `set_text` payloads are refused before they are copied
/// into a session message.
pub const SET_TEXT_MAX: usize = 16 * 1024;
pub const OPEN_APP_MAX: usize = 256;

/// Peer `open_app` failures. Passed through unchanged by [`normalize_peer_code`].
/// Each is at most 32 bytes. They are not [`INJECT_FAILED`].
pub const NO_LAUNCH_INTENT: &str = "no_launch_intent";
pub const START_THREW: &str = "start_threw";
pub const FG_TIMEOUT: &str = "fg_timeout";

/// Hard bound for the connection task waiting on `open_app`. The foreground
/// poll stops at 2.5s; this also covers a hung accessibility call.
pub const OPEN_APP_BLOCKING_TIMEOUT_MS: u64 = 5_000;

/// What the controller knows before it tries to enqueue.
#[derive(Clone, Copy)]
pub struct Gate<'a> {
    pub has_sender: bool,
    pub remote_desktop: bool,
    pub input_allowed: bool,
    pub platform: &'a str,
}

/// Admit or refuse a semantic action. Does not send anything.
pub fn gate(g: &Gate<'_>, kind: &str, arg: &str) -> &'static str {
    if !g.has_sender {
        return NO_SESSION;
    }
    if !g.remote_desktop {
        return UNSUPPORTED;
    }
    if !g.input_allowed {
        return INJECT_FAILED;
    }
    let platform = g.platform.trim();
    if !platform.is_empty() && !platform.eq_ignore_ascii_case("android") {
        return UNSUPPORTED;
    }
    validate(kind, arg)
}

pub fn validate(kind: &str, arg: &str) -> &'static str {
    match kind {
        SYS_BACK | SYS_HOME | SYS_RECENTS => {
            if arg.is_empty() {
                OK
            } else {
                UNSUPPORTED
            }
        }
        OPEN_APP => {
            if package_spec_ok(arg) {
                OK
            } else {
                UNSUPPORTED
            }
        }
        LIST_APPS => {
            if list_apps_offset(arg).is_some() {
                OK
            } else {
                UNSUPPORTED
            }
        }
        IME => match arg {
            "show" | "hide" => OK,
            _ => UNSUPPORTED,
        },
        SET_TEXT => {
            if arg.len() > SET_TEXT_MAX {
                log::error!(
                    "semantic set_text refused: {} bytes exceeds {}",
                    arg.len(),
                    SET_TEXT_MAX
                );
                UNSUPPORTED
            } else {
                OK
            }
        }
        CHECK_UPDATE => {
            if arg.is_empty() || release_page_arg_ok(arg) {
                OK
            } else {
                UNSUPPORTED
            }
        }
        SNAPSHOT => {
            if arg.is_empty() {
                OK
            } else {
                UNSUPPORTED
            }
        }
        CLICK => crate::a11y_snapshot::click_arg_verdict(arg),
        _ => UNSUPPORTED,
    }
}

pub fn action_message(kind: &str, arg: &str) -> Message {
    let mut msg = Message::new();
    msg.set_semantic_action(SemanticAction {
        kind: kind.to_owned(),
        arg: arg.to_owned(),
        ..Default::default()
    });
    msg
}

pub fn result_message(kind: &str, code: &str) -> Message {
    let mut msg = Message::new();
    msg.set_semantic_action_result(SemanticActionResult {
        kind: kind.to_owned(),
        code: normalize_peer_code(code).to_owned(),
        ..Default::default()
    });
    msg
}

/// Like [`result_message`], plus a `list_apps` detail payload.
///
/// Any other kind drops the detail. `open_app` failure codes are unchanged.
pub fn result_message_with_detail(kind: &str, code: &str, detail: &str) -> Message {
    let normalized = normalize_peer_code(code);
    let (code, detail) = if kind == LIST_APPS && normalized == OK {
        match normalize_list_apps_detail(detail) {
            Some(detail) => (OK, detail),
            None => {
                list_apps_payload_rejected();
                (INJECT_FAILED, String::new())
            }
        }
    } else if kind == SNAPSHOT && normalized == OK {
        match crate::a11y_snapshot::accept_snapshot_detail(detail) {
            Ok(detail) => (OK, detail),
            Err(()) => (SNAPSHOT_FAILED, String::new()),
        }
    } else {
        (normalized, String::new())
    };
    let mut msg = Message::new();
    msg.set_semantic_action_result(SemanticActionResult {
        kind: kind.to_owned(),
        code: code.to_owned(),
        detail,
        ..Default::default()
    });
    msg
}

/// Stored completion, or [`RESULT_MISSING`] when the slot is empty.
///
/// An empty string is not success. Callers wait until this is `ok`,
/// `unsupported`, or `inject_failed`.
pub fn require_peer_result(stored: &str) -> &str {
    if stored.trim().is_empty() {
        RESULT_MISSING
    } else {
        stored
    }
}

/// True only for a peer completion of `ok`.
///
/// [`OK`] from `session_semantic_action` is the enqueue acknowledgement.
/// Pass the value from `session_semantic_result`, not the enqueue return.
pub fn is_peer_success(result: &str) -> bool {
    result == OK
}

/// One semantic action at a time. The next `begin` waits until `finish`.
#[derive(Default)]
pub struct Flight {
    inflight: bool,
    code: String,
    detail: String,
}

impl Flight {
    /// Reserve the single slot. The previous completion is cleared.
    pub fn begin(&mut self) -> Result<(), &'static str> {
        if self.inflight {
            return Err(IN_FLIGHT);
        }
        self.inflight = true;
        self.code.clear();
        self.detail.clear();
        Ok(())
    }

    /// The send did not happen. Another action may start.
    pub fn abort(&mut self) {
        self.inflight = false;
    }

    /// Peer completion. Another action may start. Clears any previous payload.
    pub fn finish(&mut self, code: &str) {
        self.code = normalize_peer_code(code).to_owned();
        self.detail.clear();
        self.inflight = false;
    }

    fn finish_list(&mut self, code: &str, detail: &str) {
        self.finish(code);
        if self.code != OK {
            return;
        }
        match normalize_list_apps_detail(detail) {
            Some(detail) => self.detail = detail,
            None => {
                list_apps_payload_rejected();
                self.code = INJECT_FAILED.to_owned();
                self.detail.clear();
            }
        }
    }

    fn finish_snapshot(&mut self, code: &str, detail: &str) {
        self.finish(code);
        if self.code != OK {
            return;
        }
        match crate::a11y_snapshot::accept_snapshot_detail(detail) {
            Ok(detail) => self.detail = detail,
            Err(()) => {
                self.code = SNAPSHOT_FAILED.to_owned();
                self.detail.clear();
            }
        }
    }

    pub fn result(&self) -> &str {
        require_peer_result(&self.code)
    }

    /// `list_apps` JSON, or empty when this result has no payload.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Record a peer completion. `list_apps` keeps its detail. Every other kind
/// stores only the code, so an `open_app` failure cannot be rewritten by a
/// payload and is not collapsed into [`INJECT_FAILED`].
pub fn store_peer_result(flight: &mut Flight, kind: &str, code: &str, detail: &str) {
    if kind == LIST_APPS {
        flight.finish_list(code, detail);
    } else if kind == SNAPSHOT {
        flight.finish_snapshot(code, detail);
    } else {
        flight.finish(code);
    }
}

/// Client decision used by `session_semantic_action` before any message is sent.
///
/// `Ok` means this call may send one `SemanticAction`. `Err` means send nothing.
/// A second `Ok` admission while a result is outstanding is [`IN_FLIGHT`].
pub fn prepare_send(flight: &mut Flight, admitted: &'static str) -> Result<(), &'static str> {
    if admitted != OK {
        return Err(admitted);
    }
    flight.begin()
}

/// Peer codes are a closed set. Anything else is an injection failure, and an
/// overlong code is not copied into the result slot.
pub fn normalize_peer_code(code: &str) -> &'static str {
    if code.len() > 32 {
        log::error!("semantic action peer code is {} bytes", code.len());
        return INJECT_FAILED;
    }
    match code {
        OK => OK,
        UNSUPPORTED => UNSUPPORTED,
        INJECT_FAILED => INJECT_FAILED,
        NO_LAUNCH_INTENT => NO_LAUNCH_INTENT,
        START_THREW => START_THREW,
        FG_TIMEOUT => FG_TIMEOUT,
        UPDATE_NETWORK => UPDATE_NETWORK,
        UPDATE_CHECKSUM => UPDATE_CHECKSUM,
        UPDATE_SIGNATURE => UPDATE_SIGNATURE,
        UPDATE_INSTALL => UPDATE_INSTALL,
        UPDATE_ID_CHANGED => UPDATE_ID_CHANGED,
        SNAPSHOT_FAILED => SNAPSHOT_FAILED,
        REF_STALE => REF_STALE,
        _ => {
            log::warn!("semantic action peer code `{code}` is not a known result");
            INJECT_FAILED
        }
    }
}

/// Run an already-admitted action on this peer.
pub fn execute_on_peer(kind: &str, arg: &str) -> &'static str {
    if kind == SNAPSHOT || kind == CLICK {
        log::error!("semantic {kind} does not run on the direct peer path");
        return SNAPSHOT_FAILED;
    }
    let verdict = validate(kind, arg);
    if verdict != OK {
        return verdict;
    }
    #[cfg(target_os = "android")]
    {
        return match scrap::android::call_main_service_semantic_action(kind, arg) {
            Ok(code) => normalize_peer_code(&code),
            Err(e) => {
                log::error!("semantic action jni failed: {e}");
                INJECT_FAILED
            }
        };
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (kind, arg);
        UNSUPPORTED
    }
}

/// Peer `list_apps` outcome. `detail` is set only when `code` is [`OK`].
pub struct ListAppsPeer {
    pub code: &'static str,
    pub detail: String,
}

impl ListAppsPeer {
    pub fn failed(code: &'static str) -> Self {
        Self {
            code,
            detail: String::new(),
        }
    }
}

/// Run an already-shaped `list_apps` on this peer.
///
/// The JNI string is `ok\n` plus the JSON page, or a bare failure code.
/// A bare `ok` is not an empty list.
pub fn execute_list_apps_on_peer(arg: &str) -> ListAppsPeer {
    let verdict = validate(LIST_APPS, arg);
    if verdict != OK {
        return ListAppsPeer::failed(verdict);
    }
    #[cfg(target_os = "android")]
    {
        return match scrap::android::call_main_service_semantic_action(LIST_APPS, arg) {
            Ok(raw) => accept_list_apps_wire(&raw),
            Err(e) => {
                log::error!("semantic list_apps jni failed: {e}");
                ListAppsPeer::failed(INJECT_FAILED)
            }
        };
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = arg;
        ListAppsPeer::failed(UNSUPPORTED)
    }
}

pub fn accept_list_apps_wire(raw: &str) -> ListAppsPeer {
    if let Some(json) = raw.strip_prefix("ok\n") {
        return match normalize_list_apps_detail(json) {
            Some(detail) => ListAppsPeer { code: OK, detail },
            None => {
                list_apps_payload_rejected();
                ListAppsPeer::failed(INJECT_FAILED)
            }
        };
    }
    match raw {
        OK => {
            list_apps_payload_rejected();
            ListAppsPeer::failed(INJECT_FAILED)
        }
        _ => ListAppsPeer::failed(normalize_peer_code(raw)),
    }
}

/// Decimal page offset. Empty is 0. Offsets past [`LIST_APPS_CAP`] are refused.
pub fn list_apps_offset(arg: &str) -> Option<usize> {
    if arg.is_empty() {
        return Some(0);
    }
    if arg.len() > 3 || !arg.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if arg.len() > 1 && arg.as_bytes()[0] == b'0' {
        return None;
    }
    let n: usize = arg.parse().ok()?;
    if n > LIST_APPS_CAP {
        return None;
    }
    Some(n)
}

/// Canonical `list_apps` detail. `None` means the payload is not a page.
///
/// Items without a usable package are dropped. A page longer than
/// [`LIST_APPS_PAGE`] is refused. `truncated` is copied through.
pub fn normalize_list_apps_detail(raw: &str) -> Option<String> {
    if raw.len() > LIST_APPS_DETAIL_MAX {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = value.as_object()?;
    let truncated = obj.get("truncated")?.as_bool()?;
    let items = obj.get("items")?.as_array()?;
    if items.len() > LIST_APPS_PAGE {
        return None;
    }
    let mut out = Vec::with_capacity(items.len());
    let mut dropped = 0usize;
    for item in items {
        let map = item.as_object()?;
        let package = match map.get("package").and_then(|v| v.as_str()) {
            Some(package) => package,
            None => return None,
        };
        if package.len() > OPEN_APP_MAX || !package_name_ok(package) {
            dropped += 1;
            continue;
        }
        let label = match map.get("label") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => clean_label(s),
            Some(_) => return None,
        };
        let activity = match map.get("activity") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => {
                if s.is_empty() {
                    None
                } else if s.len() <= OPEN_APP_MAX && activity_name_ok(s) {
                    Some(s.to_owned())
                } else {
                    None
                }
            }
            Some(_) => return None,
        };
        out.push(serde_json::json!({
            "package": package,
            "label": label,
            "activity": activity,
        }));
    }
    if dropped > 0 {
        hbb_common::throttled_log!(
            std::time::Duration::from_secs(30),
            warn,
            "semantic list_apps dropped {dropped} items"
        );
    }
    Some(
        serde_json::json!({
            "items": out,
            "truncated": truncated,
        })
        .to_string(),
    )
}

fn clean_label(raw: &str) -> Option<String> {
    if raw.len() > LIST_APPS_LABEL_MAX {
        return None;
    }
    let mut cleaned = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_control() {
            cleaned.push(' ');
        } else {
            cleaned.push(ch);
        }
    }
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn list_apps_payload_rejected() {
    hbb_common::throttled_log!(
        std::time::Duration::from_secs(30),
        error,
        "semantic list_apps payload rejected"
    );
}

fn package_spec_ok(spec: &str) -> bool {
    if spec.is_empty() || spec.len() > OPEN_APP_MAX {
        return false;
    }
    let (pkg, activity) = match spec.split_once('/') {
        Some((pkg, activity)) => (pkg, Some(activity)),
        None => (spec, None),
    };
    if !package_name_ok(pkg) {
        return false;
    }
    match activity {
        None => true,
        Some(activity) => activity_name_ok(activity),
    }
}

fn package_name_ok(name: &str) -> bool {
    let mut parts = name.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    if !ident_segment(first) {
        return false;
    }
    let mut more = false;
    for part in parts {
        if !ident_segment(part) {
            return false;
        }
        more = true;
    }
    more
}

fn activity_name_ok(activity: &str) -> bool {
    if let Some(rest) = activity.strip_prefix('.') {
        return !rest.is_empty() && rest.split('.').all(ident_segment);
    }
    package_name_ok(activity)
}

fn release_page_arg_ok(arg: &str) -> bool {
    crate::peer_apk_update::release_tag_arg_ok(arg)
}

fn ident_segment(seg: &str) -> bool {
    let mut chars = seg.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop<'a>(platform: &'a str) -> Gate<'a> {
        Gate {
            has_sender: true,
            remote_desktop: true,
            input_allowed: true,
            platform,
        }
    }

    #[test]
    fn admits_system_text_and_launch_actions() {
        assert_eq!(KINDS.len(), 10);
        let g = desktop("Android");
        for kind in [SYS_BACK, SYS_HOME, SYS_RECENTS] {
            assert_eq!(gate(&g, kind, ""), OK, "{kind}");
        }
        assert_eq!(gate(&g, IME, "show"), OK);
        assert_eq!(gate(&g, IME, "hide"), OK);
        assert_eq!(gate(&g, SET_TEXT, ""), OK);
        assert_eq!(gate(&g, SET_TEXT, "hello"), OK);
        assert_eq!(gate(&g, OPEN_APP, "com.android.settings"), OK);
        assert_eq!(gate(&g, OPEN_APP, "com.android.settings/.Settings"), OK);
        assert_eq!(
            gate(
                &g,
                OPEN_APP,
                "com.android.settings/com.android.settings.Settings"
            ),
            OK
        );
        assert_eq!(gate(&desktop(""), SYS_HOME, ""), OK);
        assert_eq!(gate(&desktop("android"), SYS_BACK, ""), OK);
    }

    #[test]
    fn failure_codes_stay_distinct() {
        let ready = desktop("Android");
        assert_eq!(
            gate(
                &Gate {
                    has_sender: false,
                    ..ready
                },
                "not-a-kind",
                ""
            ),
            NO_SESSION
        );
        assert_eq!(
            gate(
                &Gate {
                    remote_desktop: false,
                    ..ready
                },
                SYS_BACK,
                ""
            ),
            UNSUPPORTED
        );
        assert_eq!(
            gate(
                &Gate {
                    input_allowed: false,
                    ..ready
                },
                SYS_BACK,
                ""
            ),
            INJECT_FAILED
        );
        assert_eq!(gate(&desktop("Windows"), SYS_HOME, ""), UNSUPPORTED);
        assert_eq!(
            gate(&desktop("Linux"), OPEN_APP, "com.android.settings"),
            UNSUPPORTED
        );
        assert_eq!(gate(&ready, "sys.back ", ""), UNSUPPORTED);
        assert_eq!(gate(&ready, SYS_BACK, "x"), UNSUPPORTED);
        assert_eq!(gate(&ready, IME, ""), UNSUPPORTED);
        assert_eq!(gate(&ready, IME, "toggle"), UNSUPPORTED);
        assert_eq!(gate(&ready, OPEN_APP, ""), UNSUPPORTED);
        assert_eq!(gate(&ready, OPEN_APP, "settings"), UNSUPPORTED);
        assert_eq!(
            gate(&ready, OPEN_APP, "com.android.settings;rm"),
            UNSUPPORTED
        );
        assert_eq!(
            gate(&ready, OPEN_APP, "com.android.settings/$(id)"),
            UNSUPPORTED
        );
        let huge = "a".repeat(SET_TEXT_MAX + 1);
        assert_eq!(gate(&ready, SET_TEXT, &huge), UNSUPPORTED);
        assert_eq!(gate(&ready, SET_TEXT, &"a".repeat(SET_TEXT_MAX)), OK);
        assert_eq!(normalize_peer_code("ok"), OK);
        assert_eq!(normalize_peer_code("unsupported"), UNSUPPORTED);
        assert_eq!(normalize_peer_code("inject_failed"), INJECT_FAILED);
        assert_eq!(normalize_peer_code("no_session"), INJECT_FAILED);
        assert_eq!(normalize_peer_code(&"x".repeat(33)), INJECT_FAILED);
        assert_eq!(execute_on_peer(SYS_HOME, "x"), UNSUPPORTED);
        assert_eq!(execute_on_peer("pixel", ""), UNSUPPORTED);
        #[cfg(not(target_os = "android"))]
        assert_eq!(execute_on_peer(SYS_HOME, ""), UNSUPPORTED);
    }

    #[test]
    fn enqueue_ok_waits_for_a_peer_result() {
        let ready = desktop("Android");
        assert_eq!(gate(&ready, SYS_BACK, ""), OK);
        let waiting = require_peer_result("");
        assert_eq!(waiting, RESULT_MISSING);
        assert_ne!(waiting, OK);
        assert!(!waiting.is_empty());
        assert!(waiting.contains(INJECT_FAILED));
        assert!(!is_peer_success(waiting));
        assert!(!is_peer_success(""));
        assert!(!is_peer_success(require_peer_result("   ")));
        assert!(is_peer_success(require_peer_result(OK)));
        assert!(!is_peer_success(require_peer_result(NO_SESSION)));
        assert!(!is_peer_success(require_peer_result(UNSUPPORTED)));
        assert!(!is_peer_success(require_peer_result(INJECT_FAILED)));
        assert_eq!(require_peer_result(NO_SESSION), NO_SESSION);
        assert_eq!(require_peer_result(UNSUPPORTED), UNSUPPORTED);
        assert_eq!(require_peer_result(INJECT_FAILED), INJECT_FAILED);
    }

    #[test]
    fn client_does_not_send_until_the_peer_result_arrives() {
        let mut flight = Flight::default();
        let mut sent = Vec::new();

        prepare_send(&mut flight, OK).unwrap();
        sent.push(action_message(SYS_BACK, ""));
        assert_eq!(sent.len(), 1);
        assert_eq!(flight.result(), RESULT_MISSING);
        assert!(!is_peer_success(flight.result()));

        assert_eq!(prepare_send(&mut flight, OK), Err(IN_FLIGHT));
        assert_eq!(sent.len(), 1);
        assert_eq!(flight.result(), RESULT_MISSING);

        assert_eq!(prepare_send(&mut flight, NO_SESSION), Err(NO_SESSION));
        assert_eq!(sent.len(), 1);

        flight.finish("ok");
        assert!(is_peer_success(flight.result()));
        prepare_send(&mut flight, OK).unwrap();
        sent.push(action_message(SYS_HOME, ""));
        assert_eq!(sent.len(), 2);
        assert_eq!(flight.result(), RESULT_MISSING);
        assert!(!is_peer_success(flight.result()));

        flight.abort();
        prepare_send(&mut flight, OK).unwrap();
        sent.push(action_message(SYS_RECENTS, ""));
        assert_eq!(sent.len(), 3);
    }

    #[test]
    fn action_message_is_the_session_semantic_action() {
        let msg = action_message(SYS_BACK, "");
        let bytes = msg.write_to_bytes().unwrap();
        let parsed = Message::parse_from_bytes(&bytes).unwrap();
        match parsed.union {
            Some(base::message_proto::message::Union::SemanticAction(action)) => {
                assert_eq!(action.kind, SYS_BACK);
                assert_eq!(action.arg, "");
            }
            other => panic!("expected SemanticAction, got {other:?}"),
        }
        let reply = result_message(OPEN_APP, "inject_failed");
        match reply.union {
            Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                assert_eq!(result.kind, OPEN_APP);
                assert_eq!(result.code, INJECT_FAILED);
            }
            other => panic!("expected SemanticActionResult, got {other:?}"),
        }
    }

    #[test]
    fn open_app_failures_stay_explicit_and_share_the_flight() {
        let ready = desktop("Android");
        for code in [NO_LAUNCH_INTENT, START_THREW, FG_TIMEOUT] {
            assert_eq!(normalize_peer_code(code), code, "{code}");
            assert_ne!(normalize_peer_code(code), INJECT_FAILED, "{code}");
            assert!(!code.starts_with(INJECT_FAILED), "{code}");
            assert!(!is_peer_success(code), "{code}");
            assert!(code.len() <= 32, "{code}");
            let reply = result_message(OPEN_APP, code);
            match reply.union {
                Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                    assert_eq!(result.kind, OPEN_APP);
                    assert_eq!(result.code, code);
                    assert!(result.detail.is_empty(), "{code}");
                }
                other => panic!("expected SemanticActionResult, got {other:?}"),
            }
        }

        let mut flight = Flight::default();
        prepare_send(&mut flight, gate(&ready, OPEN_APP, "com.android.settings")).unwrap();
        assert_eq!(
            prepare_send(&mut flight, gate(&ready, OPEN_APP, "com.android.settings")),
            Err(IN_FLIGHT)
        );
        flight.finish(NO_LAUNCH_INTENT);
        assert_eq!(flight.result(), NO_LAUNCH_INTENT);
        assert!(!is_peer_success(flight.result()));

        prepare_send(&mut flight, OK).unwrap();
        flight.finish(FG_TIMEOUT);
        assert_eq!(flight.result(), FG_TIMEOUT);
        assert!(!is_peer_success(flight.result()));
        assert!(flight.detail().is_empty());
        assert_eq!(OPEN_APP_BLOCKING_TIMEOUT_MS, 5_000);
    }

    #[test]
    fn list_apps_page_passes_through_and_open_app_codes_stay() {
        let ready = desktop("Android");
        assert_eq!(LIST_APPS_PAGE, 40);
        assert_eq!(LIST_APPS_CAP, 256);
        assert_eq!(LIST_APPS_BLOCKING_TIMEOUT_MS, 5_000);
        assert_eq!(gate(&ready, LIST_APPS, ""), OK);
        assert_eq!(gate(&ready, LIST_APPS, "0"), OK);
        assert_eq!(gate(&ready, LIST_APPS, "40"), OK);
        assert_eq!(gate(&ready, LIST_APPS, "256"), OK);
        assert_eq!(gate(&ready, LIST_APPS, "257"), UNSUPPORTED);
        assert_eq!(gate(&ready, LIST_APPS, "00"), UNSUPPORTED);
        assert_eq!(gate(&ready, LIST_APPS, "-1"), UNSUPPORTED);
        assert_eq!(gate(&desktop("Windows"), LIST_APPS, ""), UNSUPPORTED);
        assert_eq!(list_apps_offset(""), Some(0));
        assert_eq!(list_apps_offset("240"), Some(240));

        let empty = r#"{"items":[],"truncated":false}"#;
        let norm = normalize_list_apps_detail(empty).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&norm).unwrap();
        assert!(parsed["items"].as_array().unwrap().is_empty());
        assert_eq!(parsed["truncated"], false);
        assert_eq!(
            normalize_list_apps_detail(&norm).as_deref(),
            Some(norm.as_str())
        );

        let mixed = r#"{"truncated":true,"items":[
            {"package":"com.android.settings","label":"Settings","activity":".Settings"},
            {"package":"nope","label":"X","activity":null},
            {"activity":"$(id)","label":"  ","package":"com.example.app"},
            {"package":"com.example.app","label":null,"activity":"com.example.app.Main"}
        ]}"#;
        let parsed: serde_json::Value =
            serde_json::from_str(&normalize_list_apps_detail(mixed).unwrap()).unwrap();
        assert_eq!(parsed["truncated"], true);
        let items = parsed["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["package"], "com.android.settings");
        assert_eq!(items[0]["label"], "Settings");
        assert_eq!(items[0]["activity"], ".Settings");
        assert_eq!(items[1]["package"], "com.example.app");
        assert!(items[1]["label"].is_null());
        assert!(items[1]["activity"].is_null());
        assert_eq!(items[2]["activity"], "com.example.app.Main");

        let mut too_many = Vec::new();
        for i in 0..41 {
            too_many.push(format!(
                r#"{{"package":"com.example.n{i}","label":null,"activity":null}}"#
            ));
        }
        let big = format!(r#"{{"items":[{}],"truncated":false}}"#, too_many.join(","));
        assert!(normalize_list_apps_detail(&big).is_none());
        assert!(normalize_list_apps_detail(&"x".repeat(LIST_APPS_DETAIL_MAX + 1)).is_none());
        assert!(normalize_list_apps_detail("[]").is_none());

        let wire = accept_list_apps_wire(&format!("ok\n{empty}"));
        assert_eq!(wire.code, OK);
        assert!(wire.detail.contains("\"items\":[]"));
        assert_eq!(accept_list_apps_wire("ok").code, INJECT_FAILED);
        assert!(accept_list_apps_wire("ok").detail.is_empty());
        assert_eq!(accept_list_apps_wire("unsupported").code, UNSUPPORTED);
        assert_eq!(accept_list_apps_wire("inject_failed").code, INJECT_FAILED);
        for code in [NO_LAUNCH_INTENT, START_THREW, FG_TIMEOUT] {
            assert_eq!(accept_list_apps_wire(code).code, code, "{code}");
            assert_ne!(accept_list_apps_wire(code).code, INJECT_FAILED, "{code}");
        }

        let kept = result_message_with_detail(OPEN_APP, NO_LAUNCH_INTENT, empty);
        match kept.union {
            Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                assert_eq!(result.code, NO_LAUNCH_INTENT);
                assert!(result.detail.is_empty());
            }
            other => panic!("expected SemanticActionResult, got {other:?}"),
        }
        let page = result_message_with_detail(LIST_APPS, OK, empty);
        match page.union {
            Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                assert_eq!(result.kind, LIST_APPS);
                assert_eq!(result.code, OK);
                assert!(result.detail.contains("\"items\":[]"));
                assert!(result.detail.contains("\"truncated\":false"));
            }
            other => panic!("expected SemanticActionResult, got {other:?}"),
        }
        let rejected = result_message_with_detail(LIST_APPS, OK, "nope");
        match rejected.union {
            Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                assert_eq!(result.code, INJECT_FAILED);
                assert!(result.detail.is_empty());
            }
            other => panic!("expected SemanticActionResult, got {other:?}"),
        }

        let mut flight = Flight::default();
        store_peer_result(&mut flight, LIST_APPS, OK, empty);
        assert!(is_peer_success(flight.result()));
        assert!(flight.detail().contains("\"items\":[]"));
        flight.begin().unwrap();
        assert!(flight.detail().is_empty());
        assert_eq!(flight.result(), RESULT_MISSING);
        flight.finish(START_THREW);
        assert_eq!(flight.result(), START_THREW);
        assert!(flight.detail().is_empty());
        assert!(!is_peer_success(flight.result()));

        store_peer_result(&mut flight, OPEN_APP, FG_TIMEOUT, empty);
        assert_eq!(flight.result(), FG_TIMEOUT);
        assert!(flight.detail().is_empty());
        store_peer_result(&mut flight, OPEN_APP, NO_LAUNCH_INTENT, empty);
        assert_eq!(flight.result(), NO_LAUNCH_INTENT);
        assert_ne!(flight.result(), INJECT_FAILED);

        store_peer_result(&mut flight, LIST_APPS, OK, "");
        assert_eq!(flight.result(), INJECT_FAILED);
        assert!(flight.detail().is_empty());
        assert!(!is_peer_success(flight.result()));

        store_peer_result(&mut flight, LIST_APPS, UNSUPPORTED, empty);
        assert_eq!(flight.result(), UNSUPPORTED);
        assert!(flight.detail().is_empty());

        #[cfg(not(target_os = "android"))]
        {
            let outcome = execute_list_apps_on_peer("");
            assert_eq!(outcome.code, UNSUPPORTED);
            assert!(outcome.detail.is_empty());
            assert_eq!(execute_list_apps_on_peer("nope").code, UNSUPPORTED);
        }
    }

    #[test]
    fn check_update_codes_stay_explicit() {
        let ready = desktop("Android");
        assert_eq!(gate(&ready, CHECK_UPDATE, ""), OK);
        assert_eq!(
            gate(
                &ready,
                CHECK_UPDATE,
                "https://github.com/1304257658/mobile-cu-bridge/releases/tag/device-e1e2e5e8e912-arm64-20261103"
            ),
            OK
        );
        assert_eq!(
            gate(&ready, CHECK_UPDATE, "https://example.com/app.apk"),
            UNSUPPORTED
        );
        assert_eq!(
            gate(
                &ready,
                CHECK_UPDATE,
                "https://github.com/rustdesk/rustdesk/releases/tag/1.4.6"
            ),
            UNSUPPORTED
        );
        assert_eq!(
            gate(
                &ready,
                CHECK_UPDATE,
                "https://api.rustdesk.com/version/latest"
            ),
            UNSUPPORTED
        );
        assert_eq!(gate(&desktop("Windows"), CHECK_UPDATE, ""), UNSUPPORTED);
        for code in [
            UPDATE_NETWORK,
            UPDATE_CHECKSUM,
            UPDATE_SIGNATURE,
            UPDATE_INSTALL,
            UPDATE_ID_CHANGED,
        ] {
            assert_eq!(normalize_peer_code(code), code, "{code}");
            assert_ne!(code, INJECT_FAILED);
            assert!(!is_peer_success(code));
            assert!(code.len() <= 32, "{code}");
            let reply = result_message(CHECK_UPDATE, code);
            match reply.union {
                Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                    assert_eq!(result.kind, CHECK_UPDATE);
                    assert_eq!(result.code, code);
                    assert!(result.detail.is_empty());
                }
                other => panic!("expected SemanticActionResult, got {other:?}"),
            }
        }
        let mut flight = Flight::default();
        store_peer_result(&mut flight, CHECK_UPDATE, UPDATE_ID_CHANGED, "ignored");
        assert_eq!(flight.result(), UPDATE_ID_CHANGED);
        assert!(flight.detail().is_empty());
        assert!(!is_peer_success(flight.result()));
    }

    #[test]
    fn snapshot_and_click_codes_stay_explicit() {
        let ready = desktop("Android");
        assert_eq!(gate(&ready, SNAPSHOT, ""), OK);
        assert_eq!(gate(&ready, SNAPSHOT, "com.sankuai.meituan"), UNSUPPORTED);
        assert_eq!(gate(&desktop("Windows"), SNAPSHOT, ""), UNSUPPORTED);
        assert_eq!(gate(&ready, CLICK, "n0"), OK);
        let nine = format!(
            "[{}]",
            (0..9)
                .map(|i| format!("\"n{i}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(gate(&ready, CLICK, &nine), SNAPSHOT_FAILED);
        assert_eq!(execute_on_peer(SNAPSHOT, ""), SNAPSHOT_FAILED);
        assert_eq!(execute_on_peer(CLICK, "n0"), SNAPSHOT_FAILED);
        for code in [SNAPSHOT_FAILED, REF_STALE] {
            assert_eq!(normalize_peer_code(code), code, "{code}");
            assert_ne!(code, INJECT_FAILED);
            assert!(!is_peer_success(code));
            let reply = result_message(CLICK, code);
            match reply.union {
                Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                    assert_eq!(result.code, code);
                    assert!(result.detail.is_empty());
                }
                other => panic!("expected SemanticActionResult, got {other:?}"),
            }
        }
        let page = r#"{"truncated":false,"reason":"","nodes":[]}"#;
        let mut flight = Flight::default();
        store_peer_result(&mut flight, SNAPSHOT, OK, page);
        assert!(is_peer_success(flight.result()));
        assert!(flight.detail().contains("\"nodes\":[]"));
        store_peer_result(&mut flight, SNAPSHOT, REF_STALE, page);
        assert_eq!(flight.result(), REF_STALE);
        assert!(flight.detail().is_empty());
        store_peer_result(
            &mut flight,
            SNAPSHOT,
            OK,
            &"x".repeat(crate::a11y_snapshot::caps().unwrap().payload_max_bytes + 1),
        );
        assert_eq!(flight.result(), SNAPSHOT_FAILED);
        assert!(flight.detail().is_empty());
        assert!(!is_peer_success(flight.result()));
    }
}
