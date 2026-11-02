import 'package:flutter_hbb/models/semantic_action.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('semantic plane is the six session actions', () {
    expect(semanticKinds, [
      'sys.back',
      'sys.home',
      'sys.recents',
      'open_app',
      'ime',
      'set_text',
    ]);
    expect(semanticKinds.toSet().length, semanticKinds.length);
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
  });
}
