//! Mecanim generic animation clips.
//!
//! A clip stores its curves in three groups, numbered consecutively: the *streamed* curves
//! (`m_StreamedClip`), the *dense* ones (`m_DenseClip`, sampled at a fixed rate) and the
//! *constant* ones (`m_ConstantClip`). The bindings in `m_ClipBindingConstant` name what each
//! consecutive run of curves drives: a Transform's position (3 curves), rotation as a quaternion
//! (4), scale (3) or Euler angles (3), or one value of some other component.
//!
//! Every curve is read into the same shape, a list of keys each holding the cubic polynomial of
//! the segment that starts there, `a s^3 + b s^2 + c s + d` with `s` the time since the key:
//!
//! - **Streamed**: a list of frames `time, key count, keys...`, a key being the curve index and
//!   the four coefficients (`a, b, c, d`, so `d` is the value and `c` the outgoing slope). The
//!   first frame has the lowest float as its time and holds the value before the first key; the last has time
//!   +infinity and no keys. Only the curves that have a key at a frame's time appear in it.
//! - **Dense**: `frame_count * curve_count` floats, frame major, one frame every
//!   `1 / sample_rate` seconds from `begin_time`, interpolated linearly.
//! - **Constant**: one value per curve.
//!
//! A cubic Hermite segment is fixed by its two end values and slopes, and the polynomial is
//! exactly such a segment, so the keys convert to glTF's cubic spline without loss. A segment
//! whose three upper coefficients are all zero while the next key has a different value is
//! *stepped* (the value jumps at the next key).
//!
//! The streamed layout follows what AssetStudio and AssetRipper (both MIT) document.

use std::collections::BTreeMap;

use anyhow::{bail, ensure, Result};

use crate::unity::{AnimationClip, GenericBinding};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub time: f32,
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

impl Key {
    pub fn constant(time: f32, value: f32) -> Key {
        Key {
            time,
            a: 0.0,
            b: 0.0,
            c: 0.0,
            d: value,
        }
    }

    // Evaluated in double precision: short segments have huge coefficients that cancel
    fn at(&self, s: f32) -> f32 {
        let (a, b, c, d, s) = (
            self.a as f64,
            self.b as f64,
            self.c as f64,
            self.d as f64,
            s as f64,
        );
        (((a * s + b) * s + c) * s + d) as f32
    }

    fn slope_at(&self, s: f32) -> f32 {
        let (a, b, c, s) = (self.a as f64, self.b as f64, self.c as f64, s as f64);
        ((3.0 * a * s + 2.0 * b) * s + c) as f32
    }

    fn is_flat(&self) -> bool {
        self.a == 0.0 && self.b == 0.0 && self.c == 0.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct Curve {
    /// Keys by increasing time.
    pub keys: Vec<Key>,
    /// The value before the first key, from the streamed clip's leading frame.
    pub initial: Option<f32>,
}

impl Curve {
    /// Index of the segment containing `t`: the last key at or before it.
    fn segment(&self, t: f32) -> Option<usize> {
        let n = self.keys.partition_point(|k| k.time <= t);
        n.checked_sub(1)
    }

    pub fn value(&self, t: f32) -> f32 {
        match self.segment(t) {
            Some(i) => self.keys[i].at(t - self.keys[i].time),
            None => self.keys.first().map_or(0.0, |k| k.d),
        }
    }

    /// Slope just after `t`.
    pub fn slope_out(&self, t: f32) -> f32 {
        match self.segment(t) {
            Some(i) => self.keys[i].slope_at(t - self.keys[i].time),
            None => 0.0,
        }
    }

    /// Slope just before `t`.
    pub fn slope_in(&self, t: f32) -> f32 {
        let n = self.keys.partition_point(|k| k.time < t);
        match n.checked_sub(1) {
            Some(i) => self.keys[i].slope_at(t - self.keys[i].time),
            None => 0.0,
        }
    }

    /// Whether the segment that is active just after `t` jumps to the next key's value.
    pub fn stepped_after(&self, t: f32) -> bool {
        match self.segment(t) {
            Some(i) if i + 1 < self.keys.len() => {
                self.keys[i].is_flat() && self.keys[i + 1].d != self.keys[i].d
            }
            _ => false,
        }
    }

    /// Cuts the curve to `start..=stop`: the key in effect at `start` moves there (its polynomial
    /// re-expressed from the new origin, which changes nothing) and later keys beyond `stop` go.
    /// Returns how many keys were affected.
    fn trim(&mut self, start: f32, stop: f32) -> usize {
        let mut touched = 0;
        let first_in = self.keys.partition_point(|k| k.time < start);
        if first_in > 0 && self.keys.get(first_in).is_none_or(|k| k.time > start) {
            // The key before `start` is in effect at `start`
            let k = self.keys[first_in - 1];
            let s = start - k.time;
            let moved = Key {
                time: start,
                a: k.a,
                b: 3.0 * k.a * s + k.b,
                c: 3.0 * k.a * s * s + 2.0 * k.b * s + k.c,
                d: k.at(s),
            };
            self.keys.drain(..first_in);
            self.keys.insert(0, moved);
            touched += first_in;
        } else if first_in > 0 {
            self.keys.drain(..first_in);
            touched += first_in;
        }
        let end = self.keys.partition_point(|k| k.time <= stop);
        if end < self.keys.len() {
            touched += self.keys.len() - end;
            self.keys.truncate(end.max(1));
            // The last kept key's segment reaches `stop`: end the curve there with its value
            let last = *self.keys.last().unwrap();
            if last.time < stop {
                self.keys
                    .push(Key::constant(stop, last.at(stop - last.time)));
            }
        }
        touched
    }

    /// Whether every segment is a straight line (or a hold).
    pub fn is_linear(&self) -> bool {
        self.keys.iter().all(|k| k.a == 0.0 && k.b == 0.0)
    }

    pub fn is_constant(&self) -> bool {
        self.keys.len() <= 1
    }
}

/// What a run of curves drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop {
    Position,
    Rotation,
    Scale,
    /// Euler angles in degrees
    Euler,
}

/// Binding type ids and attribute numbers for Transforms.
pub const TYPE_TRANSFORM: i32 = 4;

/// A binding with the curves it owns.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub binding: GenericBinding,
    pub first_curve: usize,
    pub curve_count: usize,
    /// `None` when the binding drives something other than a Transform's TRS.
    pub prop: Option<Prop>,
}

pub fn slots(bindings: &[GenericBinding]) -> Vec<Slot> {
    let mut out = Vec::with_capacity(bindings.len());
    let mut at = 0;
    for b in bindings {
        let (prop, n) = if b.typeID == TYPE_TRANSFORM {
            match b.attribute {
                1 => (Some(Prop::Position), 3),
                2 => (Some(Prop::Rotation), 4),
                3 => (Some(Prop::Scale), 3),
                4 => (Some(Prop::Euler), 3),
                _ => (None, 1),
            }
        } else {
            (None, 1)
        };
        out.push(Slot {
            binding: *b,
            first_curve: at,
            curve_count: n,
            prop,
        });
        at += n;
    }
    out
}

/// All curves of a clip in binding order, plus its time range.
#[derive(Debug, Default)]
pub struct ClipCurves {
    pub curves: Vec<Curve>,
    pub start: f32,
    pub stop: f32,
    /// Smooth segments that end more than 1e-3 away from the next key's value. Zero when the
    /// polynomial layout is read right.
    pub gaps: usize,
    /// Keys that lay outside `start..=stop` and were cut or moved to the range's edge
    pub trimmed_keys: usize,
}

/// Splits a clip's three curve groups into [`Curve`]s.
pub fn read_curves(clip: &AnimationClip) -> Result<ClipCurves> {
    let m = &clip.m_MuscleClip;
    let c = &m.m_Clip.data;
    let sc = &c.m_StreamedClip;
    let dc = &c.m_DenseClip;
    let kc = &c.m_ConstantClip;
    let n_stream = sc.curveCount as usize;
    let n_dense = dc.m_CurveCount as usize;
    let mut curves = vec![Curve::default(); n_stream + n_dense + kc.data.len()];

    // Streamed
    let d = &sc.data;
    let mut i = 0;
    let mut last_time = f32::MIN;
    while i < d.len() {
        ensure!(i + 2 <= d.len(), "streamed clip ends inside a frame header");
        let time = f32::from_bits(d[i]);
        let count = d[i + 1] as usize;
        i += 2;
        ensure!(
            i + count * 5 <= d.len(),
            "streamed clip ends inside a frame of {count} keys"
        );
        ensure!(
            time >= last_time,
            "streamed frame times go backwards ({last_time} then {time})"
        );
        last_time = time;
        for k in 0..count {
            let idx = d[i + k * 5] as usize;
            ensure!(idx < n_stream, "streamed key for curve {idx} of {n_stream}");
            let f = |j: usize| f32::from_bits(d[i + k * 5 + 1 + j]);
            let (a, b, cc, dd) = (f(0), f(1), f(2), f(3));
            if time <= f32::MIN {
                curves[idx].initial = Some(dd);
            } else if time.is_finite() {
                curves[idx].keys.push(Key {
                    time,
                    a,
                    b,
                    c: cc,
                    d: dd,
                });
            }
        }
        i += count * 5;
    }

    // Dense
    let frames = dc.m_FrameCount.max(0) as usize;
    if n_dense > 0 && frames > 0 {
        ensure!(
            dc.m_SampleArray.len() >= frames * n_dense,
            "dense clip holds {} samples for {frames} frames of {n_dense} curves",
            dc.m_SampleArray.len()
        );
        ensure!(dc.m_SampleRate > 0.0, "dense clip with sample rate 0");
        for k in 0..n_dense {
            let cv = &mut curves[n_stream + k];
            for f in 0..frames {
                let time = dc.m_BeginTime + f as f32 / dc.m_SampleRate;
                let v = dc.m_SampleArray[f * n_dense + k];
                let slope = if f + 1 < frames {
                    (dc.m_SampleArray[(f + 1) * n_dense + k] - v) * dc.m_SampleRate
                } else {
                    0.0
                };
                cv.keys.push(Key {
                    time,
                    a: 0.0,
                    b: 0.0,
                    c: slope,
                    d: v,
                });
            }
        }
    }

    // Constant
    for (k, &v) in kc.data.iter().enumerate() {
        curves[n_stream + n_dense + k]
            .keys
            .push(Key::constant(m.m_StartTime, v));
    }

    // Consistency: a smooth segment must arrive at the next key's value
    let mut gaps = 0;
    for cv in &curves {
        for w in cv.keys.windows(2) {
            // Segments a few microseconds long have coefficients far beyond what float times
            // can reproduce, so they are not a test of the layout
            if !w[0].is_flat() && w[1].time - w[0].time > 1e-4 {
                let end = w[0].at(w[1].time - w[0].time);
                if (end - w[1].d).abs() > 1e-3 * w[1].d.abs().max(1.0) {
                    gaps += 1;
                }
            }
        }
    }
    // Keys outside the clip's range: the clip plays from `start` to `stop`
    let (start, stop) = (m.m_StartTime, m.m_StopTime);
    let mut trimmed = 0;
    for cv in curves.iter_mut() {
        trimmed += cv.trim(start, stop);
    }
    Ok(ClipCurves {
        curves,
        start,
        stop,
        trimmed_keys: trimmed,
        gaps,
    })
}

/// How a built channel should be interpolated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interp {
    Step,
    Linear,
    Cubic,
}

/// One animated property of one node, ready for glTF: `values` has `comps` floats per key; for
/// [`Interp::Cubic`] the in and out tangents are in `in_tangents` and `out_tangents`, as slopes
/// (change per second) the way glTF stores them; the player multiplies them by the key interval.
#[derive(Clone, Debug)]
pub struct Channel {
    pub prop: Prop,
    pub interp: Interp,
    pub comps: usize,
    pub times: Vec<f32>,
    pub values: Vec<f32>,
    pub in_tangents: Vec<f32>,
    pub out_tangents: Vec<f32>,
    /// Jumps inside a smooth channel, each written as a key one float step before the jump
    pub steps_encoded: usize,
    /// Keys added to an Euler rotation channel to stay within [`EULER_TOLERANCE_DEG`]
    pub approximated: usize,
    /// Largest angle (degrees) between the written channel and the game's Euler curve at the
    /// midpoints checked
    pub max_error_deg: f32,
}

/// The largest float below `t` (for `t` > 0): a step is written as a key this close before the jump.
fn just_before(t: f32) -> f32 {
    if t > 0.0 {
        f32::from_bits(t.to_bits() - 1)
    } else {
        t
    }
}

/// Union of the key times of some curves, increasing.
fn union_times(comps: &[&Curve]) -> Vec<f32> {
    let mut times: Vec<f32> = comps
        .iter()
        .flat_map(|c| c.keys.iter().map(|k| k.time))
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times.dedup();
    times
}

/// Builds a channel from the component curves of one binding (3 for position and scale, 4 for
/// rotation).
///
/// - Curves made only of holds give a [`Interp::Step`] channel.
/// - Straight segments only give [`Interp::Linear`].
/// - Anything else gives [`Interp::Cubic`], which reproduces the polynomials exactly. A hold
///   followed by a jump inside such a channel is written as a key one float step before the jump
///   (`steps_encoded` counts them), as a cubic spline has no other way to jump.
pub fn build_channel(prop: Prop, comps: &[&Curve]) -> Result<Channel> {
    let n = comps.len();
    if comps.iter().any(|c| c.keys.is_empty()) {
        bail!("a curve without keys");
    }
    let mut times = union_times(comps);
    let all_flat = comps.iter().all(|c| c.keys.iter().all(|k| k.is_flat()));
    let linear = comps.iter().all(|c| c.is_linear());
    let mut steps_encoded = 0;
    if !all_flat {
        // Add the key before each jump
        let mut extra = Vec::new();
        for w in times.windows(2) {
            if comps.iter().any(|c| c.stepped_after(w[0])) {
                let t = just_before(w[1]);
                if t > w[0] {
                    extra.push(t);
                    steps_encoded += 1;
                }
            }
        }
        times.extend(extra);
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        times.dedup();
    }
    let keyframes = times.len();
    let mut values = vec![0.0; keyframes * n];
    let mut in_t = vec![0.0; keyframes * n];
    let mut out_t = vec![0.0; keyframes * n];
    for (ci, c) in comps.iter().enumerate() {
        for (ti, &t) in times.iter().enumerate() {
            values[ti * n + ci] = c.value(t);
            if ti + 1 < keyframes {
                out_t[ti * n + ci] = c.slope_out(t);
            }
            if ti > 0 {
                in_t[ti * n + ci] = c.slope_in(t);
            }
        }
    }
    let interp = if all_flat {
        Interp::Step
    } else if linear && steps_encoded == 0 {
        Interp::Linear
    } else {
        Interp::Cubic
    };
    Ok(Channel {
        prop,
        interp,
        comps: n,
        times,
        values,
        in_tangents: in_t,
        out_tangents: out_t,
        steps_encoded,
        approximated: 0,
        max_error_deg: 0.0,
    })
}

/// How far (in degrees) a written Euler channel may stray from the game's curve between keys.
pub const EULER_TOLERANCE_DEG: f32 = 0.05;

/// Halvings of an interval allowed when adding keys to an Euler channel.
pub const EULER_MAX_DEPTH: u32 = 5;

fn quat_dot(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn quat_angle_deg(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    2.0 * quat_dot(a, b).abs().clamp(0.0, 1.0).acos().to_degrees()
}

fn quat_slerp(a: &[f32; 4], b: &[f32; 4], u: f32) -> [f32; 4] {
    let v = slerp(a, b, u);
    [v[0], v[1], v[2], v[3]]
}

fn euler_at(comps: &[&Curve], order: [usize; 3], t: f32) -> [f32; 4] {
    euler_to_quaternion(
        [comps[0].value(t), comps[1].value(t), comps[2].value(t)],
        order,
    )
}

/// Builds a rotation channel from the three Euler-angle curves of a binding.
///
/// Mecanim turns the interpolated angles into a rotation at every instant, which is not a
/// polynomial in the quaternion. The channel holds the rotation at every key of the curves
/// exactly, and where linear (spherical) interpolation between two keys would stray from the
/// game's curve by more than [`EULER_TOLERANCE_DEG`] it holds extra keys, found by halving the
/// interval (`approximated` counts them). A curve with a single key is exact.
pub fn build_euler_channel(comps: &[&Curve], order: [usize; 3]) -> Result<Channel> {
    ensure!(
        comps.len() == 3,
        "Euler binding with {} curves",
        comps.len()
    );
    if comps.iter().any(|c| c.keys.is_empty()) {
        bail!("a curve without keys");
    }
    let mut times = union_times(comps);
    // Holds followed by jumps, as in build_channel
    let mut extra = Vec::new();
    for w in times.windows(2) {
        if comps.iter().any(|c| c.stepped_after(w[0])) {
            let t = just_before(w[1]);
            if t > w[0] {
                extra.push(t);
            }
        }
    }
    times.extend(extra);
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times.dedup();

    // Keys with the shortest path between neighbours
    let mut keys: Vec<(f32, [f32; 4])> = Vec::with_capacity(times.len());
    for &t in &times {
        let mut q = euler_at(comps, order, t);
        if let Some((_, prev)) = keys.last() {
            if quat_dot(prev, &q) < 0.0 {
                q = q.map(|x| -x);
            }
        }
        keys.push((t, q));
    }
    let mut sub = Subdivide {
        comps,
        order,
        out: Vec::with_capacity(keys.len()),
        added: 0,
        max_err: 0.0,
    };
    for i in 0..keys.len() {
        sub.out.push(keys[i]);
        if i + 1 < keys.len() {
            sub.between(keys[i], keys[i + 1], 0);
        }
    }
    let (approximated, max_err) = (sub.added, sub.max_err);
    let mut ch_times = Vec::with_capacity(sub.out.len());
    let mut values = Vec::with_capacity(sub.out.len() * 4);
    for (t, q) in sub.out {
        ch_times.push(t);
        values.extend_from_slice(&q);
    }
    let interp = if ch_times.len() > 1 && comps.iter().all(|c| c.keys.iter().all(|k| k.is_flat())) {
        Interp::Step
    } else {
        Interp::Linear
    };
    let n = ch_times.len();
    Ok(Channel {
        prop: Prop::Rotation,
        interp,
        comps: 4,
        times: ch_times,
        values,
        in_tangents: vec![0.0; n * 4],
        out_tangents: vec![0.0; n * 4],
        steps_encoded: 0,
        approximated,
        max_error_deg: max_err,
    })
}

/// State for adding keys to an Euler channel.
struct Subdivide<'a> {
    comps: &'a [&'a Curve],
    order: [usize; 3],
    out: Vec<(f32, [f32; 4])>,
    added: usize,
    max_err: f32,
}

impl Subdivide<'_> {
    /// Pushes the keys needed strictly between `a` and `b` (not `b` itself).
    fn between(&mut self, a: (f32, [f32; 4]), b: (f32, [f32; 4]), depth: u32) {
        if depth >= EULER_MAX_DEPTH || b.0 - a.0 <= 4.0 * f32::EPSILON * b.0.abs().max(1.0) {
            return;
        }
        let tm = 0.5 * (a.0 + b.0);
        let mut truth = euler_at(self.comps, self.order, tm);
        if quat_dot(&a.1, &truth) < 0.0 {
            truth = truth.map(|x| -x);
        }
        let lerp = quat_slerp(&a.1, &b.1, 0.5);
        let err = quat_angle_deg(&truth, &lerp);
        if err <= EULER_TOLERANCE_DEG {
            self.max_err = self.max_err.max(err);
            return;
        }
        if depth + 1 >= EULER_MAX_DEPTH {
            self.max_err = self.max_err.max(err);
        }
        let mid = (tm, truth);
        self.between(a, mid, depth + 1);
        self.out.push(mid);
        self.added += 1;
        self.between(mid, b, depth + 1);
    }
}

/// Samples a channel at `t` the way a glTF player would (for tests and cross-checks): step,
/// linear (slerp for rotations) or cubic Hermite (component-wise, then normalised for rotations).
pub fn sample_channel(ch: &Channel, t: f32) -> Vec<f32> {
    let n = ch.comps;
    let last = ch.times.len() - 1;
    let get = |arr: &[f32], k: usize| arr[k * n..(k + 1) * n].to_vec();
    if t <= ch.times[0] {
        return get(&ch.values, 0);
    }
    if t >= ch.times[last] {
        return get(&ch.values, last);
    }
    let k = ch.times.partition_point(|&x| x <= t) - 1;
    let (t0, t1) = (ch.times[k], ch.times[k + 1]);
    let u = (t - t0) / (t1 - t0);
    let v0 = get(&ch.values, k);
    let v1 = get(&ch.values, k + 1);
    let mut out: Vec<f32> = match ch.interp {
        Interp::Step => v0.clone(),
        Interp::Linear => {
            if ch.prop == Prop::Rotation {
                slerp(&v0, &v1, u)
            } else {
                v0.iter().zip(&v1).map(|(a, b)| a + (b - a) * u).collect()
            }
        }
        Interp::Cubic => {
            let m0 = get(&ch.out_tangents, k);
            let m1 = get(&ch.in_tangents, k + 1);
            let dt = t1 - t0;
            let (u2, u3) = (u * u, u * u * u);
            (0..n)
                .map(|i| {
                    (2.0 * u3 - 3.0 * u2 + 1.0) * v0[i]
                        + (u3 - 2.0 * u2 + u) * dt * m0[i]
                        + (-2.0 * u3 + 3.0 * u2) * v1[i]
                        + (u3 - u2) * dt * m1[i]
                })
                .collect()
        }
    };
    if ch.prop == Prop::Rotation && ch.interp != Interp::Step {
        normalize(&mut out);
    }
    out
}

fn normalize(q: &mut [f32]) {
    let len = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    if len > 0.0 {
        for x in q.iter_mut() {
            *x /= len;
        }
    }
}

fn slerp(a: &[f32], b: &[f32], u: f32) -> Vec<f32> {
    let mut dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let mut b = b.to_vec();
    if dot < 0.0 {
        dot = -dot;
        for x in b.iter_mut() {
            *x = -*x;
        }
    }
    if dot > 0.9995 {
        let mut o: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x + (y - x) * u).collect();
        normalize(&mut o);
        return o;
    }
    let theta = dot.clamp(-1.0, 1.0).acos();
    let s = theta.sin();
    let (wa, wb) = (((1.0 - u) * theta).sin() / s, (u * theta).sin() / s);
    a.iter().zip(&b).map(|(x, y)| wa * x + wb * y).collect()
}

/// The order in which the three Euler rotations are applied: the axes in the order they turn.
/// Unity numbers them `XYZ = 0, XZY = 1, YZX = 2, YXZ = 3, ZXY = 4, ZYX = 5`; the generic binding's
/// `customType` holds the number for Euler rotations. `ZXY` is Unity's own order for
/// `Quaternion.Euler` and `localEulerAngles`.
pub fn euler_order(custom_type: u8) -> Option<[usize; 3]> {
    Some(match custom_type {
        0 => [0, 1, 2],
        1 => [0, 2, 1],
        2 => [1, 2, 0],
        3 => [1, 0, 2],
        4 => [2, 0, 1],
        5 => [2, 1, 0],
        _ => return None,
    })
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// A rotation of `deg` degrees about one axis (0 x, 1 y, 2 z).
fn axis_quaternion(axis: usize, deg: f32) -> [f32; 4] {
    let (s, c) = (deg.to_radians() * 0.5).sin_cos();
    let mut q = [0.0, 0.0, 0.0, c];
    q[axis] = s;
    q
}

/// The rotation of Euler angles in degrees (x, y, z), applied about the axes in `order` (see
/// [`euler_order`]): the first axis in the list turns first.
pub fn euler_to_quaternion(deg: [f32; 3], order: [usize; 3]) -> [f32; 4] {
    let q0 = axis_quaternion(order[0], deg[order[0]]);
    let q1 = axis_quaternion(order[1], deg[order[1]]);
    let q2 = axis_quaternion(order[2], deg[order[2]]);
    quat_mul(q2, quat_mul(q1, q0))
}

/// Counts of what a clip's bindings drive that this crate does not read.
pub fn unread_bindings(slots: &[Slot]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for s in slots.iter().filter(|s| s.prop.is_none()) {
        let b = &s.binding;
        let what = match (b.typeID, b.customType) {
            (1, _) => "GameObject active flag".to_string(),
            (137, 20) => "blend shape weight".to_string(),
            (137, 22) | (23, 22) => "renderer material property".to_string(),
            (t, c) => format!("type {t} custom {c}"),
        };
        *m.entry(what).or_default() += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unity::DenseClip;

    fn key(time: f32, a: f32, b: f32, c: f32, d: f32) -> Key {
        Key { time, a, b, c, d }
    }

    #[test]
    fn cubic_segment_round_trips_through_hermite() {
        // p(s) = 2 s^3 - 3 s^2 + 1 on [0, 1] has value 1 -> 0 with zero end slopes
        let c = Curve {
            keys: vec![key(0.0, 2.0, -3.0, 0.0, 1.0), key(1.0, 0.0, 0.0, 0.0, 0.0)],
            initial: None,
        };
        let ch = build_channel(Prop::Position, &[&c, &c, &c]).unwrap();
        assert_eq!(ch.interp, Interp::Cubic);
        for t in [0.0, 0.25, 0.5, 0.9] {
            let want = c.value(t);
            let got = sample_channel(&ch, t);
            assert!((got[0] - want).abs() < 1e-6, "{t}: {} vs {want}", got[0]);
        }
        // Out slope at 0 is 0; in slope at 1 is 6 - 6 = 0
        assert_eq!(ch.out_tangents[0], 0.0);
        assert!(ch.in_tangents[3].abs() < 1e-6);
    }

    #[test]
    fn in_slope_matches_the_polynomial_derivative() {
        // p(s) = s^2 on [0, 2]: slope 4 at the end
        let c = Curve {
            keys: vec![key(0.0, 0.0, 1.0, 0.0, 0.0), key(2.0, 0.0, 0.0, 0.0, 4.0)],
            initial: None,
        };
        assert_eq!(c.slope_in(2.0), 4.0);
        assert_eq!(c.value(1.0), 1.0);
    }

    #[test]
    fn stepped_segments_are_found() {
        let c = Curve {
            keys: vec![key(0.0, 0.0, 0.0, 0.0, 1.0), key(1.0, 0.0, 0.0, 0.0, 2.0)],
            initial: None,
        };
        assert!(c.stepped_after(0.0));
        assert!(!c.stepped_after(1.0));
        let ch = build_channel(Prop::Scale, &[&c, &c, &c]).unwrap();
        assert_eq!(ch.interp, Interp::Step);
    }

    #[test]
    fn dense_linear_curve_is_linear_channel() {
        let c = Curve {
            keys: vec![key(0.0, 0.0, 0.0, 2.0, 0.0), key(1.0, 0.0, 0.0, 0.0, 2.0)],
            initial: None,
        };
        let ch = build_channel(Prop::Position, &[&c, &c, &c]).unwrap();
        assert_eq!(ch.interp, Interp::Linear);
        assert_eq!(sample_channel(&ch, 0.5)[0], 1.0);
    }

    fn clip_with(
        streamed: Vec<u32>,
        curves: u32,
        dense: DenseClip,
        constant: Vec<f32>,
    ) -> AnimationClip {
        use crate::unity::{
            ClipBindingConstant, ClipMuscleConstant, ConstantClip, OffsetPtrClip, StreamedClip,
        };
        AnimationClip {
            m_Name: "t".into(),
            m_Legacy: false,
            m_Compressed: false,
            m_RotationCurves: vec![],
            m_CompressedRotationCurves: vec![],
            m_EulerCurves: vec![],
            m_PositionCurves: vec![],
            m_ScaleCurves: vec![],
            m_FloatCurves: vec![],
            m_SampleRate: 30.0,
            m_WrapMode: 0,
            m_MuscleClip: ClipMuscleConstant {
                m_Clip: OffsetPtrClip {
                    data: crate::unity::Clip {
                        m_StreamedClip: StreamedClip {
                            data: streamed,
                            curveCount: curves,
                        },
                        m_DenseClip: dense,
                        m_ConstantClip: ConstantClip { data: constant },
                    },
                },
                m_StartTime: 0.0,
                m_StopTime: 1.0,
                m_LoopTime: false,
            },
            m_ClipBindingConstant: ClipBindingConstant::default(),
        }
    }

    fn frame(time: f32, keys: &[(u32, [f32; 4])]) -> Vec<u32> {
        let mut v = vec![time.to_bits(), keys.len() as u32];
        for (c, co) in keys {
            v.push(*c);
            v.extend(co.iter().map(|x| x.to_bits()));
        }
        v
    }

    #[test]
    fn reads_streamed_dense_and_constant_curves() {
        let mut data = Vec::new();
        // The leading frame (the lowest float) holds the value before the first key
        data.extend(frame(
            f32::MIN,
            &[(0, [0.0, 0.0, 0.0, 5.0]), (1, [0.0, 0.0, 0.0, 6.0])],
        ));
        // Curve 0: p(s) = 2 s^3 - 3 s^2 + 1, from 1 at t=0 to 0 at t=1; curve 1: constant 6 then 7
        data.extend(frame(
            0.0,
            &[(0, [2.0, -3.0, 0.0, 1.0]), (1, [0.0, 0.0, 0.0, 6.0])],
        ));
        data.extend(frame(
            1.0,
            &[(0, [0.0, 0.0, 0.0, 0.0]), (1, [0.0, 0.0, 0.0, 7.0])],
        ));
        data.extend(frame(f32::INFINITY, &[]));
        let dense = DenseClip {
            m_FrameCount: 3,
            m_CurveCount: 1,
            m_SampleRate: 2.0,
            m_BeginTime: 0.0,
            m_SampleArray: vec![0.0, 1.0, 4.0],
        };
        let c = clip_with(data, 2, dense, vec![9.0]);
        let cc = read_curves(&c).unwrap();
        assert_eq!(cc.curves.len(), 4);
        assert_eq!(cc.gaps, 0);
        // Curve 0 reproduces the polynomial; the leading value is kept apart
        assert_eq!(cc.curves[0].initial, Some(5.0));
        assert_eq!(cc.curves[0].keys.len(), 2);
        assert!((cc.curves[0].value(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(cc.curves[0].value(1.0), 0.0);
        // Curve 1 holds 6 until its key at 1.0, where it jumps to 7
        assert_eq!(cc.curves[1].value(0.999), 6.0);
        assert!(cc.curves[1].stepped_after(0.0));
        // Dense: three frames half a second apart, read linearly
        assert_eq!(cc.curves[2].value(0.25), 0.5);
        assert_eq!(cc.curves[2].value(0.75), 2.5);
        // Constant
        assert_eq!(cc.curves[3].value(0.4), 9.0);
    }

    #[test]
    fn malformed_streams_are_errors() {
        // A frame announcing two keys but holding one
        let mut data = frame(0.0, &[(0, [0.0; 4])]);
        data[1] = 2;
        let c = clip_with(data, 1, DenseClip::default(), vec![]);
        assert!(read_curves(&c).is_err());
        // A key for a curve that does not exist
        let c = clip_with(
            frame(0.0, &[(3, [0.0; 4])]),
            1,
            DenseClip::default(),
            vec![],
        );
        assert!(read_curves(&c).is_err());
    }

    #[test]
    fn keys_before_the_start_move_to_it() {
        // p(s) = s on [-1, 3]: the key at -1 is in effect at the start (0) with value 1
        let mut c = Curve {
            keys: vec![key(-1.0, 0.0, 0.0, 1.0, 0.0), key(3.0, 0.0, 0.0, 0.0, 4.0)],
            initial: None,
        };
        assert_eq!(c.trim(0.0, 2.0), 1 + 1);
        assert_eq!(c.keys[0].time, 0.0);
        assert_eq!(c.keys[0].d, 1.0);
        assert_eq!(c.value(1.5), 2.5);
        // The curve ends at the clip's stop time with the value it reaches there
        assert_eq!(c.keys.len(), 2);
        assert_eq!((c.keys[1].time, c.keys[1].d), (2.0, 3.0));
    }

    #[test]
    fn bindings_own_runs_of_curves() {
        let b = |attr| GenericBinding {
            path: 0,
            attribute: attr,
            typeID: TYPE_TRANSFORM,
            customType: 0,
            isPPtrCurve: 0,
        };
        let other = GenericBinding {
            typeID: 137,
            customType: 20,
            ..b(5)
        };
        let s = slots(&[b(1), b(2), other, b(3)]);
        let firsts: Vec<_> = s.iter().map(|s| (s.first_curve, s.curve_count)).collect();
        assert_eq!(firsts, [(0, 3), (3, 4), (7, 1), (8, 3)]);
        assert_eq!(s[2].prop, None);
    }

    #[test]
    fn euler_orders() {
        let zxy = euler_order(4).unwrap();
        // 90 degrees about Y alone
        let q = euler_to_quaternion([0.0, 90.0, 0.0], zxy);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        assert!((q[1] - r).abs() < 1e-6 && (q[3] - r).abs() < 1e-6);
        // ZXY applies Z first: +X turned 90 about Z becomes +Y, which a 90 turn about Y leaves alone
        let v = rotate(euler_to_quaternion([0.0, 90.0, 90.0], zxy), [1.0, 0.0, 0.0]);
        assert!(
            v[1] > 0.99 && v[0].abs() < 1e-5 && v[2].abs() < 1e-5,
            "{v:?}"
        );
        // XYZ applies Y before Z: +X is turned about Y (into -Z) and then about Z (which leaves -Z)
        let xyz = euler_order(0).unwrap();
        let v = rotate(euler_to_quaternion([0.0, 90.0, 90.0], xyz), [1.0, 0.0, 0.0]);
        assert!(
            v[2] < -0.99 && v[0].abs() < 1e-5 && v[1].abs() < 1e-5,
            "{v:?}"
        );
        assert_eq!(euler_order(9), None);
    }

    fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
        let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
        let t = [
            2.0 * (y * v[2] - z * v[1]),
            2.0 * (z * v[0] - x * v[2]),
            2.0 * (x * v[1] - y * v[0]),
        ];
        [
            v[0] + w * t[0] + (y * t[2] - z * t[1]),
            v[1] + w * t[1] + (z * t[0] - x * t[2]),
            v[2] + w * t[2] + (x * t[1] - y * t[0]),
        ]
    }
}
