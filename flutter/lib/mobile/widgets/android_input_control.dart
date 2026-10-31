import 'package:flutter/material.dart';

import '../../common.dart';

const androidInputServicePathTip =
    'Accessibility → Installed services → RustDesk Input';
const androidInputRestrictedSettingsTip =
    'App info → RustDesk → ⋮ → 允许受限制的设置 / 允许受限设置, then retry overlay and accessibility.';

bool isOppoFamilyOem(String manufacturer, String brand) {
  final haystack = '${manufacturer.trim()} ${brand.trim()}'.toLowerCase();
  const markers = ['oppo', 'oneplus', 'realme', 'oplus', 'coloros'];
  return markers.any(haystack.contains);
}

bool showAndroidInputControlHints({
  required bool inputEnabled,
  required bool launchFailed,
  required String manufacturer,
  required String brand,
}) {
  if (inputEnabled) return false;
  return launchFailed || isOppoFamilyOem(manufacturer, brand);
}

class AndroidInputControlHints extends StatelessWidget {
  const AndroidInputControlHints({
    Key? key,
    required this.inputEnabled,
    required this.launchFailed,
    required this.manufacturer,
    required this.brand,
  }) : super(key: key);

  final bool inputEnabled;
  final bool launchFailed;
  final String manufacturer;
  final String brand;

  @override
  Widget build(BuildContext context) {
    if (!showAndroidInputControlHints(
      inputEnabled: inputEnabled,
      launchFailed: launchFailed,
      manufacturer: manufacturer,
      brand: brand,
    )) {
      return const SizedBox.shrink();
    }
    final style = Theme.of(context)
        .textTheme
        .bodySmall
        ?.copyWith(color: MyTheme.darkGray);
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(translate(androidInputServicePathTip), style: style),
          const SizedBox(height: 4),
          Text(translate(androidInputRestrictedSettingsTip), style: style),
        ],
      ),
    );
  }
}
