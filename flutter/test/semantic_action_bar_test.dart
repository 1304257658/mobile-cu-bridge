import 'package:flutter/material.dart';
import 'package:flutter_hbb/common/widgets/semantic_action_bar.dart';
import 'package:flutter_hbb/models/semantic_action.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('six controls wait for peer ok and ignore taps while busy',
      (tester) async {
    final calls = <List<String>>[];
    var peer = semanticResultMissing;
    await tester.pumpWidget(_harness(SemanticActionBar(
      port: SemanticActionPort(
        enqueue: (kind, arg) {
          calls.add([kind, arg]);
          return semanticOk;
        },
        peerResult: () => peer,
      ),
      pollInterval: const Duration(milliseconds: 10),
      timeout: const Duration(seconds: 5),
    )));

    expect(find.byKey(const Key('semantic-action-bar')), findsOneWidget);
    for (final kind in semanticKinds) {
      expect(find.text(kind), findsOneWidget);
    }
    expect(find.byKey(const Key('semantic-open_app-arg')), findsOneWidget);
    expect(find.byKey(const Key('semantic-set_text-arg')), findsOneWidget);
    expect(find.byKey(const Key('semantic-ime-arg')), findsOneWidget);
    expect(find.byType(PopupMenuButton), findsNothing);
    expect(
      tester
          .widget<TextButton>(find.widgetWithText(TextButton, semanticSysHome))
          .onPressed,
      isNotNull,
    );

    await tester.tap(find.text(semanticSysHome));
    await tester.pump();
    expect(calls, [
      [semanticSysHome, '']
    ]);
    expect(find.text('ok'), findsNothing);
    expect(find.text(semanticResultMissing), findsNothing);
    for (final kind in semanticKinds) {
      expect(
        tester
            .widget<TextButton>(find.widgetWithText(TextButton, kind))
            .onPressed,
        isNull,
      );
    }
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('semantic-open_app-arg')))
          .enabled,
      isFalse,
    );

    peer = semanticOk;
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pump();
    expect(find.text('ok'), findsOneWidget);
    expect(find.byKey(const Key('semantic-failure')), findsNothing);
    expect(calls, [
      [semanticSysHome, '']
    ]);
  });

  testWidgets('failure codes are plaintext and enqueue ok is not success',
      (tester) async {
    final calls = <List<String>>[];
    var queued = semanticNoSession;
    var peer = semanticResultMissing;
    var peerReads = 0;
    await tester.pumpWidget(_harness(SemanticActionBar(
      port: SemanticActionPort(
        enqueue: (kind, arg) {
          calls.add([kind, arg]);
          return queued;
        },
        peerResult: () {
          peerReads++;
          return peer;
        },
      ),
      pollInterval: const Duration(milliseconds: 10),
      timeout: const Duration(seconds: 5),
    )));

    await tester.tap(find.text(semanticSysBack));
    await tester.pump();
    expect(peerReads, 0);
    expect(find.text(semanticNoSession), findsOneWidget);
    expect(find.text('ok'), findsNothing);

    queued = semanticInFlight;
    await tester.tap(find.text(semanticSysRecents));
    await tester.pump();
    expect(find.text(semanticInFlight), findsOneWidget);
    expect(find.text('ok'), findsNothing);
    expect(peerReads, 0);

    queued = semanticOk;
    peer = semanticInjectFailed;
    await tester.tap(find.text(semanticSysHome));
    await tester.pump();
    expect(peerReads, 1);
    expect(find.text(semanticInjectFailed), findsOneWidget);
    expect(find.text('ok'), findsNothing);

    peer = semanticUnsupported;
    await tester.tap(find.text(semanticOpenApp));
    await tester.pump();
    expect(find.text(semanticUnsupported), findsOneWidget);
    expect(find.text('ok'), findsNothing);
    expect(calls.last, [semanticOpenApp, '']);
  });

  testWidgets('field confirms stay on the bar and a missed peer is timeout',
      (tester) async {
    final calls = <List<String>>[];
    var clock = DateTime.utc(2026);
    await tester.pumpWidget(_harness(SemanticActionBar(
      port: SemanticActionPort(
        enqueue: (kind, arg) {
          calls.add([kind, arg]);
          return semanticOk;
        },
        peerResult: () => semanticResultMissing,
      ),
      timeout: const Duration(milliseconds: 30),
      pollInterval: const Duration(milliseconds: 10),
      now: () => clock,
      wait: (delay) {
        clock = clock.add(delay);
        return Future<void>.value();
      },
    )));

    await tester.enterText(
      find.byKey(const Key('semantic-ime-arg')),
      semanticImeHide,
    );
    await tester.tap(find.text(semanticIme));
    await tester.pumpAndSettle();
    expect(calls, [
      [semanticIme, semanticImeHide]
    ]);
    expect(find.text(semanticBarTimeout), findsOneWidget);
    expect(find.text('ok'), findsNothing);
    expect(find.byType(Dialog), findsNothing);
    expect(find.byType(SemanticActionBar), findsOneWidget);

    await tester.enterText(
      find.byKey(const Key('semantic-open_app-arg')),
      'com.android.settings',
    );
    await tester.tap(find.text(semanticOpenApp));
    await tester.pumpAndSettle();
    expect(calls.last, [semanticOpenApp, 'com.android.settings']);

    await tester.enterText(
      find.byKey(const Key('semantic-set_text-arg')),
      'hello',
    );
    await tester.tap(find.text(semanticSetText));
    await tester.pumpAndSettle();
    expect(calls.last, [semanticSetText, 'hello']);
    expect(calls.length, 3);
    expect(find.text('ok'), findsNothing);
  });
}

Widget _harness(Widget child) {
  return MaterialApp(
    home: Scaffold(body: child),
  );
}
