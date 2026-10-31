//! Semantic shortcuts sent as a session control message.
//!
//! The controller admits the action and sends [`SemanticAction`]. The
//! controlled Android peer executes it (accessibility global action, a
//! package intent, or the focused field). The peer answers with
//! [`SemanticActionResult`]. Host input injection, the clipboard, and
//! pointer hits are not this path.

use base::message_proto::{Message, SemanticAction, SemanticActionResult};
use hbb_common::{log, protobuf::Message as _};

pub const SYS_BACK: &str = "sys.back";
pub const SYS_HOME: &str = "sys.home";
pub const SYS_RECENTS: &str = "sys.recents";
pub const OPEN_APP: &str = "open_app";
pub const IME: &str = "ime";
pub const SET_TEXT: &str = "set_text";

pub const KINDS: [&str; 6] = [SYS_BACK, SYS_HOME, SYS_RECENTS, OPEN_APP, IME, SET_TEXT];

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

/// Bytes, not chars. Larger `set_text` payloads are refused before they are copied
/// into a session message.
pub const SET_TEXT_MAX: usize = 16 * 1024;
pub const OPEN_APP_MAX: usize = 256;

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
        _ => {
            log::warn!("semantic action peer code `{code}` is not a known result");
            INJECT_FAILED
        }
    }
}

/// Run an already-admitted action on this peer.
pub fn execute_on_peer(kind: &str, arg: &str) -> &'static str {
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
        assert_eq!(KINDS.len(), 6);
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
}
