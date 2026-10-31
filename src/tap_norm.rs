//! A content-normalized tap on the contain rectangle.
//!
//! The controller reads the session's latest [`ViewportFrame`], maps
//! `(x, y)` in `[0, 1]` to remote content pixels, and sends one [`TapNorm`].
//! The Android peer plays that as a single accessibility gesture and answers
//! with [`TapNormResult`]. The gesture is not a host click, and it is not a
//! stand-in for `sys.*`, `open_app`, or `set_text`.
//!
//! The send shares [`crate::semantic_action::Flight`] with semantic actions.
//! `ok` from the enqueue is not success. Success is a peer result of `ok`.

use std::sync::Mutex;

use base::message_proto::{Message, TapNorm, TapNormResult};
use hbb_common::{log, protobuf::Message as _};

use crate::semantic_action::{self, Flight, INJECT_FAILED, NO_SESSION, OK, UNSUPPORTED};

/// The point is outside the contain rectangle, or the session has no frame.
/// Nothing is sent. The point is not moved onto the content edge.
pub const TAP_MISS: &str = "tap_miss";

/// Remote content placed in the view. `w` and `h` are the content size in
/// remote pixels, `scale` is the uniform contain scale, and the offset is the
/// content origin inside the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportFrame {
    pub w: f64,
    pub h: f64,
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
}

/// What the controller knows before it reads the frame.
#[derive(Clone, Copy)]
pub struct Gate<'a> {
    pub has_sender: bool,
    pub remote_desktop: bool,
    pub input_allowed: bool,
    pub platform: &'a str,
}

fn usable(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

fn frame_usable(frame: &ViewportFrame) -> bool {
    usable(frame.w)
        && usable(frame.h)
        && usable(frame.scale)
        && frame.offset_x.is_finite()
        && frame.offset_y.is_finite()
}

/// Pixel that contains the continuous position `n * size` on `[0, size]`.
/// `n == 1` is the last pixel. `n` outside `[0, 1]` is not a pixel.
fn content_pixel(n: f64, size: f64) -> Option<i32> {
    if !n.is_finite() || !(0.0..=1.0).contains(&n) || !usable(size) {
        return None;
    }
    let max = size.floor() as i32 - 1;
    if max < 0 {
        return None;
    }
    let px = if n >= 1.0 {
        max
    } else {
        (n * size).floor() as i32
    };
    if (0..=max).contains(&px) {
        Some(px)
    } else {
        None
    }
}

/// `(nx, ny)` are relative to the contain content rectangle, not the window.
/// A value outside `[0, 1]` is the letterbox (or past it) and is a miss.
pub fn map_norm(frame: &ViewportFrame, nx: f64, ny: f64) -> Result<(i32, i32), ()> {
    if !frame_usable(frame) || !nx.is_finite() || !ny.is_finite() {
        return Err(());
    }
    if !(0.0..=1.0).contains(&nx) || !(0.0..=1.0).contains(&ny) {
        return Err(());
    }
    let left = frame.offset_x;
    let top = frame.offset_y;
    let right = frame.offset_x + frame.w * frame.scale;
    let bottom = frame.offset_y + frame.h * frame.scale;
    let vx = frame.offset_x + nx * frame.w * frame.scale;
    let vy = frame.offset_y + ny * frame.h * frame.scale;
    if !vx.is_finite() || !vy.is_finite() || vx < left || vx > right || vy < top || vy > bottom {
        return Err(());
    }
    match (content_pixel(nx, frame.w), content_pixel(ny, frame.h)) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(()),
    }
}

/// A view point in pixels. The letterbox is `(px - offset) / (w * scale)`
/// outside `[0, 1]`. That point is a miss, not the nearest content pixel.
pub fn map_view(frame: &ViewportFrame, px: f64, py: f64) -> Result<(i32, i32), ()> {
    if !frame_usable(frame) || !px.is_finite() || !py.is_finite() {
        return Err(());
    }
    let nx = (px - frame.offset_x) / (frame.w * frame.scale);
    let ny = (py - frame.offset_y) / (frame.h * frame.scale);
    map_norm(frame, nx, ny)
}

/// Admit one tap. Reads `slot` now, so a frame noted after the previous tap
/// is the one this call uses. Does not send and does not take the flight slot.
pub fn admit_latest(
    g: &Gate<'_>,
    slot: &Mutex<Option<ViewportFrame>>,
    nx: f64,
    ny: f64,
) -> Result<(i32, i32), &'static str> {
    if !g.has_sender {
        return Err(NO_SESSION);
    }
    if !g.remote_desktop {
        return Err(UNSUPPORTED);
    }
    if !g.input_allowed {
        return Err(INJECT_FAILED);
    }
    let platform = g.platform.trim();
    if !platform.is_empty() && !platform.eq_ignore_ascii_case("android") {
        return Err(UNSUPPORTED);
    }
    let frame = slot.lock().unwrap();
    let Some(frame) = frame.as_ref() else {
        return Err(TAP_MISS);
    };
    map_norm(frame, nx, ny).map_err(|_| TAP_MISS)
}

/// Take the shared flight slot only after the point is admitted.
/// A miss leaves the slot untouched.
pub fn begin_tap(
    flight: &mut Flight,
    admitted: Result<(i32, i32), &'static str>,
) -> Result<(i32, i32), &'static str> {
    let point = admitted?;
    semantic_action::prepare_send(flight, OK)?;
    Ok(point)
}

pub fn tap_message(x: i32, y: i32) -> Message {
    let mut msg = Message::new();
    msg.set_tap_norm(TapNorm {
        x,
        y,
        ..Default::default()
    });
    msg
}

pub fn result_message(code: &str) -> Message {
    let mut msg = Message::new();
    msg.set_tap_norm_result(TapNormResult {
        code: normalize_peer_code(code).to_owned(),
        ..Default::default()
    });
    msg
}

/// Peer codes are a closed set. `tap_miss` stays itself. Anything else the
/// peer invents is an injection failure.
pub fn normalize_peer_code(code: &str) -> &'static str {
    if code.len() > 32 {
        log::error!("norm tap peer code is {} bytes", code.len());
        return INJECT_FAILED;
    }
    match code {
        OK => OK,
        UNSUPPORTED => UNSUPPORTED,
        INJECT_FAILED => INJECT_FAILED,
        TAP_MISS => TAP_MISS,
        _ => {
            log::warn!("norm tap peer code `{code}` is not a known result");
            INJECT_FAILED
        }
    }
}

/// Run an already-admitted tap on this peer.
pub fn execute_on_peer(x: i32, y: i32) -> &'static str {
    #[cfg(target_os = "android")]
    {
        return match scrap::android::call_main_service_norm_tap(x, y) {
            Ok(code) => normalize_peer_code(&code),
            Err(e) => {
                log::error!("norm tap jni failed: {e}");
                INJECT_FAILED
            }
        };
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (x, y);
        UNSUPPORTED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic_action::{
        action_message, is_peer_success, require_peer_result, INJECT_FAILED, IN_FLIGHT, NO_SESSION,
        OK, RESULT_MISSING, SYS_BACK, UNSUPPORTED,
    };
    fn ready<'a>() -> Gate<'a> {
        Gate {
            has_sender: true,
            remote_desktop: true,
            input_allowed: true,
            platform: "Android",
        }
    }

    /// 1080x2400 phone letterboxed in an 800x600 view. Scale 0.25, origin x = 265.
    fn portrait() -> ViewportFrame {
        ViewportFrame {
            w: 1080.0,
            h: 2400.0,
            scale: 0.25,
            offset_x: 265.0,
            offset_y: 0.0,
        }
    }

    /// The same view after the phone rotates to 2400x1080.
    fn landscape() -> ViewportFrame {
        let scale = 800.0 / 2400.0;
        ViewportFrame {
            w: 2400.0,
            h: 1080.0,
            scale,
            offset_x: 0.0,
            offset_y: (600.0 - 1080.0 * scale) / 2.0,
        }
    }

    #[test]
    fn contain_rect_maps_to_remote_pixels() {
        let frame = portrait();
        assert_eq!(map_norm(&frame, 0.0, 0.0).unwrap(), (0, 0));
        assert_eq!(map_norm(&frame, 0.5, 0.25).unwrap(), (540, 600));
        assert_eq!(map_norm(&frame, 1.0, 1.0).unwrap(), (1079, 2399));
        // The same content point, expressed as a view pixel, lands on the same peer pixel.
        assert_eq!(map_view(&frame, 400.0, 150.0).unwrap(), (540, 600));
        assert_eq!(map_view(&frame, 535.0, 150.0).unwrap(), (1079, 600));
    }

    #[test]
    fn letterbox_is_a_miss_and_is_not_the_content_edge() {
        let frame = portrait();
        let edge = map_norm(&frame, 0.0, 0.25).unwrap();
        assert_eq!(edge, (0, 600));
        assert!(map_view(&frame, 0.0, 150.0).is_err());
        assert!(map_view(&frame, 536.0, 150.0).is_err());
        assert!(map_norm(&frame, -0.001, 0.25).is_err());
        assert!(map_norm(&frame, 1.001, 0.25).is_err());
        assert!(map_norm(&frame, f64::NAN, 0.5).is_err());
        assert!(map_norm(&frame, f64::INFINITY, 0.5).is_err());
        let broken = ViewportFrame {
            scale: 0.0,
            ..frame
        };
        assert!(map_norm(&broken, 0.5, 0.25).is_err());
    }

    #[test]
    fn remap_uses_the_frame_stored_now() {
        let g = ready();
        let slot = Mutex::new(Some(portrait()));
        assert_eq!(admit_latest(&g, &slot, 0.5, 0.25).unwrap(), (540, 600));
        *slot.lock().unwrap() = Some(landscape());
        assert_eq!(admit_latest(&g, &slot, 0.5, 0.25).unwrap(), (1200, 270));
        assert!(map_view(slot.lock().unwrap().as_ref().unwrap(), 100.0, 0.0).is_err());
        *slot.lock().unwrap() = None;
        assert_eq!(admit_latest(&g, &slot, 0.5, 0.25), Err(TAP_MISS));
    }

    #[test]
    fn failure_codes_stay_distinct() {
        let g = ready();
        let slot = Mutex::new(Some(portrait()));
        assert_eq!(
            admit_latest(
                &Gate {
                    has_sender: false,
                    ..g
                },
                &slot,
                0.5,
                0.25
            ),
            Err(NO_SESSION)
        );
        assert_eq!(
            admit_latest(
                &Gate {
                    remote_desktop: false,
                    ..g
                },
                &slot,
                0.5,
                0.25
            ),
            Err(UNSUPPORTED)
        );
        assert_eq!(
            admit_latest(
                &Gate {
                    input_allowed: false,
                    ..g
                },
                &slot,
                0.5,
                0.25
            ),
            Err(INJECT_FAILED)
        );
        assert_eq!(
            admit_latest(
                &Gate {
                    platform: "Windows",
                    ..g
                },
                &slot,
                0.5,
                0.25
            ),
            Err(UNSUPPORTED)
        );
        assert_eq!(admit_latest(&g, &slot, -0.1, 0.5), Err(TAP_MISS));
        assert_eq!(normalize_peer_code("ok"), OK);
        assert_eq!(normalize_peer_code("unsupported"), UNSUPPORTED);
        assert_eq!(normalize_peer_code("inject_failed"), INJECT_FAILED);
        assert_eq!(normalize_peer_code("tap_miss"), TAP_MISS);
        assert_eq!(normalize_peer_code("no_session"), INJECT_FAILED);
        assert_eq!(normalize_peer_code(&"x".repeat(33)), INJECT_FAILED);
        #[cfg(not(target_os = "android"))]
        assert_eq!(execute_on_peer(1, 2), UNSUPPORTED);
    }

    #[test]
    fn tap_shares_the_semantic_flight_and_waits_for_the_peer() {
        let g = ready();
        let slot = Mutex::new(Some(portrait()));
        let mut flight = Flight::default();
        let mut sent = Vec::new();

        let miss = begin_tap(&mut flight, admit_latest(&g, &slot, -0.2, 0.5));
        assert_eq!(miss, Err(TAP_MISS));
        assert_eq!(flight.result(), RESULT_MISSING);

        semantic_action::prepare_send(&mut flight, OK).unwrap();
        sent.push(action_message(SYS_BACK, ""));
        assert_eq!(sent.len(), 1);
        assert_eq!(flight.result(), RESULT_MISSING);
        assert!(!is_peer_success(flight.result()));

        let blocked = begin_tap(&mut flight, admit_latest(&g, &slot, 0.5, 0.25));
        assert_eq!(blocked, Err(IN_FLIGHT));
        assert_eq!(sent.len(), 1);
        assert_eq!(require_peer_result(""), RESULT_MISSING);

        let still_miss = begin_tap(&mut flight, admit_latest(&g, &slot, 1.5, 0.5));
        assert_eq!(still_miss, Err(TAP_MISS));
        assert_eq!(flight.result(), RESULT_MISSING);

        flight.finish("ok");
        assert!(is_peer_success(flight.result()));

        let point = begin_tap(&mut flight, admit_latest(&g, &slot, 0.5, 0.25)).unwrap();
        sent.push(tap_message(point.0, point.1));
        assert_eq!(sent.len(), 2);
        assert_eq!(flight.result(), RESULT_MISSING);
        assert!(!is_peer_success(flight.result()));
        assert_eq!(
            semantic_action::prepare_send(&mut flight, OK),
            Err(IN_FLIGHT)
        );

        flight.finish_known(normalize_peer_code("ok"));
        assert!(is_peer_success(flight.result()));
        flight.finish_known(TAP_MISS);
        assert!(!is_peer_success(flight.result()));
        assert_eq!(flight.result(), TAP_MISS);
    }

    #[test]
    fn tap_message_is_a_session_tap_not_a_mouse_or_semantic_event() {
        let msg = tap_message(540, 600);
        let bytes = msg.write_to_bytes().unwrap();
        let parsed = Message::parse_from_bytes(&bytes).unwrap();
        match parsed.union {
            Some(base::message_proto::message::Union::TapNorm(tap)) => {
                assert_eq!(tap.x, 540);
                assert_eq!(tap.y, 600);
            }
            other => panic!("expected TapNorm, got {other:?}"),
        }
        let reply = result_message("tap_miss");
        match reply.union {
            Some(base::message_proto::message::Union::TapNormResult(result)) => {
                assert_eq!(result.code, TAP_MISS);
            }
            other => panic!("expected TapNormResult, got {other:?}"),
        }
    }
}
