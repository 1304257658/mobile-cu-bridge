import 'dart:math' as math;

/// Origin of the fitted content inside the view, in view pixels.
class ViewportOffset {
  final double x;
  final double y;

  const ViewportOffset(this.x, this.y);
}

/// Remote frame placed in a view with a uniform contain (letterbox) fit.
///
/// [w] and [h] are the remote content size. [scale] maps that content onto
/// the view, and [offset] is the content origin inside the view. A later
/// normalized tap `(nx, ny)` on the fitted rectangle is `nx * w`, `ny * h`
/// in remote pixels; a view point is outside the content when
/// `(px - offset.x) / (w * scale)` is outside `[0, 1]` (same for y).
class ViewportFrame {
  final double w;
  final double h;
  final double scale;
  final ViewportOffset offset;

  const ViewportFrame({
    required this.w,
    required this.h,
    required this.scale,
    required this.offset,
  });
}

bool _usable(double v) => v.isFinite && v > 0;

/// Whether an Android session letterboxes for [style].
///
/// The desktop client writes `original` as the default for a new peer. That
/// value, an unset style, and `adaptive` all contain-fit. `custom` stays an
/// explicit zoom.
bool androidViewUsesContain(String style) => style != 'custom';

/// Scale [contentW] x [contentH] uniformly so it fits inside the view.
///
/// The same scale is used on both axes, so the content is not stretched.
/// Neither fitted axis exceeds the view, so the content is not cropped.
/// Empty margins are split on the longer axis.
ViewportFrame containFit({
  required double contentW,
  required double contentH,
  required double viewW,
  required double viewH,
}) {
  if (!_usable(contentW) ||
      !_usable(contentH) ||
      !_usable(viewW) ||
      !_usable(viewH)) {
    return ViewportFrame(
      w: contentW,
      h: contentH,
      scale: 1,
      offset: const ViewportOffset(0, 0),
    );
  }
  final scale = math.min(viewW / contentW, viewH / contentH);
  final fittedW = contentW * scale;
  final fittedH = contentH * scale;
  return ViewportFrame(
    w: contentW,
    h: contentH,
    scale: scale,
    offset: ViewportOffset(
      math.max(0.0, (viewW - fittedW) / 2),
      math.max(0.0, (viewH - fittedH) / 2),
    ),
  );
}

/// Remote content pixel for a normalized tap. Null is a miss.
class NormTapPoint {
  final int x;
  final int y;

  const NormTapPoint(this.x, this.y);
}

/// The point is outside the contain rectangle. It is not moved to the edge.
const String tapMiss = 'tap_miss';

bool _frameUsable(ViewportFrame frame) =>
    _usable(frame.w) &&
    _usable(frame.h) &&
    _usable(frame.scale) &&
    frame.offset.x.isFinite &&
    frame.offset.y.isFinite;

int? _contentPixel(double n, double size) {
  if (!n.isFinite || n < 0 || n > 1 || !_usable(size)) return null;
  final max = size.floor() - 1;
  if (max < 0) return null;
  final px = n >= 1 ? max : (n * size).floor();
  if (px < 0 || px > max) return null;
  return px;
}

/// [nx] and [ny] are relative to the contain content rectangle, not the window.
/// A value outside `[0, 1]` is the letterbox and is a miss.
NormTapPoint? mapNormTap(ViewportFrame frame, double nx, double ny) {
  if (!_frameUsable(frame) || !nx.isFinite || !ny.isFinite) return null;
  if (nx < 0 || nx > 1 || ny < 0 || ny > 1) return null;
  final left = frame.offset.x;
  final top = frame.offset.y;
  final right = frame.offset.x + frame.w * frame.scale;
  final bottom = frame.offset.y + frame.h * frame.scale;
  final vx = frame.offset.x + nx * frame.w * frame.scale;
  final vy = frame.offset.y + ny * frame.h * frame.scale;
  if (!vx.isFinite ||
      !vy.isFinite ||
      vx < left ||
      vx > right ||
      vy < top ||
      vy > bottom) {
    return null;
  }
  final x = _contentPixel(nx, frame.w);
  final y = _contentPixel(ny, frame.h);
  if (x == null || y == null) return null;
  return NormTapPoint(x, y);
}

/// A view point in pixels. The letterbox is outside the content rectangle.
NormTapPoint? mapViewTap(ViewportFrame frame, double px, double py) {
  if (!_frameUsable(frame) || !px.isFinite || !py.isFinite) return null;
  final nx = (px - frame.offset.x) / (frame.w * frame.scale);
  final ny = (py - frame.offset.y) / (frame.h * frame.scale);
  return mapNormTap(frame, nx, ny);
}
