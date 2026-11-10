/// Frozen semantic shortcut plane for a remote session.
///
/// The native controller sends these through `sessionSemanticAction` (Rust
/// `session_semantic_action`). The peer executes them. They are not host
/// key events, a clipboard write, or a tap on the system navigation bar.
const String semanticSysBack = 'sys.back';
const String semanticSysHome = 'sys.home';
const String semanticSysRecents = 'sys.recents';
const String semanticOpenApp = 'open_app';
const String semanticListApps = 'list_apps';
const String semanticIme = 'ime';
const String semanticSetText = 'set_text';
const String semanticCheckUpdate = 'check_update';

/// `list_apps` returns at most this many items. The next page starts here.
const int semanticListAppsPage = 40;

/// `list_apps` stops after this many launcher items and sets `truncated`.
const int semanticListAppsCap = 256;

/// Session bar controls. `list_apps` stays off this list.
const List<String> semanticKinds = <String>[
  semanticSysBack,
  semanticSysHome,
  semanticSysRecents,
  semanticOpenApp,
  semanticIme,
  semanticSetText,
];

/// Enqueue acknowledgement from `sessionSemanticAction`, and the peer
/// completion from `sessionSemanticResult`. Enqueue `ok` is not success.
const String semanticOk = 'ok';
const String semanticNoSession = 'no_session';
const String semanticUnsupported = 'unsupported';
const String semanticInjectFailed = 'inject_failed';

/// `sessionSemanticResult` when the peer has not answered. Not success.
const String semanticResultMissing = 'inject_failed: no peer result';

/// Second `sessionSemanticAction` before the in-flight one has a peer result.
/// The call is not sent.
const String semanticInFlight = 'inject_failed: action in flight';

/// Peer `open_app` failures. Not success, and not [semanticInjectFailed].
/// `list_apps` does not use these. Its items are the peer detail payload.
/// An empty items array with [semanticOk] is an empty launcher set.
const String semanticNoLaunchIntent = 'no_launch_intent';
const String semanticStartThrew = 'start_threw';
const String semanticFgTimeout = 'fg_timeout';

/// Peer `check_update` failures. Not success, and not [semanticInjectFailed].
/// `id_changed` means the device id or enc_id differed after cover-install.
const String semanticUpdateNetwork = 'network';
const String semanticUpdateChecksum = 'checksum';
const String semanticUpdateSignature = 'signature';
const String semanticUpdateInstall = 'install';
const String semanticUpdateIdChanged = 'id_changed';

/// Detail on a `check_update` [semanticOk] when cover-install was skipped.
/// Not a result code. Read it from `sessionSemanticDetail`.
const String semanticUpdateAlreadyLatest = 'already_latest';

/// True only when [peerResult] is the peer completion `ok`.
///
/// Do not pass the return value of `sessionSemanticAction`. That `ok` means
/// the message was queued. [semanticUpdateAlreadyLatest] is a detail, not success
/// by itself.
bool semanticPeerSuccess(String peerResult) => peerResult == semanticOk;

/// Empty or missing completion is a readable failure, not success.
String semanticCompletion(String? peerResult) {
  if (peerResult == null || peerResult.trim().isEmpty) {
    return semanticResultMissing;
  }
  return peerResult;
}

/// `ime` argument. `show` lets the soft keyboard appear for a focused field.
/// `hide` suppresses it. Neither taps the keyboard.
const String semanticImeShow = 'show';
const String semanticImeHide = 'hide';
