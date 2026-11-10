import 'package:flutter_hbb/models/semantic_action.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('semantic plane is the session actions', () {
    expect(semanticKinds, [
      'sys.back',
      'sys.home',
      'sys.recents',
      'open_app',
      'ime',
      'set_text',
    ]);
    expect(semanticKinds.toSet().length, semanticKinds.length);
    expect(semanticListApps, 'list_apps');
    expect(semanticCheckUpdate, 'check_update');
    expect(semanticKinds.contains(semanticCheckUpdate), isFalse);
    expect(semanticListAppsPage, 40);
    expect(semanticListAppsCap, 256);
  });

  test('failure codes stay no_session unsupported and inject_failed', () {
    expect(
      {semanticNoSession, semanticUnsupported, semanticInjectFailed}.length,
      3,
    );
    expect(semanticImeShow, 'show');
    expect(semanticImeHide, 'hide');
  });

  test('enqueue ok is not success until the peer result is ok', () {
    expect(semanticCompletion(null), semanticResultMissing);
    expect(semanticCompletion(''), semanticResultMissing);
    expect(semanticCompletion('   '), semanticResultMissing);
    expect(semanticPeerSuccess(semanticCompletion(null)), isFalse);
    expect(semanticPeerSuccess(semanticCompletion('')), isFalse);
    expect(semanticResultMissing, isNot(semanticOk));
    expect(semanticResultMissing, contains(semanticInjectFailed));
    expect(semanticPeerSuccess(semanticOk), isTrue);
    expect(semanticPeerSuccess(semanticNoSession), isFalse);
    expect(semanticPeerSuccess(semanticUnsupported), isFalse);
    expect(semanticPeerSuccess(semanticInjectFailed), isFalse);
    expect(semanticPeerSuccess(semanticInFlight), isFalse);
    expect(semanticInFlight, isNot(semanticOk));
    expect(semanticInFlight, isNot(semanticResultMissing));
    expect(semanticNoLaunchIntent, 'no_launch_intent');
    expect(semanticStartThrew, 'start_threw');
    expect(semanticFgTimeout, 'fg_timeout');
    for (final code in [
      semanticNoLaunchIntent,
      semanticStartThrew,
      semanticFgTimeout,
    ]) {
      expect(semanticPeerSuccess(code), isFalse);
      expect(code, isNot(contains(semanticInjectFailed)));
      expect(code, isNot(semanticOk));
    }
    expect(semanticUpdateNetwork, 'network');
    expect(semanticUpdateChecksum, 'checksum');
    expect(semanticUpdateSignature, 'signature');
    expect(semanticUpdateInstall, 'install');
    expect(semanticUpdateIdChanged, 'id_changed');
    for (final code in [
      semanticUpdateNetwork,
      semanticUpdateChecksum,
      semanticUpdateSignature,
      semanticUpdateInstall,
      semanticUpdateIdChanged,
    ]) {
      expect(semanticPeerSuccess(code), isFalse);
      expect(code, isNot(contains(semanticInjectFailed)));
      expect(code, isNot(semanticOk));
    }
    expect(semanticUpdateAlreadyLatest, 'already_latest');
    expect(semanticPeerSuccess(semanticUpdateAlreadyLatest), isFalse);
    expect(semanticUpdateAlreadyLatest, isNot(semanticOk));
    expect(semanticUpdateAlreadyLatest, isNot(contains(semanticInjectFailed)));
  });

  test('peer success is only ok and already_latest is not a code', () {
    expect(semanticPeerSuccess(semanticOk), isTrue);
    expect(semanticPeerSuccess(semanticUpdateAlreadyLatest), isFalse);
    for (final code in [
      semanticUpdateAlreadyLatest,
      semanticUpdateNetwork,
      semanticUpdateChecksum,
      semanticUpdateSignature,
      semanticUpdateInstall,
      semanticUpdateIdChanged,
      semanticNoSession,
      semanticUnsupported,
      semanticInjectFailed,
    ]) {
      expect(semanticPeerSuccess(code), isFalse);
      expect(code, isNot(semanticOk));
    }
  });
}
