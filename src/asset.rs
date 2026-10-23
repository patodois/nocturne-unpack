//! A decoded, game-neutral model in glTF's conventions (right-handed, +Y up, metres, UVs with
//! the origin at the top left), shared by the glTF writer and the rewrite-bridge provider.

use std::collections::BTreeMap;

pub use crate::texture::Image;

#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// One draw call: a sub-mesh with its own trimmed vertex arrays.
#[derive(Clone, Debug, Default)]
pub struct Primitive {
    pub name: String,
    /// The Unity object id of the mesh this sub-mesh came from
    pub mesh_id: i64,
    /// The node that carries the mesh
    pub node: usize,
    pub skin: Option<usize>,
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 4]>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct Skin {
    pub joints: Vec<usize>,
    /// Column-major, one per joint
    pub inverse_bind: Vec<[f32; 16]>,
    /// The renderer's root bone
    pub skeleton: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    Mask(f32),
    Blend,
}

/// glTF sampler values (GL enums), from the texture's own settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sampler {
    pub mag_filter: u32,
    pub min_filter: u32,
    pub wrap_s: u32,
    pub wrap_t: u32,
}

#[derive(Clone, Debug)]
pub struct TextureRef {
    pub image: usize,
    pub sampler: Sampler,
    /// `KHR_texture_transform` offset and scale, when the material's tiling is not the identity
    pub transform: Option<([f32; 2], [f32; 2])>,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub shader: String,
    pub keywords: Vec<String>,
    pub base_color: Option<TextureRef>,
    /// Linear RGBA
    pub base_color_factor: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub alpha: AlphaMode,
    /// The shader reads vertex colours (keyword `ENABLE_VERTEXCOLOR`)
    pub vertex_colors: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Translation,
    Rotation,
    Scale,
}

impl Target {
    pub fn components(self) -> usize {
        if self == Target::Rotation {
            4
        } else {
            3
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
    CubicSpline,
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub node: usize,
    pub target: Target,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    /// glTF layout: for a cubic spline, in-tangent, value, out-tangent per key
    pub values: Vec<f32>,
}

impl Channel {
    /// The channel's value at `t` seconds, the way glTF defines it: held before the first and after
    /// the last key; step, linear (spherical for rotations) or cubic Hermite in between.
    pub fn sample(&self, t: f32) -> Vec<f32> {
        let n = self.target.components();
        let cubic = self.interpolation == Interpolation::CubicSpline;
        let per_key = if cubic { 3 * n } else { n };
        let value = |k: usize| -> &[f32] {
            let at = k * per_key + if cubic { n } else { 0 };
            &self.values[at..at + n]
        };
        let last = self.times.len() - 1;
        if t <= self.times[0] {
            return value(0).to_vec();
        }
        if t >= self.times[last] {
            return value(last).to_vec();
        }
        let k = self.times.partition_point(|&x| x <= t) - 1;
        let (t0, t1) = (self.times[k], self.times[k + 1]);
        let u = (t - t0) / (t1 - t0);
        let (v0, v1) = (value(k), value(k + 1));
        let mut out: Vec<f32> = match self.interpolation {
            Interpolation::Step => v0.to_vec(),
            Interpolation::Linear if self.target == Target::Rotation => slerp(v0, v1, u),
            Interpolation::Linear => v0.iter().zip(v1).map(|(a, b)| a + (b - a) * u).collect(),
            Interpolation::CubicSpline => {
                let dt = t1 - t0;
                let out_tangent = &self.values[k * per_key + 2 * n..k * per_key + 3 * n];
                let in_tangent = &self.values[(k + 1) * per_key..(k + 1) * per_key + n];
                let (u2, u3) = (u * u, u * u * u);
                (0..n)
                    .map(|i| {
                        (2.0 * u3 - 3.0 * u2 + 1.0) * v0[i]
                            + (u3 - 2.0 * u2 + u) * dt * out_tangent[i]
                            + (-2.0 * u3 + 3.0 * u2) * v1[i]
                            + (u3 - u2) * dt * in_tangent[i]
                    })
                    .collect()
            }
        };
        if self.target == Target::Rotation && self.interpolation != Interpolation::Step {
            let len = out.iter().map(|x| x * x).sum::<f32>().sqrt();
            if len > 0.0 {
                out.iter_mut().for_each(|x| *x /= len);
            }
        }
        out
    }
}

fn slerp(a: &[f32], b: &[f32], u: f32) -> Vec<f32> {
    let mut dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let mut b = b.to_vec();
    if dot < 0.0 {
        dot = -dot;
        b.iter_mut().for_each(|x| *x = -*x);
    }
    if dot > 0.9995 {
        return a.iter().zip(&b).map(|(x, y)| x + (y - x) * u).collect();
    }
    let theta = dot.clamp(-1.0, 1.0).acos();
    let s = theta.sin();
    let (wa, wb) = (((1.0 - u) * theta).sin() / s, (u * theta).sin() / s);
    a.iter().zip(&b).map(|(x, y)| wa * x + wb * y).collect()
}

#[derive(Clone, Debug)]
pub struct Animation {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<Channel>,
}

/// Counts of things the file has that were not carried over, by description.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub left_out: BTreeMap<String, usize>,
    /// Material properties that were not mapped: shader, then property name, then how many
    /// materials set it
    pub unmapped: BTreeMap<String, BTreeMap<String, usize>>,
}

impl Report {
    pub fn add(&mut self, what: impl Into<String>) {
        self.add_n(what, 1);
    }

    pub fn add_n(&mut self, what: impl Into<String>, n: usize) {
        if n == 0 {
            return;
        }
        *self.left_out.entry(what.into()).or_default() += n;
    }

    /// Notes a material property of a shader that is not carried over.
    pub fn add_unmapped(&mut self, shader: &str, property: &str) {
        *self
            .unmapped
            .entry(shader.to_string())
            .or_default()
            .entry(property.to_string())
            .or_default() += 1;
    }

    pub fn merge(&mut self, other: &Report) {
        for (k, v) in &other.left_out {
            self.add_n(k.clone(), *v);
        }
        for (shader, props) in &other.unmapped {
            for (p, n) in props {
                *self
                    .unmapped
                    .entry(shader.clone())
                    .or_default()
                    .entry(p.clone())
                    .or_default() += n;
            }
        }
    }

    /// Lines describing the left-out material properties, one per shader.
    pub fn unmapped_lines(&self, names: bool) -> Vec<String> {
        self.unmapped
            .iter()
            .map(|(shader, props)| {
                let mut line = format!("{shader}: {} properties not mapped", props.len());
                if names {
                    let list: Vec<&str> = props.keys().map(String::as_str).collect();
                    line.push_str(&format!(" ({})", list.join(", ")));
                }
                line
            })
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Asset {
    pub name: String,
    pub nodes: Vec<Node>,
    pub primitives: Vec<Primitive>,
    pub skins: Vec<Skin>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub animations: Vec<Animation>,
    pub report: Report,
}

/// Unity's left-handed space to glTF's right-handed one: x is negated.
pub mod space {
    pub fn position(p: [f32; 3]) -> [f32; 3] {
        [-p[0], p[1], p[2]]
    }

    /// A rotation: a mirror turns the axis of rotation's y and z around.
    pub fn rotation(q: [f32; 4]) -> [f32; 4] {
        [q[0], -q[1], -q[2], q[3]]
    }

    pub fn tangent(t: [f32; 4]) -> [f32; 4] {
        [-t[0], t[1], t[2], -t[3]]
    }

    /// `F M F` for a row-major matrix, returned column-major.
    pub fn matrix_to_column_major(m: &[[f32; 4]; 4]) -> [f32; 16] {
        let f = [-1.0, 1.0, 1.0, 1.0];
        std::array::from_fn(|k| {
            let (col, row) = (k / 4, k % 4);
            m[row][col] * f[row] * f[col]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::space::*;

    #[test]
    fn mirror_keeps_a_rotation_consistent() {
        // Rotating +Z by 90 degrees about Y in Unity (left-handed) takes it to +X; after the
        // mirror the same motion must take +Z to -X.
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let q = rotation([0.0, s, 0.0, s]);
        // Rotate [0, 0, 1] by q = (0, -s, 0, s): a +/-90 degree turn about Y
        let v = rotate(q, [0.0, 0.0, 1.0]);
        assert!((v[0] + 1.0).abs() < 1e-6 && v[2].abs() < 1e-6, "{v:?}");
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

    #[test]
    fn cubic_channel_samples_like_gltf() {
        use super::{Channel, Interpolation, Target};
        // Value 0 -> 1 over 2 s with out/in tangents 1 (per second): the Hermite curve through
        // the midpoint is (0 + 1)/2 + dt/8 * (m0 - m1) = 0.5
        let ch = Channel {
            node: 0,
            target: Target::Translation,
            interpolation: Interpolation::CubicSpline,
            times: vec![0.0, 2.0],
            values: vec![
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0,
                0.0, 0.0,
            ],
        };
        // Tangents (0,0,0) in, value (0,0,0), out (1,0,0); second key in (1,0,0), value (1,0,0)...
        let mid = ch.sample(1.0);
        assert!((mid[0] - 0.5).abs() < 1e-6, "{mid:?}");
    }

    #[test]
    fn matrix_translation_is_mirrored() {
        let m = [
            [1.0, 0.0, 0.0, 5.0],
            [0.0, 1.0, 0.0, 6.0],
            [0.0, 0.0, 1.0, 7.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let c = matrix_to_column_major(&m);
        // Translation sits at elements 12..15 of a column-major matrix, with x negated
        assert_eq!(&c[12..15], &[-5.0, 6.0, 7.0]);
        assert_eq!(c[0], 1.0);
    }
}
