import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:uuid/uuid.dart';

import 'models/platform_model.dart';
import 'models/semantic_action.dart';

/// One-off acceptance driver. Not a session-bar control.
///
/// Polls `${TMPDIR:-/tmp}/e2-smoke/cmd.txt` for `kind\targ\tmode`.
/// An empty mode is `live`. `list_apps` stays off [semanticKinds]; this file
/// is how an operator sends it.
class E2SmokeDriver {
  E2SmokeDriver._();

  static const Duration _pollEvery = Duration(milliseconds: 200);
  static const Duration _peerWait = Duration(seconds: 15);

  static Timer? _timer;
  static UuidValue? _sessionId;
  static int _ownerSeq = 0;
  static int? _active;
  static bool _busy = false;

  static String get root {
    final env = Platform.environment['TMPDIR'];
    final tmp = (env == null || env.isEmpty) ? '/tmp' : env;
    return '${tmp.replaceAll(RegExp(r'/+$'), '')}/e2-smoke';
  }

  /// Starts polling. A later [start] replaces this one. [stop] is a no-op
  /// unless [owner] is still the active driver.
  static int start(UuidValue sessionId) {
    final owner = ++_ownerSeq;
    _active = owner;
    _sessionId = sessionId;
    _timer?.cancel();
    _timer = Timer.periodic(_pollEvery, (_) {
      unawaited(_tick());
    });
    try {
      Directory(root).createSync(recursive: true);
      _append('listening $root session=$sessionId');
    } catch (e) {
      debugPrint('e2 smoke: $e');
    }
    return owner;
  }

  static void stop(int owner) {
    if (_active != owner) return;
    _timer?.cancel();
    _timer = null;
    _active = null;
    _sessionId = null;
  }

  static Future<void> _tick() async {
    if (_busy) return;
    final sessionId = _sessionId;
    if (sessionId == null) return;
    final cmd = File('$root/cmd.txt');
    if (!cmd.existsSync()) return;
    String raw;
    try {
      raw = cmd.readAsStringSync();
    } catch (_) {
      return;
    }
    final parsed = parseE2SmokeCommand(raw);
    if (parsed == null) return;
    try {
      cmd.deleteSync();
    } catch (_) {
      return;
    }
    _busy = true;
    try {
      await _run(sessionId, parsed);
    } catch (e, st) {
      debugPrint('e2 smoke: $e');
      debugPrintStack(stackTrace: st);
      _writeOutputs('harness_error: $e', '');
      _append(
          'kind=${parsed.kind} arg=${parsed.arg} mode=${parsed.mode} harness_error=$e');
    } finally {
      _busy = false;
    }
  }

  static Future<void> _run(UuidValue sessionId, E2SmokeCommand parsed) async {
    final queued = bind.sessionSemanticAction(
      sessionId: sessionId,
      kind: parsed.kind,
      arg: parsed.arg,
    );
    final deadline = DateTime.now().add(_peerWait);
    var result = bind.sessionSemanticResult(sessionId: sessionId);
    while (result == semanticResultMissing && DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(_pollEvery);
      if (_sessionId != sessionId) return;
      result = bind.sessionSemanticResult(sessionId: sessionId);
    }
    final detail = bind.sessionSemanticDetail(sessionId: sessionId);
    _writeOutputs(result, detail);
    _append(
        'kind=${parsed.kind} arg=${parsed.arg} mode=${parsed.mode} enqueue=$queued result=$result');
    _append('detail=$detail');
  }

  static void _writeOutputs(String result, String detail) {
    Directory(root).createSync(recursive: true);
    _atomic('last_result.txt', _line(result));
    _atomic('last_detail.txt', _line(detail));
  }

  static String _line(String value) {
    if (value.endsWith('\n')) return value;
    return '$value\n';
  }

  static void _atomic(String name, String contents) {
    final dir = root;
    final tmp = File('$dir/.$name.tmp');
    tmp.writeAsStringSync(contents);
    tmp.renameSync('$dir/$name');
  }

  static void _append(String line) {
    try {
      Directory(root).createSync(recursive: true);
      final stamp = DateTime.now().toUtc().toIso8601String();
      File('$root/out.log')
          .writeAsStringSync('$stamp $line\n', mode: FileMode.append);
    } catch (e) {
      debugPrint('e2 smoke log: $e');
    }
  }
}

class E2SmokeCommand {
  const E2SmokeCommand(this.kind, this.arg, this.mode);

  final String kind;
  final String arg;
  final String mode;
}

/// First line of [raw], split on tabs: kind, arg, mode.
/// Mode defaults to `live`. Empty or blank kind is not a command.
E2SmokeCommand? parseE2SmokeCommand(String raw) {
  final line = raw.split(RegExp(r'\r?\n')).first;
  if (line.trim().isEmpty) return null;
  final parts = line.split('\t');
  final kind = parts[0].trim();
  if (kind.isEmpty) return null;
  final arg = parts.length > 1 ? parts[1] : '';
  final modePart = parts.length > 2 ? parts[2].trim() : '';
  return E2SmokeCommand(kind, arg, modePart.isEmpty ? 'live' : modePart);
}
