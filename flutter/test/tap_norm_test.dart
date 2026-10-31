import 'package:flutter_hbb/models/tap_norm.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('failure codes are no_session unsupported inject_failed and tap_miss', () {
    expect(
      {
        tapNormNoSession,
        tapNormUnsupported,
        tapNormInjectFailed,
        tapNormMiss,
      }.length,
      4,
    );
    expect(tapNormMiss, 'tap_miss');
    expect(tapNormPeerSuccess(tapNormOk), isTrue);
    expect(tapNormPeerSuccess(tapNormMiss), isFalse);
    expect(tapNormPeerSuccess(tapNormNoSession), isFalse);
    expect(tapNormPeerSuccess(tapNormUnsupported), isFalse);
    expect(tapNormPeerSuccess(tapNormInjectFailed), isFalse);
    expect(tapNormPeerSuccess(tapNormInFlight), isFalse);
    expect(tapNormPeerSuccess(tapNormResultMissing), isFalse);
    expect(tapNormResultMissing, isNot(tapNormOk));
  });
}
