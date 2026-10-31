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
}
