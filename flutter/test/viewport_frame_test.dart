import 'package:flutter_hbb/models/viewport_frame.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('containFit', () {
    test('letterboxes a portrait phone inside a landscape view', () {
      final frame = containFit(
        contentW: 1080,
        contentH: 2400,
        viewW: 800,
        viewH: 600,
      );

      expect(frame.w, 1080);
      expect(frame.h, 2400);
      expect(frame.scale, 600 / 2400);
      expect(frame.offset.y, 0);
      expect(frame.offset.x, closeTo((800 - 1080 * frame.scale) / 2, 1e-9));

      final fittedW = frame.w * frame.scale;
      final fittedH = frame.h * frame.scale;
      expect(fittedW / fittedH, closeTo(1080 / 2400, 1e-9));
      expect(fittedW, lessThanOrEqualTo(800));
      expect(fittedH, lessThanOrEqualTo(600));
      expect(frame.scale, lessThan(800 / 1080));
    });

    test('letterboxes wide content on the vertical axis', () {
      final frame = containFit(
        contentW: 1920,
        contentH: 1080,
        viewW: 400,
        viewH: 800,
      );

      expect(frame.scale, 400 / 1920);
      expect(frame.offset.x, 0);
      expect(frame.offset.y, closeTo((800 - 1080 * frame.scale) / 2, 1e-9));
      expect(frame.w * frame.scale, lessThanOrEqualTo(400));
      expect(frame.h * frame.scale, lessThanOrEqualTo(800));
    });

    test('fills the view when the aspect already matches', () {
      final frame = containFit(
        contentW: 200,
        contentH: 100,
        viewW: 400,
        viewH: 200,
      );

      expect(frame.scale, 2);
      expect(frame.offset.x, 0);
      expect(frame.offset.y, 0);
      expect(frame.w * frame.scale, 400);
      expect(frame.h * frame.scale, 200);
    });

    test('treats Android original and unset styles as contain', () {
      expect(androidViewUsesContain(''), isTrue);
      expect(androidViewUsesContain('original'), isTrue);
      expect(androidViewUsesContain('adaptive'), isTrue);
      expect(androidViewUsesContain('custom'), isFalse);
    });

    test('does not invent a scale for an empty view or content', () {
      final frame = containFit(
        contentW: 1080,
        contentH: 0,
        viewW: 800,
        viewH: 600,
      );

      expect(frame.w, 1080);
      expect(frame.h, 0);
      expect(frame.scale, 1);
      expect(frame.offset.x, 0);
      expect(frame.offset.y, 0);

      final emptyView = containFit(
        contentW: 1080,
        contentH: 2400,
        viewW: double.nan,
        viewH: 600,
      );
      expect(emptyView.scale, 1);
      expect(emptyView.offset.x, 0);
      expect(emptyView.offset.y, 0);
    });
  });

  group('norm tap', () {
    ViewportFrame phoneInView() => containFit(
          contentW: 1080,
          contentH: 2400,
          viewW: 800,
          viewH: 600,
        );

    test('an interior content point is that remote pixel', () {
      final frame = phoneInView();
      final point = mapNormTap(frame, 0.5, 0.25);

      expect(point, isNotNull);
      expect(point!.x, 540);
      expect(point.y, 600);
      expect(mapNormTap(frame, 0, 0), const NormTapPoint(0, 0));
      expect(mapNormTap(frame, 1, 1), const NormTapPoint(1079, 2399));
      expect(mapViewTap(frame, 400, 150), const NormTapPoint(540, 600));
      expect(mapViewTap(frame, 535, 150), const NormTapPoint(1079, 600));
    });

    test('a letterbox point is tap_miss and is not the content edge', () {
      final frame = phoneInView();
      final edge = mapNormTap(frame, 0, 0.25);

      expect(edge, const NormTapPoint(0, 600));
      expect(mapViewTap(frame, 0, 150), isNull);
      expect(mapViewTap(frame, 536, 150), isNull);
      expect(mapNormTap(frame, -0.001, 0.25), isNull);
      expect(mapNormTap(frame, 1.001, 0.25), isNull);
      expect(mapNormTap(frame, double.nan, 0.5), isNull);
      expect(tapMiss, 'tap_miss');
    });

    test('a rotated frame maps the same norm to the new pixel', () {
      final portrait = phoneInView();
      expect(mapNormTap(portrait, 0.5, 0.25), const NormTapPoint(540, 600));

      final landscape = containFit(
        contentW: 2400,
        contentH: 1080,
        viewW: 800,
        viewH: 600,
      );
      expect(mapNormTap(landscape, 0.5, 0.25), const NormTapPoint(1200, 270));
      expect(mapViewTap(landscape, 100, 0), isNull);
    });
  });
}
