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

  test('failure codes are distinct from success', () {
    expect(
      {semanticOk, semanticNoSession, semanticUnsupported, semanticInjectFailed}
          .length,
      4,
    );
    expect(semanticImeShow, 'show');
    expect(semanticImeHide, 'hide');
  });
}
