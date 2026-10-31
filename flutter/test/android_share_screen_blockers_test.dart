import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/generated_bridge.dart';
import 'package:flutter_hbb/mobile/widgets/android_input_control.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/utils/android_documents.dart';
import 'package:flutter_test/flutter_test.dart';

class _Binding extends Fake implements RustdeskImpl {
  @override
  String translate(
          {required String name, required String locale, dynamic hint}) =>
      name;
}

void main() {
  setUp(() {
    platformFFI.setBindingForTesting(_Binding());
  });

  test('app-private logs do not ask for a folder', () async {
    expect(await prepareAndroidLogFolder(() async => true), isTrue);
  });

  test('a log directory that is not ready is not a folder choice', () async {
    await expectLater(
      prepareAndroidLogFolder(() async => false),
      throwsA(isA<StateError>()),
    );
    await expectLater(
      prepareAndroidLogFolder(() async => null),
      throwsA(isA<StateError>()),
    );
  });

  test('oppo-family devices are the restricted-settings case', () {
    expect(isOppoFamilyOem('OPPO', 'OPPO'), isTrue);
    expect(isOppoFamilyOem('OnePlus', 'OnePlus'), isTrue);
    expect(isOppoFamilyOem('realme', 'realme'), isTrue);
    expect(isOppoFamilyOem('oplus', 'ColorOS'), isTrue);
    expect(isOppoFamilyOem('Google', 'google'), isFalse);
    expect(isOppoFamilyOem('', ''), isFalse);
  });

  testWidgets('input control shows the service path and restricted settings',
      (tester) async {
    Future<void> pump({
      required bool inputEnabled,
      required bool launchFailed,
      required String manufacturer,
      required String brand,
    }) {
      return tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: AndroidInputControlHints(
            inputEnabled: inputEnabled,
            launchFailed: launchFailed,
            manufacturer: manufacturer,
            brand: brand,
          ),
        ),
      ));
    }

    await pump(
      inputEnabled: false,
      launchFailed: false,
      manufacturer: 'OPPO',
      brand: 'OPPO',
    );
    expect(find.text(androidInputServicePathTip), findsOneWidget);
    expect(find.text(androidInputRestrictedSettingsTip), findsOneWidget);
    _expectNoDeveloperMode(tester);

    await pump(
      inputEnabled: false,
      launchFailed: true,
      manufacturer: 'Google',
      brand: 'google',
    );
    expect(find.text(androidInputServicePathTip), findsOneWidget);
    expect(find.text(androidInputRestrictedSettingsTip), findsOneWidget);

    await pump(
      inputEnabled: true,
      launchFailed: true,
      manufacturer: 'OPPO',
      brand: 'OPPO',
    );
    expect(find.text(androidInputRestrictedSettingsTip), findsNothing);
    expect(find.text(androidInputServicePathTip), findsNothing);

    await pump(
      inputEnabled: false,
      launchFailed: false,
      manufacturer: 'Google',
      brand: 'google',
    );
    expect(find.text(androidInputRestrictedSettingsTip), findsNothing);
  });
}

void _expectNoDeveloperMode(WidgetTester tester) {
  final joined = tester
      .widgetList<Text>(find.byType(Text))
      .map((text) => text.data ?? '')
      .join('\n')
      .toLowerCase();
  expect(joined, isNot(contains('developer')));
  expect(joined, isNot(contains('开发者')));
  expect(joined, contains('允许受限制的设置'));
  expect(joined, contains('允许受限设置'));
  expect(joined, contains('rustdesk input'));
}
