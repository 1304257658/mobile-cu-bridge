import 'dart:async';

import 'package:flutter/material.dart';

import '../../models/semantic_action.dart';

/// Controller-side wait ended with no peer completion. Not success.
const String semanticBarTimeout = 'timeout';

/// How long the bar waits for a peer completion before showing [semanticBarTimeout].
const Duration semanticBarPeerWait = Duration(seconds: 8);

/// Existing session channel: enqueue, then read the peer completion.
///
/// [enqueue] is `sessionSemanticAction`. Its `ok` means queued, not done.
/// [peerResult] is `sessionSemanticResult`.
class SemanticActionPort {
  const SemanticActionPort({required this.enqueue, required this.peerResult});

  final String Function(String kind, String arg) enqueue;
  final String Function() peerResult;
}

/// One in-flight semantic action for the session bar.
///
/// A second [send] while [busy] is dropped. Nothing is queued locally.
/// The session slot is the same single-flight gate: a rejected enqueue is
/// shown as its code (`inject_failed: action in flight` and the rest).
class SemanticActionRunner {
  SemanticActionRunner({
    required this.port,
    this.timeout = semanticBarPeerWait,
    this.pollInterval = const Duration(milliseconds: 200),
    DateTime Function()? now,
    Future<void> Function(Duration delay)? wait,
    required this.onChanged,
    required this.onPeerSuccess,
  })  : now = now ?? DateTime.now,
        wait = wait ?? ((Duration delay) => Future<void>.delayed(delay));

  SemanticActionPort port;
  Duration timeout;
  Duration pollInterval;
  DateTime Function() now;
  Future<void> Function(Duration delay) wait;
  final VoidCallback onChanged;
  final VoidCallback onPeerSuccess;

  bool busy = false;
  String failure = '';

  bool _disposed = false;
  int _ticket = 0;

  void dispose() {
    _disposed = true;
    _ticket++;
  }

  bool _alive(int ticket) => !_disposed && ticket == _ticket;

  /// Sends one action. Enqueue `ok` does not complete the action.
  Future<void> send(String kind, String arg) async {
    if (busy || _disposed) return;
    busy = true;
    failure = '';
    final ticket = ++_ticket;
    _notify();
    String queued;
    try {
      queued = port.enqueue(kind, arg);
    } catch (_) {
      _fail(ticket, semanticInjectFailed);
      return;
    }
    if (!_alive(ticket)) return;
    // `ok` here is the enqueue acknowledgement. Peer success is a later code.
    if (queued != semanticOk) {
      _fail(ticket, semanticCompletion(queued));
      return;
    }
    final deadline = now().add(timeout);
    final step =
        pollInterval.inMilliseconds <= 0 ? 1 : pollInterval.inMilliseconds;
    final maxPolls = timeout.inMilliseconds <= 0
        ? 1
        : (timeout.inMilliseconds + step - 1) ~/ step;
    for (var poll = 0; poll < maxPolls; poll++) {
      if (!_alive(ticket)) return;
      String code;
      try {
        code = semanticCompletion(port.peerResult());
      } catch (_) {
        _fail(ticket, semanticInjectFailed);
        return;
      }
      if (code != semanticResultMissing) {
        _finishPeer(ticket, code);
        return;
      }
      if (!now().isBefore(deadline)) break;
      await wait(pollInterval);
    }
    if (!_alive(ticket)) return;
    _fail(ticket, semanticBarTimeout);
  }

  void _finishPeer(int ticket, String code) {
    if (!_alive(ticket)) return;
    busy = false;
    if (semanticPeerSuccess(code)) {
      failure = '';
      _notify();
      onPeerSuccess();
      return;
    }
    failure = code;
    _notify();
  }

  void _fail(int ticket, String code) {
    if (!_alive(ticket)) return;
    busy = false;
    failure = code;
    _notify();
  }

  void _notify() {
    if (_disposed) return;
    onChanged();
  }
}

/// Same-screen bar: six semantic actions, no menu and no second page.
///
/// `sys.back`, `sys.home`, and `sys.recents` are immediate buttons.
/// `ime`, `open_app`, and `set_text` each have one line and a confirm
/// on this bar. The whole bar is grey and inert while a peer result is
/// outstanding. Peer `ok` is a short toast. Any other completion is the
/// code in plaintext on the bar.
class SemanticActionBar extends StatefulWidget {
  const SemanticActionBar({
    super.key,
    required this.port,
    this.timeout = semanticBarPeerWait,
    this.pollInterval = const Duration(milliseconds: 200),
    this.now,
    this.wait,
  });

  final SemanticActionPort port;
  final Duration timeout;
  final Duration pollInterval;
  final DateTime Function()? now;
  final Future<void> Function(Duration delay)? wait;

  @override
  State<SemanticActionBar> createState() => _SemanticActionBarState();
}

class _SemanticActionBarState extends State<SemanticActionBar> {
  late final SemanticActionRunner _runner;
  late final TextEditingController _ime;
  late final TextEditingController _package;
  late final TextEditingController _text;

  @override
  void initState() {
    super.initState();
    _ime = TextEditingController(text: semanticImeShow);
    _package = TextEditingController();
    _text = TextEditingController();
    _runner = SemanticActionRunner(
      port: widget.port,
      timeout: widget.timeout,
      pollInterval: widget.pollInterval,
      now: widget.now,
      wait: widget.wait,
      onChanged: () {
        if (mounted) setState(() {});
      },
      onPeerSuccess: () {
        if (!mounted) return;
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('ok'),
            duration: Duration(seconds: 2),
            behavior: SnackBarBehavior.floating,
          ),
        );
      },
    );
  }

  @override
  void didUpdateWidget(SemanticActionBar oldWidget) {
    super.didUpdateWidget(oldWidget);
    _runner.port = widget.port;
    _runner.timeout = widget.timeout;
    _runner.pollInterval = widget.pollInterval;
    if (widget.now != null) _runner.now = widget.now!;
    if (widget.wait != null) _runner.wait = widget.wait!;
  }

  @override
  void dispose() {
    _runner.dispose();
    _ime.dispose();
    _package.dispose();
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final busy = _runner.busy;
    final scheme = Theme.of(context).colorScheme;
    return Material(
      key: const Key('semantic-action-bar'),
      color: scheme.surface,
      elevation: 6,
      child: Opacity(
        opacity: busy ? 0.4 : 1,
        child: IgnorePointer(
          ignoring: busy,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Wrap(
                  spacing: 4,
                  runSpacing: 4,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    for (final kind in const <String>[
                      semanticSysBack,
                      semanticSysHome,
                      semanticSysRecents,
                    ])
                      _immediate(kind, busy),
                    _fieldAction(
                      kind: semanticIme,
                      controller: _ime,
                      hint: 'show|hide',
                      width: 88,
                      busy: busy,
                    ),
                    _fieldAction(
                      kind: semanticOpenApp,
                      controller: _package,
                      hint: 'package',
                      width: 140,
                      busy: busy,
                    ),
                    _fieldAction(
                      kind: semanticSetText,
                      controller: _text,
                      hint: 'text',
                      width: 140,
                      busy: busy,
                    ),
                  ],
                ),
                if (_runner.failure.isNotEmpty)
                  Text(
                    _runner.failure,
                    key: const Key('semantic-failure'),
                    style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                          color: scheme.onSurface,
                        ),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _immediate(String kind, bool busy) {
    return TextButton(
      key: Key('semantic-$kind'),
      onPressed: busy ? null : () => _runner.send(kind, ''),
      child: Text(kind),
    );
  }

  Widget _fieldAction({
    required String kind,
    required TextEditingController controller,
    required String hint,
    required double width,
    required bool busy,
  }) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        SizedBox(
          width: width,
          child: TextField(
            key: Key('semantic-$kind-arg'),
            controller: controller,
            enabled: !busy,
            maxLines: 1,
            textInputAction: TextInputAction.done,
            decoration: InputDecoration(
              isDense: true,
              hintText: hint,
            ),
            onSubmitted:
                busy ? null : (_) => _runner.send(kind, controller.text),
          ),
        ),
        TextButton(
          key: Key('semantic-$kind'),
          onPressed: busy ? null : () => _runner.send(kind, controller.text),
          child: Text(kind),
        ),
      ],
    );
  }
}
