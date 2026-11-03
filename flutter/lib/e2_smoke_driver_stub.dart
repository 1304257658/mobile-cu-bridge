import 'package:uuid/uuid.dart';

/// Web has no command-file harness. The desktop page still links this symbol.
class E2SmokeDriver {
  static int start(UuidValue sessionId) => 0;

  static void stop(int owner) {}
}
