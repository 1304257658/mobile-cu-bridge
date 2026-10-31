/// Content-normalized tap on the contain rectangle.
///
/// The native controller sends it through `sessionTapNorm` (Rust
/// `session_tap_norm`) on the same session channel as semantic actions, and
/// on the same single-flight slot. The peer plays one gesture. This is not a
/// host click and it is not `sys.*`, `open_app`, or `set_text`.
const String tapNormOk = 'ok';
const String tapNormNoSession = 'no_session';
const String tapNormUnsupported = 'unsupported';
const String tapNormInjectFailed = 'inject_failed';
const String tapNormMiss = 'tap_miss';

/// `sessionTapResult` when the peer has not answered. Not success.
const String tapNormResultMissing = 'inject_failed: no peer result';

/// Second action before the in-flight one has a peer result. Not sent.
const String tapNormInFlight = 'inject_failed: action in flight';

/// True only when [peerResult] is the peer completion `ok`.
///
/// Do not pass the return value of `sessionTapNorm`. That `ok` means the
/// message was queued.
bool tapNormPeerSuccess(String peerResult) => peerResult == tapNormOk;
