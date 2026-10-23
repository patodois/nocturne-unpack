//! World transforms of a node hierarchy, as translation, rotation and scale.

use crate::asset::Node;

type Mat3 = [[f64; 3]; 3];

/// A node's world transform: the matrix is `T * R * S`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct World {
    pub translation: [f32; 3],
    /// Unit quaternion x, y, z, w
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// The world matrix has shear (a rotated child of a non-uniformly scaled parent), which a
    /// translation, rotation and scale cannot hold; the closest such transform is kept.
    pub sheared: bool,
}

fn mul3(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, e) in row.iter_mut().enumerate() {
            *e = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    m
}

fn quat_to_mat(q: [f32; 4]) -> Mat3 {
    let [x, y, z, w] = q.map(f64::from);
    let n = (x * x + y * y + z * z + w * w).sqrt();
    let (x, y, z, w) = (x / n, y / n, z / n, w / n);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

fn mat_to_quat(m: &Mat3) -> [f32; 4] {
    let trace = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w);
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        w = 0.25 * s;
        x = (m[2][1] - m[1][2]) / s;
        y = (m[0][2] - m[2][0]) / s;
        z = (m[1][0] - m[0][1]) / s;
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        w = (m[2][1] - m[1][2]) / s;
        x = 0.25 * s;
        y = (m[0][1] + m[1][0]) / s;
        z = (m[0][2] + m[2][0]) / s;
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        w = (m[0][2] - m[2][0]) / s;
        x = (m[0][1] + m[1][0]) / s;
        y = 0.25 * s;
        z = (m[1][2] + m[2][1]) / s;
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        w = (m[1][0] - m[0][1]) / s;
        x = (m[0][2] + m[2][0]) / s;
        y = (m[1][2] + m[2][1]) / s;
        z = 0.25 * s;
    }
    let n = (x * x + y * y + z * z + w * w).sqrt();
    [
        (x / n) as f32,
        (y / n) as f32,
        (z / n) as f32,
        (w / n) as f32,
    ]
}

/// World transform of every node (parents come first in `nodes`). A node whose scale has a zero
/// component comes out with that scale kept at zero.
pub fn world_transforms(nodes: &[Node]) -> Vec<World> {
    // Linear part and translation, in double precision
    let mut lin: Vec<Mat3> = Vec::with_capacity(nodes.len());
    let mut pos: Vec<[f64; 3]> = Vec::with_capacity(nodes.len());
    for n in nodes {
        let r = quat_to_mat(n.rotation);
        let s = n.scale.map(f64::from);
        let mut local: Mat3 = [[0.0; 3]; 3];
        for (row, l) in local.iter_mut().enumerate() {
            for (col, e) in l.iter_mut().enumerate() {
                *e = r[row][col] * s[col];
            }
        }
        let t = n.translation.map(f64::from);
        match n.parent {
            Some(p) => {
                let pl = &lin[p];
                let pt = pos[p];
                let moved = [
                    pl[0][0] * t[0] + pl[0][1] * t[1] + pl[0][2] * t[2] + pt[0],
                    pl[1][0] * t[0] + pl[1][1] * t[1] + pl[1][2] * t[2] + pt[1],
                    pl[2][0] * t[0] + pl[2][1] * t[1] + pl[2][2] * t[2] + pt[2],
                ];
                lin.push(mul3(pl, &local));
                pos.push(moved);
            }
            None => {
                lin.push(local);
                pos.push(t);
            }
        }
    }
    lin.iter()
        .zip(&pos)
        .map(|(m, t)| {
            // Scale: the length of each column; a mirror (negative determinant) flips x
            let mut scale = [0.0f64; 3];
            for (c, s) in scale.iter_mut().enumerate() {
                *s = (m[0][c] * m[0][c] + m[1][c] * m[1][c] + m[2][c] * m[2][c]).sqrt();
            }
            let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
            if det < 0.0 {
                scale[0] = -scale[0];
            }
            let mut r: Mat3 = [[0.0; 3]; 3];
            for row in 0..3 {
                for col in 0..3 {
                    r[row][col] = if scale[col] == 0.0 {
                        f64::from(row == col)
                    } else {
                        m[row][col] / scale[col]
                    };
                }
            }
            // R^T R should be the identity
            let mut off = 0.0f64;
            for a in 0..3 {
                for b in 0..3 {
                    let dot: f64 = (0..3).map(|k| r[k][a] * r[k][b]).sum();
                    off = off.max((dot - f64::from(a == b)).abs());
                }
            }
            World {
                translation: t.map(|v| v as f32),
                rotation: mat_to_quat(&r),
                scale: scale.map(|v| v as f32),
                sheared: off > 1e-3,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(parent: Option<usize>, t: [f32; 3], q: [f32; 4], s: [f32; 3]) -> Node {
        Node {
            name: String::new(),
            parent,
            translation: t,
            rotation: q,
            scale: s,
        }
    }

    const R: f32 = std::f32::consts::FRAC_1_SQRT_2;

    fn near(a: &[f32], b: &[f32]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn child_follows_parent_rotation_and_scale() {
        // Parent: moved to x = 10, turned 90 degrees about Y, scaled 2. Child: 1 along local x.
        let nodes = [
            node(None, [10.0, 0.0, 0.0], [0.0, R, 0.0, R], [2.0; 3]),
            node(Some(0), [1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
        ];
        let w = world_transforms(&nodes);
        // Local +x turns to -z (rotation about +y takes +x to -z), twice as long
        assert!(
            near(&w[1].translation, &[10.0, 0.0, -2.0]),
            "{:?}",
            w[1].translation
        );
        assert!(near(&w[1].scale, &[2.0; 3]));
        assert!(near(&w[1].rotation, &[0.0, R, 0.0, R]));
        assert!(!w[1].sheared);
    }

    #[test]
    fn mirror_gives_a_negative_scale() {
        let nodes = [
            node(None, [0.0; 3], [0.0, 0.0, 0.0, 1.0], [-1.0, 1.0, 1.0]),
            node(Some(0), [1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
        ];
        let w = world_transforms(&nodes);
        assert!(near(&w[1].translation, &[-1.0, 0.0, 0.0]));
        assert_eq!(w[1].scale, [-1.0, 1.0, 1.0]);
        assert!(near(&w[1].rotation, &[0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn shear_is_flagged() {
        // A parent stretched along x with a child turned 45 degrees about z: the product shears
        let q = [0.0, 0.0, 0.382_683_43, 0.923_879_5];
        let nodes = [
            node(None, [0.0; 3], [0.0, 0.0, 0.0, 1.0], [3.0, 1.0, 1.0]),
            node(Some(0), [0.0; 3], q, [1.0; 3]),
        ];
        let w = world_transforms(&nodes);
        assert!(w[1].sheared);
        assert!(!w[0].sheared);
    }

    #[test]
    fn rotations_survive_a_round_trip() {
        for q in [
            [0.5, 0.5, 0.5, 0.5],
            [0.0, 1.0, 0.0, 0.0],
            [R, 0.0, 0.0, R],
            [0.1, -0.7, 0.2, 0.6782],
        ] {
            let n = (q.iter().map(|x| x * x).sum::<f32>()).sqrt();
            let q = q.map(|x| x / n);
            let w = world_transforms(&[node(None, [0.0; 3], q, [1.0; 3])]);
            // q and -q are the same rotation
            let d: f32 = w[0].rotation.iter().zip(&q).map(|(a, b)| a * b).sum();
            assert!(d.abs() > 1.0 - 1e-5, "{q:?} -> {:?}", w[0].rotation);
        }
    }
}
