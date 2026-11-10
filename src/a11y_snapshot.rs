//! Foreground accessibility snapshot and click-by-ref.
//!
//! The controller sends `snapshot` (empty arg) or `click` (one ref, or a
//! compact JSON array of refs). The peer walks the active window, assigns
//! refs, and later clicks those nodes. It does not launch an activity.
//! Package and MAIN/LAUNCHER stay on `open_app`.
//!
//! Every limit is read from `flutter/android/app/src/main/assets/a11y_snapshot_caps.json`.
//! A snapshot that exceeds the payload, the node cap, or the ref count is `ok`
//! with `truncated: true` and `reason` `payload`, `nodes`, or `refs`. Anything
//! that cannot produce a snapshot is `snapshot_failed`. An unknown, expired,
//! previous-generation, or moved ref is `ref_stale`. No other result code is
//! added for this path.
//!
//! Snapshot and click share one in-flight lease (`flight_lease_ms`). A call
//! that arrives while the lease is still held returns `snapshot_failed` and
//! starts no thread. After the lease, the next call reclaims the slot, marks
//! the stuck worker abandoned, and drops that worker's late result. An
//! abandoned worker does not clear the new lease. At most one abandoned
//! worker stays alive. When that one is still alive and the current lease has
//! also expired, the call returns `snapshot_failed` with detail
//! `a11y_wedged` and does not start a third thread. The thread count is only
//! in the log. A stuck JNI call that later returns drops its own slot, and
//! the next request can succeed. There is no process restart. Refs look like
//! `g7.n3`: the generation changes on every installed snapshot. Only
//! clickable nodes get a ref, and at most 80 of them. Other nodes stay in the
//! tree with their text and no ref.

use crate::semantic_action::{
    result_message, result_message_with_detail, validate, CLICK, OK, REF_STALE, SNAPSHOT,
    SNAPSHOT_FAILED, UNSUPPORTED,
};
use base::message_proto::Message;
use hbb_common::log;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

/// Current worker plus at most one abandoned worker. The timeout reply is not a thread.
const A11Y_THREAD_CAP: usize = 2;

/// Exact `detail` when the abandoned slot is full and the current lease is
/// expired. The controller matches this string. Thread count stays in the log.
pub const A11Y_WEDGED: &str = "a11y_wedged";

const CAPS_JSON: &str =
    include_str!("../flutter/android/app/src/main/assets/a11y_snapshot_caps.json");

#[derive(Clone, Copy)]
pub struct Caps {
    /// Clickable nodes that receive a ref. Not a string length.
    pub max_refs: usize,
    pub max_nodes: usize,
    pub payload_max_bytes: usize,
    pub snapshot_timeout_ms: u64,
    pub click_timeout_ms: u64,
    pub ref_ttl_ms: u64,
    pub flight_lease_ms: u64,
    pub batch_max: usize,
}

#[derive(Clone)]
pub struct RawNode {
    pub class_name: String,
    pub text: String,
    pub desc: String,
    pub view_id: String,
    pub package_name: String,
    pub clickable: bool,
    pub bounds: [i32; 4],
    pub path: Vec<u16>,
}

struct Collected {
    nodes: Vec<RawNode>,
    truncated: bool,
    reason: &'static str,
}

#[derive(Clone)]
struct Target {
    path: Vec<u16>,
    bounds: [i32; 4],
}

struct Entry {
    expires: Instant,
    target: Target,
}

struct RefTable {
    entries: HashMap<String, Entry>,
}

pub struct Outcome {
    pub code: &'static str,
    pub detail: String,
}

impl Outcome {
    fn fail() -> Self {
        Self {
            code: SNAPSHOT_FAILED,
            detail: String::new(),
        }
    }
}

pub struct Peer {
    caps: Caps,
    flight: Arc<Mutex<Flight>>,
    live_workers: Arc<AtomicUsize>,
    peak_workers: Arc<AtomicUsize>,
    spawns: Arc<AtomicUsize>,
    gen: AtomicU64,
    table: Mutex<RefTable>,
    #[cfg(test)]
    publish_gap: Mutex<Option<Box<dyn Fn() + Send>>>,
}

enum Refuse {
    Held,
    Wedged,
}

struct Flight {
    next: u64,
    holder: u64,
    since: Instant,
    abandoned: u64,
}

struct Permit {
    token: u64,
    counted: bool,
    settled: bool,
    flight: Arc<Mutex<Flight>>,
    live: Arc<AtomicUsize>,
}

impl Permit {
    fn token(&self) -> u64 {
        self.token
    }

    /// Drop the lease. `true` only when this worker still owns it, so a late
    /// result from an abandoned worker is not delivered and does not clear
    /// the replacement lease.
    fn conclude(&mut self) -> bool {
        if self.settled {
            return false;
        }
        self.settled = true;
        if self.counted {
            self.live.fetch_sub(1, Ordering::AcqRel);
            self.counted = false;
        }
        let mut flight = self.flight.lock().unwrap();
        if flight.abandoned == self.token {
            flight.abandoned = 0;
            return false;
        }
        if flight.holder == self.token {
            flight.holder = 0;
            return true;
        }
        false
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let _ = self.conclude();
    }
}

pub fn caps() -> Result<Caps, String> {
    static CACHED: OnceLock<Result<Caps, String>> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            parse_caps(CAPS_JSON).map_err(|e| {
                log::error!("semantic snapshot caps rejected: {e}");
                e
            })
        })
        .clone()
}

pub fn click_arg_verdict(arg: &str) -> &'static str {
    let caps = match caps() {
        Ok(caps) => caps,
        Err(_) => return SNAPSHOT_FAILED,
    };
    match parse_click_arg(arg, &caps) {
        Ok(_) => OK,
        Err(why) => {
            log::error!("semantic click refused: {why}");
            SNAPSHOT_FAILED
        }
    }
}

pub fn accept_snapshot_detail(detail: &str) -> Result<String, ()> {
    match check_detail(detail) {
        Ok(()) => Ok(detail.to_owned()),
        Err(why) => {
            log::error!("semantic snapshot payload rejected: {why}");
            Err(())
        }
    }
}

fn refuse_outcome(why: Refuse) -> Outcome {
    match why {
        Refuse::Held => Outcome::fail(),
        Refuse::Wedged => Outcome {
            code: SNAPSHOT_FAILED,
            detail: A11Y_WEDGED.to_owned(),
        },
    }
}

fn peer() -> Option<&'static Peer> {
    static PEER: OnceLock<Option<Peer>> = OnceLock::new();
    PEER.get_or_init(|| caps().ok().map(Peer::new)).as_ref()
}

impl Peer {
    pub fn new(caps: Caps) -> Self {
        Self {
            caps,
            flight: Arc::new(Mutex::new(Flight {
                next: 0,
                holder: 0,
                since: Instant::now(),
                abandoned: 0,
            })),
            live_workers: Arc::new(AtomicUsize::new(0)),
            peak_workers: Arc::new(AtomicUsize::new(0)),
            spawns: Arc::new(AtomicUsize::new(0)),
            gen: AtomicU64::new(0),
            table: Mutex::new(RefTable {
                entries: HashMap::new(),
            }),
            #[cfg(test)]
            publish_gap: Mutex::new(None),
        }
    }

    fn threads_started(&self) -> usize {
        self.spawns.load(Ordering::Acquire)
    }

    fn worker_peak(&self) -> usize {
        self.peak_workers.load(Ordering::Acquire)
    }

    fn acquire(&self, now: Instant) -> Result<Permit, Refuse> {
        let mut flight = self.flight.lock().unwrap();
        let lease = Duration::from_millis(self.caps.flight_lease_ms);
        let live = self.live_workers.load(Ordering::Acquire);
        if flight.holder != 0 {
            let expired = now.saturating_duration_since(flight.since) >= lease;
            if !expired {
                log::error!("semantic a11y refused: one action is already running");
                return Err(Refuse::Held);
            }
            if flight.abandoned != 0 || live >= A11Y_THREAD_CAP {
                let wedged = flight.abandoned != 0;
                drop(flight);
                log::error!("a11y wedged threads={live}");
                return Err(if wedged { Refuse::Wedged } else { Refuse::Held });
            }
            flight.abandoned = flight.holder;
        } else if live >= A11Y_THREAD_CAP {
            log::error!("a11y wedged threads={live}");
            return Err(Refuse::Held);
        }
        flight.next = flight.next.saturating_add(1);
        let token = flight.next;
        flight.holder = token;
        flight.since = now;
        Ok(Permit {
            token,
            counted: false,
            settled: false,
            flight: Arc::clone(&self.flight),
            live: Arc::clone(&self.live_workers),
        })
    }

    fn still_current(&self, token: u64) -> bool {
        self.flight.lock().unwrap().holder == token
    }

    /// Start at most one new worker. `Err` starts nothing.
    fn try_spawn<F>(&self, now: Instant, work: F) -> Result<(), Refuse>
    where
        F: FnOnce(Permit) + Send + 'static,
    {
        let mut permit = self.acquire(now)?;
        let n = self.live_workers.fetch_add(1, Ordering::AcqRel) + 1;
        self.peak_workers.fetch_max(n, Ordering::AcqRel);
        permit.counted = true;
        self.spawns.fetch_add(1, Ordering::AcqRel);
        match thread::Builder::new()
            .name("a11y-action".into())
            .spawn(move || work(permit))
        {
            Ok(_) => Ok(()),
            Err(e) => {
                self.spawns.fetch_sub(1, Ordering::AcqRel);
                log::error!("semantic a11y thread failed: {e}");
                Err(Refuse::Held)
            }
        }
    }

    fn snapshot_with<F>(&self, now: Instant, collect: F) -> Outcome
    where
        F: FnOnce() -> Result<Collected, &'static str> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let (ack_tx, ack_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        if let Err(why) = self.try_spawn(now, move |mut permit| {
            let token = permit.token();
            let result = collect();
            let _ = tx.send((token, result));
            let _ = ack_rx.recv();
            permit.conclude();
            let _ = done_tx.send(());
        }) {
            return refuse_outcome(why);
        }
        let timeout = Duration::from_millis(self.caps.snapshot_timeout_ms);
        match rx.recv_timeout(timeout) {
            Ok((token, Ok(collected))) => {
                let outcome = self
                    .publish_if_current(token, collected, now)
                    .unwrap_or_else(Outcome::fail);
                let _ = ack_tx.send(());
                let _ = done_rx.recv();
                outcome
            }
            Ok((_, Err(code))) => {
                let _ = ack_tx.send(());
                let _ = done_rx.recv();
                Outcome {
                    code,
                    detail: String::new(),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                log::error!(
                    "semantic snapshot exceeded {} ms",
                    self.caps.snapshot_timeout_ms
                );
                drop(ack_tx);
                Outcome::fail()
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                log::error!("semantic snapshot walk ended without a result");
                drop(ack_tx);
                Outcome::fail()
            }
        }
    }

    fn click_at<F>(&self, arg: &str, now: Instant, mut perform: F) -> &'static str
    where
        F: FnMut(&Target) -> &'static str,
    {
        let targets = match self.resolve(arg, now) {
            Ok(targets) => targets,
            Err(code) => return code,
        };
        let mut permit = match self.acquire(now) {
            Ok(permit) => permit,
            Err(_) => return SNAPSHOT_FAILED,
        };
        let code = perform_all(&targets, &mut perform);
        if permit.conclude() {
            code
        } else {
            SNAPSHOT_FAILED
        }
    }

    fn click_timed<F>(&self, arg: &str, now: Instant, mut perform: F) -> &'static str
    where
        F: FnMut(&Target) -> &'static str + Send + 'static,
    {
        let targets = match self.resolve(arg, now) {
            Ok(targets) => targets,
            Err(code) => return code,
        };
        let (tx, rx) = mpsc::channel();
        if self
            .try_spawn(now, move |mut permit| {
                let code = perform_all(&targets, &mut perform);
                if permit.conclude() {
                    let _ = tx.send(code);
                }
            })
            .is_err()
        {
            return SNAPSHOT_FAILED;
        }
        match rx.recv_timeout(Duration::from_millis(self.caps.click_timeout_ms)) {
            Ok(code) => code,
            Err(_) => {
                log::error!("semantic click exceeded {} ms", self.caps.click_timeout_ms);
                SNAPSHOT_FAILED
            }
        }
    }

    fn publish_if_current(
        &self,
        token: u64,
        collected: Collected,
        now: Instant,
    ) -> Option<Outcome> {
        if !self.still_current(token) {
            return None;
        }
        #[cfg(test)]
        {
            let hook = self.publish_gap.lock().unwrap().take();
            if let Some(hook) = hook {
                hook();
            }
        }
        let gen = self.gen.load(Ordering::Relaxed).saturating_add(1);
        let built = match build_snapshot(&collected, &self.caps, gen) {
            Ok(built) => built,
            Err(()) => return self.fail_if_current(token),
        };
        let expires = match now.checked_add(Duration::from_millis(self.caps.ref_ttl_ms)) {
            Some(expires) => expires,
            None => {
                log::error!("semantic snapshot ref expiry overflowed");
                return self.fail_if_current(token);
            }
        };
        let flight = self.flight.lock().unwrap();
        let mut table = self.table.lock().unwrap();
        if flight.holder != token {
            return None;
        }
        if !built.reason.is_empty() {
            log::warn!(
                "semantic snapshot truncated: {} refs, reason {}",
                built.refs.len(),
                built.reason
            );
        }
        table.entries.clear();
        for (r, target) in built.refs {
            table.entries.insert(r, Entry { expires, target });
        }
        self.gen.store(gen, Ordering::Release);
        Some(Outcome {
            code: OK,
            detail: built.detail,
        })
    }

    fn fail_if_current(&self, token: u64) -> Option<Outcome> {
        self.still_current(token).then(Outcome::fail)
    }
}

fn perform_all(
    targets: &[Target],
    perform: &mut impl FnMut(&Target) -> &'static str,
) -> &'static str {
    for target in targets {
        let code = perform(target);
        if code != OK {
            if code != REF_STALE {
                log::error!("semantic click action failed");
            }
            return if code == REF_STALE {
                REF_STALE
            } else {
                SNAPSHOT_FAILED
            };
        }
    }
    OK
}

impl Peer {
    fn resolve(&self, arg: &str, now: Instant) -> Result<Vec<Target>, &'static str> {
        let refs = match parse_click_arg(arg, &self.caps) {
            Ok(refs) => refs,
            Err(why) => {
                log::error!("semantic click refused: {why}");
                return Err(SNAPSHOT_FAILED);
            }
        };
        let mut table = self.table.lock().unwrap();
        let mut targets = Vec::with_capacity(refs.len());
        for r in &refs {
            match table.take_live(r, now) {
                Some(target) => targets.push(target),
                None => {
                    log::warn!("semantic click ref stale: {r}");
                    return Err(REF_STALE);
                }
            }
        }
        Ok(targets)
    }
}

struct Built {
    detail: String,
    reason: &'static str,
    refs: Vec<(String, Target)>,
}

fn build_snapshot(collected: &Collected, caps: &Caps, gen: u64) -> Result<Built, ()> {
    let reserve = envelope_len("payload");
    if reserve > caps.payload_max_bytes {
        log::error!(
            "semantic snapshot cap {} cannot hold the envelope",
            caps.payload_max_bytes
        );
        return Err(());
    }
    let mut parts = Vec::new();
    let mut refs = Vec::new();
    let mut used = reserve;
    let mut reason = "";
    let mut stopped = false;
    let mut omitted_ref = false;
    let mut visited = 0usize;
    for node in &collected.nodes {
        if visited == caps.max_nodes {
            reason = "nodes";
            stopped = true;
            break;
        }
        visited += 1;
        if node.path.len() > caps.max_nodes {
            log::error!("semantic snapshot path is {} indexes", node.path.len());
            return Err(());
        }
        let r = if node.clickable && refs.len() < caps.max_refs {
            Some(format!("g{gen}.n{}", refs.len()))
        } else {
            if node.clickable {
                omitted_ref = true;
            }
            None
        };
        let comma = usize::from(!parts.is_empty());
        let room = caps.payload_max_bytes - used - comma;
        let Some((json, shrunk)) = fit_node(node, r.as_deref(), room) else {
            reason = "payload";
            stopped = true;
            break;
        };
        used += comma + json.len();
        if let Some(r) = r {
            refs.push((
                r,
                Target {
                    path: node.path.clone(),
                    bounds: node.bounds,
                },
            ));
        }
        parts.push(json);
        if shrunk {
            reason = "payload";
            stopped = true;
            break;
        }
    }
    if !stopped && omitted_ref {
        reason = "refs";
    } else if !stopped && collected.truncated {
        reason = collected.reason;
    }
    let detail = assemble(reason, &parts);
    if detail.len() > caps.payload_max_bytes {
        log::error!(
            "semantic snapshot payload is {} bytes, cap is {}",
            detail.len(),
            caps.payload_max_bytes
        );
        return Err(());
    }
    Ok(Built {
        detail,
        reason,
        refs,
    })
}

fn fit_node(node: &RawNode, r: Option<&str>, budget: usize) -> Option<(String, bool)> {
    let full = render_node(
        node,
        r,
        &node.text,
        &node.desc,
        &node.view_id,
        &node.package_name,
        &node.class_name,
    );
    if full.len() <= budget {
        return Some((full, false));
    }
    if let Some(json) = shrink_text(
        node,
        r,
        budget,
        &node.desc,
        &node.view_id,
        &node.package_name,
        &node.class_name,
    ) {
        return Some((json, true));
    }
    shrink_text(node, r, budget, "", "", "", "").map(|json| (json, true))
}

fn shrink_text(
    node: &RawNode,
    r: Option<&str>,
    budget: usize,
    desc: &str,
    id: &str,
    pkg: &str,
    class_name: &str,
) -> Option<String> {
    let count = node.text.chars().count();
    let mut lo = 0usize;
    let mut hi = count;
    let mut best = None;
    loop {
        if lo > hi {
            break;
        }
        let mid = lo + (hi - lo) / 2;
        let prefix: String = node.text.chars().take(mid).collect();
        let json = render_node(node, r, &prefix, desc, id, pkg, class_name);
        if json.len() <= budget {
            best = Some(json);
            if mid == hi {
                break;
            }
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    best
}

fn render_node(
    node: &RawNode,
    r: Option<&str>,
    text: &str,
    desc: &str,
    id: &str,
    pkg: &str,
    class_name: &str,
) -> String {
    let mut out = String::with_capacity(96 + text.len());
    out.push('{');
    if let Some(r) = r {
        out.push_str("\"ref\":");
        push_json_str(&mut out, r);
        out.push(',');
    }
    out.push_str("\"cls\":");
    push_json_str(&mut out, class_name);
    out.push_str(",\"text\":");
    push_json_str(&mut out, text);
    out.push_str(",\"desc\":");
    push_json_str(&mut out, desc);
    out.push_str(",\"id\":");
    push_json_str(&mut out, id);
    out.push_str(",\"pkg\":");
    push_json_str(&mut out, pkg);
    out.push_str(",\"click\":");
    out.push_str(if node.clickable { "true" } else { "false" });
    out.push_str(",\"b\":[");
    for (i, v) in node.bounds.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&v.to_string());
    }
    out.push_str("]}");
    out
}

fn assemble(reason: &str, parts: &[String]) -> String {
    let mut out = String::new();
    out.push_str("{\"truncated\":");
    out.push_str(if reason.is_empty() { "false" } else { "true" });
    out.push_str(",\"reason\":");
    push_json_str(&mut out, reason);
    out.push_str(",\"nodes\":[");
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(part);
    }
    out.push_str("]}");
    out
}

fn envelope_len(reason: &str) -> usize {
    let flag = if reason.is_empty() { "false" } else { "true" };
    "{\"truncated\":".len()
        + flag.len()
        + ",\"reason\":".len()
        + 2
        + reason.len()
        + ",\"nodes\":[]}".len()
}

fn push_json_str(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl RefTable {
    fn take_live(&mut self, r: &str, now: Instant) -> Option<Target> {
        let expires = self.entries.get(r)?.expires;
        if now >= expires {
            self.entries.remove(r);
            return None;
        }
        self.entries.get(r).map(|entry| entry.target.clone())
    }
}

fn parse_caps(text: &str) -> Result<Caps, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "caps json is not an object".to_owned())?;
    if obj.len() != 8 {
        return Err(format!("caps json has {} fields", obj.len()));
    }
    Ok(Caps {
        max_refs: field(obj, "max_refs")?,
        max_nodes: field(obj, "max_nodes")?,
        payload_max_bytes: field(obj, "payload_max_bytes")?,
        snapshot_timeout_ms: field_u64(obj, "snapshot_timeout_ms")?,
        click_timeout_ms: field_u64(obj, "click_timeout_ms")?,
        ref_ttl_ms: field_u64(obj, "ref_ttl_ms")?,
        flight_lease_ms: field_u64(obj, "flight_lease_ms")?,
        batch_max: field(obj, "batch_max")?,
    })
}

fn field(obj: &serde_json::Map<String, Value>, key: &str) -> Result<usize, String> {
    let n = field_u64(obj, key)?;
    usize::try_from(n).map_err(|_| format!("{key} does not fit"))
}

fn field_u64(obj: &serde_json::Map<String, Value>, key: &str) -> Result<u64, String> {
    let value = obj.get(key).ok_or_else(|| format!("{key} missing"))?;
    let n = value
        .as_u64()
        .ok_or_else(|| format!("{key} is not an integer"))?;
    if n == 0 {
        return Err(format!("{key} is zero"));
    }
    Ok(n)
}

fn max_ref_token_len(max_refs: usize) -> usize {
    let gen_digits = u64::MAX.to_string().len();
    let index_digits = max_refs.saturating_sub(1).to_string().len().max(1);
    "g".len() + gen_digits + ".n".len() + index_digits
}

fn click_arg_limit(caps: &Caps) -> usize {
    let n = caps.batch_max;
    let per = 1 + max_ref_token_len(caps.max_refs) + 1;
    let seps = n.saturating_sub(1) * 2;
    1 + n * per + seps + 1
}

fn parse_click_arg(arg: &str, caps: &Caps) -> Result<Vec<String>, &'static str> {
    if arg.is_empty() {
        return Err("empty");
    }
    if arg.len() > click_arg_limit(caps) {
        return Err("longer than the cap");
    }
    if arg.as_bytes()[0] == b'[' {
        let value: Value = serde_json::from_str(arg).map_err(|_| "not a json array")?;
        let arr = value.as_array().ok_or("not a json array")?;
        if arr.is_empty() {
            return Err("batch is empty");
        }
        if arr.len() > caps.batch_max {
            return Err("batch is over the cap");
        }
        let mut refs = Vec::with_capacity(arr.len());
        for item in arr {
            let r = item.as_str().ok_or("ref is not a token")?;
            if !ref_token_ok(r, caps.max_refs) {
                return Err("ref is not a token");
            }
            refs.push(r.to_owned());
        }
        return Ok(refs);
    }
    if !ref_token_ok(arg, caps.max_refs) {
        return Err("ref is not a token");
    }
    Ok(vec![arg.to_owned()])
}

fn ref_token_ok(r: &str, max_refs: usize) -> bool {
    if r.len() > max_ref_token_len(max_refs) {
        return false;
    }
    let Some(rest) = r.strip_prefix('g') else {
        return false;
    };
    let Some((gen, index)) = rest.split_once(".n") else {
        return false;
    };
    if gen.is_empty()
        || index.is_empty()
        || (gen.len() > 1 && gen.starts_with('0'))
        || (index.len() > 1 && index.starts_with('0'))
    {
        return false;
    }
    if !gen.bytes().all(|b| b.is_ascii_digit()) || !index.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Ok(gen) = gen.parse::<u64>() else {
        return false;
    };
    let Ok(index) = index.parse::<usize>() else {
        return false;
    };
    gen >= 1 && index < max_refs
}

fn parse_wire(raw: &str, caps: &Caps) -> Result<Collected, &'static str> {
    if raw == SNAPSHOT_FAILED {
        return Err(SNAPSHOT_FAILED);
    }
    let Some(json) = raw.strip_prefix("ok\n") else {
        log::error!("semantic snapshot wire has no payload");
        return Err(SNAPSHOT_FAILED);
    };
    if json.len() > caps.payload_max_bytes {
        log::error!("semantic snapshot wire is {} bytes", json.len());
        return Err(SNAPSHOT_FAILED);
    }
    parse_collected(json, caps).map_err(|why| {
        log::error!("semantic snapshot wire rejected: {why}");
        SNAPSHOT_FAILED
    })
}

fn parse_collected(json: &str, caps: &Caps) -> Result<Collected, &'static str> {
    let value: Value = serde_json::from_str(json).map_err(|_| "not json")?;
    let obj = value.as_object().ok_or("not an object")?;
    if obj.len() != 3 {
        return Err("unexpected wire fields");
    }
    let truncated = obj
        .get("truncated")
        .and_then(|v| v.as_bool())
        .ok_or("truncated")?;
    let reason = obj.get("reason").and_then(|v| v.as_str()).ok_or("reason")?;
    let reason = match (truncated, reason) {
        (false, "") => "",
        (true, "nodes") => "nodes",
        (true, "payload") => "payload",
        (true, "refs") => "refs",
        _ => return Err("truncated flag does not match reason"),
    };
    let nodes = obj.get("nodes").and_then(|v| v.as_array()).ok_or("nodes")?;
    if nodes.len() > caps.max_nodes {
        return Err("wire has more nodes than the cap");
    }
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        out.push(parse_raw_node(node, caps)?);
    }
    Ok(Collected {
        nodes: out,
        truncated,
        reason,
    })
}

fn parse_raw_node(value: &Value, caps: &Caps) -> Result<RawNode, &'static str> {
    let obj = value.as_object().ok_or("node")?;
    if obj.len() != 8 {
        return Err("unexpected node fields");
    }
    let path_v = obj.get("path").and_then(|v| v.as_array()).ok_or("path")?;
    if path_v.len() > caps.max_nodes {
        return Err("path is too deep");
    }
    let mut path = Vec::with_capacity(path_v.len());
    for index in path_v {
        let n = index.as_u64().ok_or("path index")?;
        let n = u16::try_from(n).map_err(|_| "path index")?;
        path.push(n);
    }
    let bounds_v = obj.get("b").and_then(|v| v.as_array()).ok_or("b")?;
    if bounds_v.len() != 4 {
        return Err("b");
    }
    let mut bounds = [0i32; 4];
    for (i, item) in bounds_v.iter().enumerate() {
        let n = item.as_i64().ok_or("b")?;
        bounds[i] = i32::try_from(n).map_err(|_| "b")?;
    }
    Ok(RawNode {
        class_name: owned_str(obj, "cls")?,
        text: owned_str(obj, "text")?,
        desc: owned_str(obj, "desc")?,
        view_id: owned_str(obj, "id")?,
        package_name: owned_str(obj, "pkg")?,
        clickable: obj.get("click").and_then(|v| v.as_bool()).ok_or("click")?,
        bounds,
        path,
    })
}

fn owned_str(obj: &serde_json::Map<String, Value>, key: &str) -> Result<String, &'static str> {
    match obj.get(key).and_then(|v| v.as_str()) {
        Some(s) => Ok(s.to_owned()),
        None => Err("node field"),
    }
}

fn check_detail(detail: &str) -> Result<(), &'static str> {
    let caps = caps().map_err(|_| "caps")?;
    if detail.len() > caps.payload_max_bytes {
        return Err("payload is over the cap");
    }
    let value: Value = serde_json::from_str(detail).map_err(|_| "not json")?;
    let obj = value.as_object().ok_or("not an object")?;
    if obj.len() != 3 {
        return Err("unexpected fields");
    }
    let truncated = obj
        .get("truncated")
        .and_then(|v| v.as_bool())
        .ok_or("truncated")?;
    let reason = obj.get("reason").and_then(|v| v.as_str()).ok_or("reason")?;
    match (truncated, reason) {
        (false, "") | (true, "nodes") | (true, "payload") | (true, "refs") => {}
        _ => return Err("truncated flag does not match reason"),
    }
    let nodes = obj.get("nodes").and_then(|v| v.as_array()).ok_or("nodes")?;
    if nodes.len() > caps.max_nodes {
        return Err("too many nodes");
    }
    let mut seen = HashMap::new();
    for node in nodes {
        let obj = node.as_object().ok_or("node")?;
        let click = obj.get("click").and_then(|v| v.as_bool()).ok_or("click")?;
        let has_ref = obj.contains_key("ref");
        if obj.len() != if has_ref { 8 } else { 7 } || (has_ref && !click) {
            return Err("unexpected node fields");
        }
        if has_ref {
            let r = obj.get("ref").and_then(|v| v.as_str()).ok_or("ref")?;
            if !ref_token_ok(r, caps.max_refs) {
                return Err("ref");
            }
            if seen.insert(r.to_owned(), ()).is_some() {
                return Err("duplicate ref");
            }
        }
        for key in ["cls", "text", "desc", "id", "pkg"] {
            obj.get(key).and_then(|v| v.as_str()).ok_or(key)?;
        }
        let bounds = obj.get("b").and_then(|v| v.as_array()).ok_or("b")?;
        if bounds.len() != 4 {
            return Err("b");
        }
        for item in bounds {
            let n = item.as_i64().ok_or("b")?;
            i32::try_from(n).map_err(|_| "b")?;
        }
    }
    Ok(())
}

fn refuse_message(kind: &str, why: Refuse) -> Message {
    match why {
        Refuse::Wedged => result_message_with_detail(kind, SNAPSHOT_FAILED, A11Y_WEDGED),
        Refuse::Held => result_message(kind, SNAPSHOT_FAILED),
    }
}

fn to_message(kind: &str, outcome: &Outcome) -> Message {
    if outcome.detail.is_empty() {
        result_message(kind, outcome.code)
    } else {
        result_message_with_detail(kind, outcome.code, &outcome.detail)
    }
}

pub fn dispatch(kind: String, arg: String, send: impl FnOnce(Message) + Send + 'static) {
    if let Some(msg) = admit(&kind, &arg) {
        send(msg);
        return;
    }
    let Some(peer) = peer() else {
        send(result_message(&kind, SNAPSHOT_FAILED));
        return;
    };
    let timeout = Duration::from_millis(if kind == SNAPSHOT {
        peer.caps.snapshot_timeout_ms
    } else {
        peer.caps.click_timeout_ms
    });
    let (tx, rx) = hbb_common::tokio::sync::mpsc::channel(1);
    let started = if kind == CLICK {
        match peer.resolve(&arg, Instant::now()) {
            Err(code) => {
                send(result_message(CLICK, code));
                return;
            }
            Ok(targets) => peer.try_spawn(Instant::now(), move |mut permit| {
                let code = perform_all(&targets, &mut perform_clicks);
                if permit.conclude() {
                    let _ = tx.blocking_send(result_message(CLICK, code));
                }
            }),
        }
    } else {
        peer.try_spawn(Instant::now(), move |mut permit| {
            let token = permit.token();
            let outcome = match collect_tree() {
                Ok(collected) => peer
                    .publish_if_current(token, collected, Instant::now())
                    .unwrap_or_else(Outcome::fail),
                Err(code) if peer.still_current(token) => Outcome {
                    code,
                    detail: String::new(),
                },
                Err(_) => Outcome::fail(),
            };
            let keep = permit.conclude();
            if keep {
                let _ = tx.blocking_send(to_message(SNAPSHOT, &outcome));
            }
        })
    };
    if let Err(why) = started {
        send(refuse_message(&kind, why));
        return;
    }
    arm_timeout(rx, timeout, kind, send);
}

fn arm_timeout(
    mut rx: hbb_common::tokio::sync::mpsc::Receiver<Message>,
    timeout: Duration,
    kind: String,
    send: impl FnOnce(Message) + Send + 'static,
) {
    let fail = result_message(&kind, SNAPSHOT_FAILED);
    match hbb_common::tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(async move {
                let msg = match hbb_common::tokio::time::timeout(timeout, rx.recv()).await {
                    Ok(Some(msg)) => msg,
                    _ => fail,
                };
                send(msg);
            });
        }
        Err(_) => {
            let started = Instant::now();
            let msg = match rx.blocking_recv() {
                Some(msg) if started.elapsed() <= timeout => msg,
                _ => fail,
            };
            send(msg);
        }
    }
}

fn admit(kind: &str, arg: &str) -> Option<Message> {
    let verdict = validate(kind, arg);
    if verdict != OK {
        return Some(result_message(kind, verdict));
    }
    #[cfg(not(target_os = "android"))]
    {
        return Some(result_message(kind, UNSUPPORTED));
    }
    #[cfg(target_os = "android")]
    {
        let _ = arg;
        None
    }
}

/// `jni` is the accessibility call. Parse stays outside it. The lease already
/// allows only one active worker, so this is not locked: an abandoned call
/// still inside `jni` must not stop the new worker.
fn snapshot_from_jni<F>(jni: F) -> Result<Collected, &'static str>
where
    F: FnOnce() -> Result<String, &'static str>,
{
    let caps = caps().map_err(|_| SNAPSHOT_FAILED)?;
    parse_wire(&jni()?, &caps)
}

#[cfg(target_os = "android")]
fn collect_tree() -> Result<Collected, &'static str> {
    snapshot_from_jni(
        || match scrap::android::call_main_service_semantic_action(SNAPSHOT, "") {
            Ok(raw) => Ok(raw),
            Err(e) => {
                log::error!("semantic snapshot jni failed: {e}");
                Err(SNAPSHOT_FAILED)
            }
        },
    )
}

#[cfg(not(target_os = "android"))]
fn collect_tree() -> Result<Collected, &'static str> {
    Err(UNSUPPORTED)
}

fn click_bounds_code(stored: [i32; 4], live: [i32; 4]) -> &'static str {
    if stored == live {
        OK
    } else {
        REF_STALE
    }
}

fn perform_clicks(target: &Target) -> &'static str {
    #[cfg(target_os = "android")]
    {
        return perform_click_android(target);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = target;
        SNAPSHOT_FAILED
    }
}

#[cfg(target_os = "android")]
fn perform_click_android(target: &Target) -> &'static str {
    let arg = encode_target(target);
    match scrap::android::call_main_service_semantic_action(CLICK, &arg) {
        Ok(code) if code == OK => OK,
        Ok(code) if code == REF_STALE => REF_STALE,
        Ok(code) => {
            log::error!("semantic click -> {code}");
            SNAPSHOT_FAILED
        }
        Err(e) => {
            log::error!("semantic click jni failed: {e}");
            SNAPSHOT_FAILED
        }
    }
}

fn encode_target(target: &Target) -> String {
    let mut out = String::from("{\"path\":[");
    for (i, index) in target.path.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&index.to_string());
    }
    out.push_str("],\"b\":[");
    for (i, v) in target.bounds.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&v.to_string());
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use hbb_common::protobuf::Message as _;

    fn caps_ok() -> Caps {
        caps().unwrap()
    }

    fn sample(text: &str, path: Vec<u16>, bounds: [i32; 4]) -> RawNode {
        RawNode {
            class_name: "android.widget.Button".into(),
            text: text.into(),
            desc: String::new(),
            view_id: "com.sankuai.meituan:id/search".into(),
            package_name: "com.sankuai.meituan".into(),
            clickable: true,
            bounds,
            path,
        }
    }

    fn collect(nodes: Vec<RawNode>) -> Collected {
        Collected {
            nodes,
            truncated: false,
            reason: "",
        }
    }

    fn peer() -> Peer {
        Peer::new(caps_ok())
    }

    #[test]
    fn caps_json_is_the_contract() {
        let caps = caps_ok();
        assert_eq!(caps.max_refs, 80);
        assert_eq!(A11Y_THREAD_CAP, 2);
        assert_eq!(caps.flight_lease_ms, 5_000);
        assert_eq!(caps.max_nodes, 256);
        assert_eq!(caps.payload_max_bytes, 32 * 1024);
        assert_eq!(caps.snapshot_timeout_ms, 1_500);
        assert_eq!(caps.click_timeout_ms, 1_000);
        assert_eq!(caps.ref_ttl_ms, 5_000);
        assert_eq!(caps.batch_max, 8);
        assert_eq!(
            envelope_len("payload"),
            "{\"truncated\":true,\"reason\":\"payload\",\"nodes\":[]}".len()
        );
        let mut extra: Value = serde_json::from_str(CAPS_JSON).unwrap();
        extra
            .as_object_mut()
            .unwrap()
            .insert("fg".into(), Value::from(1));
        assert!(parse_caps(&extra.to_string()).is_err());
        let mut missing: Value = serde_json::from_str(CAPS_JSON).unwrap();
        missing.as_object_mut().unwrap().remove("batch_max");
        assert!(parse_caps(&missing.to_string()).is_err());
    }

    #[test]
    fn snapshot_click_round_trip() {
        let peer = peer();
        let now = Instant::now();
        let outcome = peer.snapshot_with(now, || {
            Ok(collect(vec![
                sample("搜索", vec![], [0, 10, 20, 30]),
                sample("外卖", vec![1u16], [0, 40, 20, 60]),
            ]))
        });
        assert_eq!(outcome.code, OK);
        let parsed: Value = serde_json::from_str(&outcome.detail).unwrap();
        assert_eq!(parsed["truncated"], false);
        assert_eq!(parsed["reason"], "");
        let nodes = parsed["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["ref"], "g1.n0");
        assert_eq!(nodes[0]["text"], "搜索");
        assert_eq!(nodes[0]["pkg"], "com.sankuai.meituan");
        assert_eq!(nodes[0]["click"], true);
        assert_eq!(nodes[1]["ref"], "g1.n1");
        assert!(outcome.detail.len() <= caps_ok().payload_max_bytes);
        accept_snapshot_detail(&outcome.detail).unwrap();
        let msg = result_message_with_detail(SNAPSHOT, OK, &outcome.detail);
        match msg.union {
            Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                assert_eq!(result.code, OK);
                assert!(result.detail.contains("\"ref\":\"g1.n0\""));
            }
            other => panic!("expected SemanticActionResult, got {other:?}"),
        }

        let mut hit = Vec::new();
        let code = peer.click_at("g1.n0", now, |target| {
            hit.push(target.bounds);
            OK
        });
        assert_eq!(code, OK);
        assert_eq!(hit, vec![[0, 10, 20, 30]]);

        let mut batch = 0;
        let code = peer.click_at("[\"g1.n0\",\"g1.n1\"]", now, |_| {
            batch += 1;
            OK
        });
        assert_eq!(code, OK);
        assert_eq!(batch, 2);
    }

    #[test]
    fn ref_stale_after_ttl_and_after_a_new_snapshot() {
        let peer = peer();
        let now = Instant::now();
        let caps = caps_ok();
        peer.snapshot_with(now, || {
            Ok(collect(vec![
                sample("搜索", vec![0u16], [1, 2, 3, 4]),
                sample("外卖", vec![1u16], [5, 6, 7, 8]),
            ]))
        });
        let live = now + Duration::from_millis(caps.ref_ttl_ms - 1);
        assert_eq!(peer.click_at("g1.n0", live, |_| OK), OK);
        let expired = now + Duration::from_millis(caps.ref_ttl_ms);
        assert_eq!(
            peer.click_at("g1.n1", expired, |_| panic!("stale ref was clicked")),
            REF_STALE
        );
        assert_eq!(
            peer.click_at("g1.n9", now, |_| panic!("unknown ref was clicked")),
            REF_STALE
        );

        peer.snapshot_with(now, || {
            Ok(collect(vec![sample("只剩一个", vec![0u16], [9, 9, 9, 9])]))
        });
        assert_eq!(
            peer.click_at("g1.n1", now, |_| panic!("replaced ref was clicked")),
            REF_STALE
        );
        assert_eq!(peer.click_at("g2.n0", now, |_| OK), OK);
    }

    #[test]
    fn payload_is_truncated_and_marked() {
        let caps = caps_ok();
        let fat = "餐".repeat(400);
        let nodes: Vec<_> = (0..80)
            .map(|i| sample(&fat, vec![i as u16], [0, i, 1, i + 1]))
            .collect();
        let built = build_snapshot(&collect(nodes), &caps, 1).unwrap();
        assert!(built.detail.len() <= caps.payload_max_bytes);
        let parsed: Value = serde_json::from_str(&built.detail).unwrap();
        assert_eq!(parsed["truncated"], true);
        assert_eq!(parsed["reason"], "payload");
        let n = parsed["nodes"].as_array().unwrap().len();
        assert!(n > 0 && n < caps.max_refs, "{n}");
        assert!(n < caps.max_nodes);

        let huge = sample(&"x".repeat(40_000), vec![0u16], [0, 0, 1, 1]);
        let built = build_snapshot(&collect(vec![huge]), &caps, 1).unwrap();
        assert!(built.detail.len() <= caps.payload_max_bytes);
        let parsed: Value = serde_json::from_str(&built.detail).unwrap();
        assert_eq!(parsed["reason"], "payload");
        let text = parsed["nodes"][0]["text"].as_str().unwrap();
        assert!(text.len() < 40_000);
        assert!("x".repeat(40_000).starts_with(text));
    }

    #[test]
    fn node_cap_stops_at_the_limit() {
        let caps = caps_ok();
        let mut nodes: Vec<_> = (0..300)
            .map(|i| RawNode {
                class_name: "c".into(),
                text: format!("t{i}"),
                desc: String::new(),
                view_id: String::new(),
                package_name: String::new(),
                clickable: false,
                bounds: [0, i, 1, i + 1],
                path: vec![i as u16],
            })
            .collect();
        nodes[256].text = "past-cap".into();
        nodes[256].clickable = true;
        let built = build_snapshot(&collect(nodes), &caps, 1).unwrap();
        let parsed: Value = serde_json::from_str(&built.detail).unwrap();
        assert_eq!(parsed["truncated"], true);
        assert_eq!(parsed["reason"], "nodes");
        let nodes = parsed["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), caps.max_nodes);
        assert_eq!(nodes[0]["text"], "t0");
        assert_eq!(nodes[0]["click"], false);
        assert!(nodes[0].get("ref").is_none());
        assert!(!built.detail.contains("past-cap"));
        assert!(built.detail.len() <= caps.payload_max_bytes);
        assert!(built.refs.is_empty());
    }

    #[test]
    fn ref_count_stops_at_eighty_clickable_nodes() {
        let caps = caps_ok();
        let nodes: Vec<_> = (0..100)
            .map(|i| sample(&format!("c{i}"), vec![i as u16], [0, i, 1, i + 1]))
            .collect();
        let built = build_snapshot(&collect(nodes), &caps, 7).unwrap();
        let parsed: Value = serde_json::from_str(&built.detail).unwrap();
        assert_eq!(parsed["truncated"], true);
        assert_eq!(parsed["reason"], "refs");
        let nodes = parsed["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 100);
        assert_eq!(nodes[0]["ref"], "g7.n0");
        assert_eq!(nodes[79]["ref"], "g7.n79");
        assert_eq!(nodes[0]["click"], true);
        assert!(nodes[80].get("ref").is_none());
        assert_eq!(nodes[80]["text"], "c80");
        assert_eq!(nodes[80]["click"], true);
        assert!(built.detail.contains("\"text\":\"c80\""));
        assert_eq!(built.refs.len(), caps.max_refs);
        accept_snapshot_detail(&built.detail).unwrap();
    }

    #[test]
    fn a_text_node_keeps_its_text_and_has_no_ref() {
        let caps = caps_ok();
        let mut text = sample("外卖", vec![1u16], [0, 40, 20, 60]);
        text.clickable = false;
        let built = build_snapshot(
            &collect(vec![sample("搜索", vec![0u16], [0, 10, 20, 30]), text]),
            &caps,
            1,
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&built.detail).unwrap();
        assert_eq!(parsed["truncated"], false);
        assert_eq!(parsed["reason"], "");
        let nodes = parsed["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["ref"], "g1.n0");
        assert_eq!(nodes[0]["text"], "搜索");
        assert_eq!(nodes[1]["text"], "外卖");
        assert_eq!(nodes[1]["click"], false);
        assert!(nodes[1].get("ref").is_none());
        assert_eq!(built.refs.len(), 1);
        accept_snapshot_detail(&built.detail).unwrap();
    }

    #[test]
    fn a_second_snapshot_does_not_start() {
        let peer = Arc::new(peer());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let first = Arc::clone(&peer);
        let worker = thread::spawn(move || {
            first.snapshot_with(Instant::now(), move || {
                started_tx.send(()).unwrap();
                let _ = release_rx.recv();
                Ok(collect(vec![sample("搜索", vec![0u16], [0, 0, 1, 1])]))
            })
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = peer.snapshot_with(Instant::now(), || {
            panic!("second snapshot walked the tree");
        });
        assert_eq!(second.code, SNAPSHOT_FAILED);
        assert!(second.detail.is_empty());
        release_tx.send(()).unwrap();
        let first = worker.join().unwrap();
        assert_eq!(first.code, OK);
        assert!(first.detail.contains("搜索"));
    }

    #[test]
    fn batch_over_the_cap_clicks_nothing() {
        let peer = peer();
        let now = Instant::now();
        let nodes: Vec<_> = (0..9)
            .map(|i| sample("x", vec![i as u16], [0, i, 1, i + 1]))
            .collect();
        assert_eq!(peer.snapshot_with(now, || Ok(collect(nodes))).code, OK);
        let arg = format!(
            "[{}]",
            (0..9)
                .map(|i| format!("\"g1.n{i}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(click_arg_verdict(&arg), SNAPSHOT_FAILED);
        assert_eq!(
            peer.click_at(&arg, now, |_| panic!("oversize batch clicked")),
            SNAPSHOT_FAILED
        );
        let eight = format!(
            "[{}]",
            (0..8)
                .map(|i| format!("\"g1.n{i}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut n = 0;
        assert_eq!(
            peer.click_at(&eight, now, |_| {
                n += 1;
                OK
            }),
            OK
        );
        assert_eq!(n, 8);
        let long = "g1.n".to_owned() + &"1".repeat(40);
        assert_eq!(
            peer.click_at(&long, now, |_| panic!("long ref clicked")),
            SNAPSHOT_FAILED
        );
    }

    #[test]
    fn snapshot_timeout_is_the_cap() {
        let peer = peer();
        let caps = caps_ok();
        let started = Instant::now();
        let slow = caps.snapshot_timeout_ms + 5_000;
        let outcome = peer.snapshot_with(Instant::now(), move || {
            thread::sleep(Duration::from_millis(slow));
            Ok(collect(vec![]))
        });
        let elapsed = started.elapsed();
        assert_eq!(outcome.code, SNAPSHOT_FAILED);
        assert!(outcome.detail.is_empty());
        assert!(elapsed >= Duration::from_millis(caps.snapshot_timeout_ms));
        assert!(elapsed < Duration::from_millis(caps.snapshot_timeout_ms + 2_000));
    }

    #[test]
    fn click_timeout_is_the_cap() {
        let peer = peer();
        let caps = caps_ok();
        let now = Instant::now();
        assert_eq!(
            peer.snapshot_with(now, move || Ok(collect(vec![sample(
                "搜索",
                vec![0u16],
                [0, 0, 1, 1]
            )])))
            .code,
            OK
        );
        let started = Instant::now();
        let slow = caps.click_timeout_ms + 5_000;
        let code = peer.click_timed("g1.n0", now, move |_target| {
            thread::sleep(Duration::from_millis(slow));
            OK
        });
        let elapsed = started.elapsed();
        assert_eq!(code, SNAPSHOT_FAILED);
        assert!(elapsed >= Duration::from_millis(caps.click_timeout_ms));
        assert!(elapsed < Duration::from_millis(caps.click_timeout_ms + 2_000));
    }

    #[test]
    fn wire_snapshot_keeps_the_node_and_assigns_a_ref() {
        let caps = caps_ok();
        let wire = concat!(
            "ok\n",
            r#"{"truncated":false,"reason":"","nodes":[{"cls":"android.widget.Button","text":"搜索","desc":"","id":"com.sankuai.meituan:id/search","pkg":"com.sankuai.meituan","click":true,"b":[0,10,20,30],"path":[]}]}"#
        );
        let collected = parse_wire(wire, &caps).unwrap();
        let built = build_snapshot(&collected, &caps, 1).unwrap();
        assert_eq!(
            built.detail,
            r#"{"truncated":false,"reason":"","nodes":[{"ref":"g1.n0","cls":"android.widget.Button","text":"搜索","desc":"","id":"com.sankuai.meituan:id/search","pkg":"com.sankuai.meituan","click":true,"b":[0,10,20,30]}]}"#
        );
        assert_eq!(
            encode_target(&built.refs[0].1),
            r#"{"path":[],"b":[0,10,20,30]}"#
        );
    }

    #[test]
    fn a_stuck_click_rejects_the_next_click_without_a_thread() {
        let peer = Arc::new(peer());
        let now = Instant::now();
        assert_eq!(
            peer.snapshot_with(now, move || Ok(collect(vec![sample(
                "搜索",
                vec![0u16],
                [0, 0, 10, 10]
            )])))
            .code,
            OK
        );
        let before = peer.threads_started();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let first = Arc::clone(&peer);
        let worker = thread::spawn(move || {
            first.click_timed("g1.n0", now, move |_target| {
                started_tx.send(()).unwrap();
                let _ = release_rx.recv();
                OK
            })
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = peer.threads_started();
        assert_eq!(started, before + 1);
        assert_eq!(peer.live_workers.load(Ordering::Acquire), 1);
        assert!(peer.worker_peak() <= A11Y_THREAD_CAP);
        let code = peer.click_timed("g1.n0", now, |_| panic!("busy click started work"));
        assert_eq!(code, SNAPSHOT_FAILED);
        assert_eq!(peer.threads_started(), started);
        release_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), OK);
    }

    fn node_wire(text: &str, bounds: [i32; 4]) -> String {
        format!(
            "ok\n{{\"truncated\":false,\"reason\":\"\",\"nodes\":[{{\"cls\":\"android.widget.Button\",\"text\":\"{text}\",\"desc\":\"\",\"id\":\"com.sankuai.meituan:id/search\",\"pkg\":\"com.sankuai.meituan\",\"click\":true,\"b\":[{},{},{},{}],\"path\":[0]}}]}}",
            bounds[0], bounds[1], bounds[2], bounds[3]
        )
    }

    fn click_bounds(peer: &Peer, now: Instant) -> [i32; 4] {
        let mut saw = [0; 4];
        assert_eq!(
            peer.click_at("g1.n0", now, |target| {
                saw = target.bounds;
                OK
            }),
            OK
        );
        saw
    }

    /// The hang is inside `snapshot_from_jni`, the call that used to hold
    /// `a11y_lock`. The new request returns `ok` while that call is still
    /// blocked. The late wire is not installed over the new ref table.
    #[test]
    fn a_jni_hang_recovers_after_the_lease_and_drops_the_late_tree() {
        let peer = Arc::new(peer());
        let t0 = Instant::now();
        let lease = Duration::from_millis(peer.caps.flight_lease_ms);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let stuck_peer = Arc::clone(&peer);
        let stuck = thread::spawn(move || {
            stuck_peer.snapshot_with(t0, move || {
                snapshot_from_jni(|| {
                    started_tx.send(()).unwrap();
                    let _ = release_rx.recv();
                    Ok(node_wire("LATE", [1, 1, 2, 2]))
                })
            })
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let during = t0 + lease - Duration::from_millis(1);
        let held = peer.threads_started();
        let busy = peer.snapshot_with(during, || panic!("request inside the lease"));
        assert_eq!(busy.code, SNAPSHOT_FAILED);
        assert!(busy.detail.is_empty());
        assert_eq!(peer.threads_started(), held);
        assert_eq!(peer.live_workers.load(Ordering::Acquire), 1);
        assert!(peer.worker_peak() <= A11Y_THREAD_CAP);

        let after = t0 + lease;
        let fresh = peer.snapshot_with(after, || {
            snapshot_from_jni(|| Ok(node_wire("FRESH", [9, 9, 19, 19])))
        });
        assert_eq!(fresh.code, OK);
        assert!(fresh.detail.contains("FRESH"));
        assert!(!fresh.detail.contains("LATE"));
        assert_eq!(peer.live_workers.load(Ordering::Acquire), 1);
        assert!(peer.worker_peak() <= A11Y_THREAD_CAP);
        assert_eq!(click_bounds(&peer, after), [9, 9, 19, 19]);

        release_tx.send(()).unwrap();
        assert_eq!(stuck.join().unwrap().code, SNAPSHOT_FAILED);
        assert_eq!(click_bounds(&peer, after), [9, 9, 19, 19]);
        assert!(peer.worker_peak() <= A11Y_THREAD_CAP);

        let (again_tx, again_rx) = mpsc::channel();
        let (release_again_tx, release_again_rx) = mpsc::channel::<()>();
        let again_peer = Arc::clone(&peer);
        let again = thread::spawn(move || {
            again_peer.snapshot_with(after, move || {
                snapshot_from_jni(|| {
                    again_tx.send(()).unwrap();
                    let _ = release_again_rx.recv();
                    Ok(node_wire("LATE2", [3, 3, 4, 4]))
                })
            })
        });
        again_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let (third_tx, third_rx) = mpsc::channel();
        let (release_third_tx, release_third_rx) = mpsc::channel::<()>();
        let third_peer = Arc::clone(&peer);
        let third = thread::spawn(move || {
            third_peer.snapshot_with(after + lease, move || {
                snapshot_from_jni(|| {
                    third_tx.send(()).unwrap();
                    let _ = release_third_rx.recv();
                    Ok(node_wire("LATE3", [5, 5, 6, 6]))
                })
            })
        });
        third_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let before_wedge = peer.threads_started();
        let wedged = peer.snapshot_with(after + lease + lease, || panic!("third worker"));
        assert_eq!(wedged.code, SNAPSHOT_FAILED);
        assert_eq!(wedged.detail, A11Y_WEDGED);
        assert_eq!(peer.threads_started(), before_wedge);
        assert!(peer.live_workers.load(Ordering::Acquire) <= A11Y_THREAD_CAP);
        assert!(peer.worker_peak() <= A11Y_THREAD_CAP);
        release_again_tx.send(()).unwrap();
        release_third_tx.send(()).unwrap();
        let _ = again.join().unwrap();
        let _ = third.join().unwrap();
        let after_release = peer.snapshot_with(after + lease + lease, || {
            snapshot_from_jni(|| Ok(node_wire("AFTER", [7, 7, 8, 8])))
        });
        assert_eq!(after_release.code, OK);
        assert!(after_release.detail.contains("AFTER"));
        assert!(peer.worker_peak() <= A11Y_THREAD_CAP);
    }

    #[test]
    fn moved_bounds_are_ref_stale_and_are_not_clicked() {
        let peer = peer();
        let now = Instant::now();
        let bounds = [0, 0, 10, 10];
        assert_eq!(
            peer.snapshot_with(now, move || Ok(collect(vec![sample(
                "搜索",
                vec![0u16],
                bounds
            )])))
            .code,
            OK
        );
        assert_eq!(click_bounds_code(bounds, [0, 0, 10, 11]), REF_STALE);
        let mut clicked = false;
        let code = peer.click_at("g1.n0", now, |target| {
            let live = [0, 0, 10, 11];
            let verdict = click_bounds_code(target.bounds, live);
            if verdict != OK {
                return verdict;
            }
            clicked = true;
            OK
        });
        assert_eq!(code, REF_STALE);
        assert!(!clicked);
    }

    #[test]
    fn an_old_generation_misses_a_same_sized_node() {
        let peer = peer();
        let now = Instant::now();
        let bounds = [4, 8, 40, 48];
        assert_eq!(
            peer.snapshot_with(now, move || Ok(collect(vec![sample(
                "搜索",
                vec![0u16],
                bounds
            )])))
            .code,
            OK
        );
        assert_eq!(
            peer.snapshot_with(now, move || Ok(collect(vec![sample(
                "搜索",
                vec![0u16],
                bounds
            )])))
            .code,
            OK
        );
        let mut clicked = false;
        assert_eq!(
            peer.click_at("g1.n0", now, |_| {
                clicked = true;
                OK
            }),
            REF_STALE
        );
        assert!(!clicked);
        assert_eq!(
            peer.click_at("g2.n0", now, |_| {
                clicked = true;
                OK
            }),
            OK
        );
        assert!(clicked);
    }

    fn controller_result(msg: Message) -> (String, String) {
        let bytes = msg.write_to_bytes().unwrap();
        let parsed = Message::parse_from_bytes(&bytes).unwrap();
        let result = match parsed.union {
            Some(base::message_proto::message::Union::SemanticActionResult(result)) => result,
            other => panic!("expected SemanticActionResult, got {other:?}"),
        };
        let mut flight = crate::semantic_action::Flight::default();
        crate::semantic_action::store_peer_result(
            &mut flight,
            &result.kind,
            &result.code,
            &result.detail,
        );
        (flight.result().to_owned(), flight.detail().to_owned())
    }

    #[test]
    fn wedged_detail_reaches_the_controller() {
        for kind in [SNAPSHOT, CLICK] {
            let (code, detail) = controller_result(refuse_message(kind, Refuse::Wedged));
            assert_eq!(code, SNAPSHOT_FAILED);
            assert_eq!(detail, A11Y_WEDGED);
        }
        let (code, detail) = controller_result(refuse_message(SNAPSHOT, Refuse::Held));
        assert_eq!(code, SNAPSHOT_FAILED);
        assert!(detail.is_empty());
        let (code, detail) = controller_result(result_message_with_detail(
            SNAPSHOT,
            SNAPSHOT_FAILED,
            "busy",
        ));
        assert_eq!(code, SNAPSHOT_FAILED);
        assert!(detail.is_empty());
    }

    #[test]
    fn a_stale_token_cannot_overwrite_the_table_after_the_check() {
        let peer = Arc::new(peer());
        let now = Instant::now();
        {
            let mut flight = peer.flight.lock().unwrap();
            flight.holder = 7;
        }
        let hooked = Arc::clone(&peer);
        *peer.publish_gap.lock().unwrap() = Some(Box::new(move || {
            let mut flight = hooked.flight.lock().unwrap();
            flight.holder = 8;
            drop(flight);
            let mut table = hooked.table.lock().unwrap();
            table.entries.clear();
            table.entries.insert(
                "kept".into(),
                Entry {
                    expires: now + Duration::from_secs(5),
                    target: Target {
                        path: vec![1],
                        bounds: [4, 4, 4, 4],
                    },
                },
            );
        }));
        let late =
            peer.publish_if_current(7, collect(vec![sample("LATE", vec![0], [1, 1, 2, 2])]), now);
        assert!(late.is_none());
        let table = peer.table.lock().unwrap();
        assert!(table.entries.contains_key("kept"));
        assert_eq!(table.entries.len(), 1);
        assert_eq!(table.entries["kept"].target.bounds, [4, 4, 4, 4]);
    }

    #[test]
    fn dispatch_off_android_is_unsupported() {
        #[cfg(not(target_os = "android"))]
        {
            let (tx, rx) = mpsc::channel();
            dispatch(SNAPSHOT.to_owned(), String::new(), move |msg| {
                tx.send(msg).unwrap();
            });
            let msg = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let bytes = msg.write_to_bytes().unwrap();
            let parsed = Message::parse_from_bytes(&bytes).unwrap();
            match parsed.union {
                Some(base::message_proto::message::Union::SemanticActionResult(result)) => {
                    assert_eq!(result.kind, SNAPSHOT);
                    assert_eq!(result.code, UNSUPPORTED);
                }
                other => panic!("expected SemanticActionResult, got {other:?}"),
            }
        }
    }
}
