/// Frozen semantic shortcut plane for a remote session.
///
/// The native controller sends these through `sessionSemanticAction` (Rust
/// `session_semantic_action`). The peer executes them. They are not host
/// key events, a clipboard write, or a tap on the system navigation bar.
const String semanticSysBack = 'sys.back';
const String semanticSysHome = 'sys.home';
const String semanticSysRecents = 'sys.recents';
const String semanticOpenApp = 'open_app';
const String semanticIme = 'ime';
const String semanticSetText = 'set_text';

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

/// Peer `open_app` failures. Not success.
const String semanticNotInstalled = 'inject_failed: not_installed';
const String semanticNoPermission = 'inject_failed: no_permission';
const String semanticOpenTimeout = 'inject_failed: timeout';

/// True only when [peerResult] is the peer completion `ok`.
///
/// Do not pass the return value of `sessionSemanticAction`. That `ok` means
/// the message was queued.
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
