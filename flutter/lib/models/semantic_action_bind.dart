import 'package:uuid/uuid.dart';

import '../common/widgets/semantic_action_bar.dart';
import 'platform_model.dart';

/// Session semantic channel. Enqueue `ok` is not peer success.
SemanticActionPort semanticActionPort(UuidValue sessionId) {
  return SemanticActionPort(
    enqueue: (String kind, String arg) => bind.sessionSemanticAction(
      sessionId: sessionId,
      kind: kind,
      arg: arg,
    ),
    peerResult: () => bind.sessionSemanticResult(sessionId: sessionId),
  );
}
