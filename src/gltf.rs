//! Writes an [`Asset`] as a binary glTF 2.0 file (`.glb`) with everything embedded.

use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{json, Value};

use crate::asset::{AlphaMode, Asset, Interpolation, Target};

struct Buffer {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Buffer {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(
        &mut self,
        view: usize,
        component: u32,
        count: usize,
        kind: &str,
        min_max: Option<(Vec<f32>, Vec<f32>)>,
    ) -> usize {
        let mut a =
            json!({"bufferView": view, "componentType": component, "count": count, "type": kind});
        if let Some((min, max)) = min_max {
            a["min"] = json!(min);
            a["max"] = json!(max);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn floats(
        &mut self,
        data: &[f32],
        kind: &str,
        width: usize,
        target: Option<u32>,
        min_max: bool,
    ) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, target);
        let count = data.len() / width;
        let mm = min_max.then(|| {
            let mut min = vec![f32::INFINITY; width];
            let mut max = vec![f32::NEG_INFINITY; width];
            for c in data.chunks_exact(width) {
                for k in 0..width {
                    min[k] = min[k].min(c[k]);
                    max[k] = max[k].max(c[k]);
                }
            }
            (min, max)
        });
        self.accessor(view, 5126, count, kind, mm)
    }
}

fn png_bytes(img: &crate::asset::Image) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&img.rgba)?;
    Ok(out)
}

/// Builds the `.glb` bytes of an asset.
pub fn to_glb(a: &Asset) -> Result<Vec<u8>> {
    let mut b = Buffer {
        bin: Vec::new(),
        views: Vec::new(),
        accessors: Vec::new(),
    };
    let mut doc = json!({"asset": {"version": "2.0", "generator": "nocturne-unpack"}});

    // Nodes
    let mut children: Vec<Vec<usize>> = vec![vec![]; a.nodes.len()];
    let mut roots = Vec::new();
    for (i, n) in a.nodes.iter().enumerate() {
        match n.parent {
            Some(p) => children[p].push(i),
            None => roots.push(i),
        }
    }
    let mut nodes: Vec<Value> = a
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut v = json!({"name": n.name});
            if n.translation != [0.0; 3] {
                v["translation"] = json!(n.translation);
            }
            if n.rotation != [0.0, 0.0, 0.0, 1.0] {
                v["rotation"] = json!(n.rotation);
            }
            if n.scale != [1.0; 3] {
                v["scale"] = json!(n.scale);
            }
            if !children[i].is_empty() {
                v["children"] = json!(children[i]);
            }
            v
        })
        .collect();

    // Images, samplers, textures
    let mut images = Vec::new();
    if !a.images.is_empty() {
        for img in &a.images {
            let png = png_bytes(img)?;
            let view = b.view(&png, None);
            images.push(json!({"bufferView": view, "mimeType": "image/png"}));
        }
    }
    let mut samplers: Vec<Value> = Vec::new();
    let mut textures: Vec<Value> = Vec::new();
    let mut tex_index: BTreeMap<(usize, [u32; 4]), usize> = BTreeMap::new();

    // Materials
    let mut materials = Vec::new();
    let mut uses_transform = false;
    for m in &a.materials {
        let mut pbr = json!({
            "baseColorFactor": m.base_color_factor,
            "metallicFactor": m.metallic,
            "roughnessFactor": m.roughness,
        });
        if let Some(t) = &m.base_color {
            let key = (
                t.image,
                [
                    t.sampler.mag_filter,
                    t.sampler.min_filter,
                    t.sampler.wrap_s,
                    t.sampler.wrap_t,
                ],
            );
            let ti = *tex_index.entry(key).or_insert_with(|| {
                samplers.push(json!({"magFilter": t.sampler.mag_filter, "minFilter": t.sampler.min_filter, "wrapS": t.sampler.wrap_s, "wrapT": t.sampler.wrap_t}));
                textures.push(json!({"sampler": samplers.len() - 1, "source": t.image}));
                textures.len() - 1
            });
            let mut info = json!({"index": ti});
            if let Some((offset, scale)) = t.transform {
                uses_transform = true;
                info["extensions"] =
                    json!({"KHR_texture_transform": {"offset": offset, "scale": scale}});
            }
            pbr["baseColorTexture"] = info;
        }
        let mut v = json!({
            "name": m.name,
            "pbrMetallicRoughness": pbr,
            "extras": {"shader": m.shader, "keywords": m.keywords},
        });
        match m.alpha {
            AlphaMode::Opaque => {}
            AlphaMode::Mask(c) => {
                v["alphaMode"] = json!("MASK");
                v["alphaCutoff"] = json!(c);
            }
            AlphaMode::Blend => v["alphaMode"] = json!("BLEND"),
        }
        materials.push(v);
    }

    // Skins
    let mut skins = Vec::new();
    for s in &a.skins {
        let flat: Vec<f32> = s.inverse_bind.iter().flatten().copied().collect();
        let acc = b.floats(&flat, "MAT4", 16, None, false);
        let mut v = json!({"joints": s.joints, "inverseBindMatrices": acc});
        if let Some(sk) = s.skeleton {
            v["skeleton"] = json!(sk);
        }
        skins.push(v);
    }

    // Meshes: one glTF mesh per node, a primitive per sub-mesh
    let mut meshes = Vec::new();
    let mut by_node: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, p) in a.primitives.iter().enumerate() {
        by_node.entry(p.node).or_default().push(i);
    }
    for (node, prims) in &by_node {
        let mut jprims = Vec::new();
        for &pi in prims {
            let p = &a.primitives[pi];
            let mut attrs = serde_json::Map::new();
            let flat3: Vec<f32> = p.positions.iter().flatten().copied().collect();
            attrs.insert(
                "POSITION".into(),
                json!(b.floats(&flat3, "VEC3", 3, Some(34962), true)),
            );
            if !p.normals.is_empty() {
                let f: Vec<f32> = p.normals.iter().flatten().copied().collect();
                attrs.insert(
                    "NORMAL".into(),
                    json!(b.floats(&f, "VEC3", 3, Some(34962), false)),
                );
            }
            if !p.tangents.is_empty() {
                let f: Vec<f32> = p.tangents.iter().flatten().copied().collect();
                attrs.insert(
                    "TANGENT".into(),
                    json!(b.floats(&f, "VEC4", 4, Some(34962), false)),
                );
            }
            if !p.uvs.is_empty() {
                let f: Vec<f32> = p.uvs.iter().flatten().copied().collect();
                attrs.insert(
                    "TEXCOORD_0".into(),
                    json!(b.floats(&f, "VEC2", 2, Some(34962), false)),
                );
            }
            if !p.colors.is_empty() {
                let f: Vec<f32> = p.colors.iter().flatten().copied().collect();
                attrs.insert(
                    "COLOR_0".into(),
                    json!(b.floats(&f, "VEC4", 4, Some(34962), false)),
                );
            }
            if !p.joints.is_empty() {
                let bytes: Vec<u8> = p
                    .joints
                    .iter()
                    .flatten()
                    .flat_map(|j| j.to_le_bytes())
                    .collect();
                let v = b.view(&bytes, Some(34962));
                attrs.insert(
                    "JOINTS_0".into(),
                    json!(b.accessor(v, 5123, p.joints.len(), "VEC4", None)),
                );
                let f: Vec<f32> = p.weights.iter().flatten().copied().collect();
                attrs.insert(
                    "WEIGHTS_0".into(),
                    json!(b.floats(&f, "VEC4", 4, Some(34962), false)),
                );
            }
            let wide = p.indices.iter().any(|&i| i > 65535);
            let (bytes, comp): (Vec<u8>, u32) = if wide {
                (
                    p.indices.iter().flat_map(|i| i.to_le_bytes()).collect(),
                    5125,
                )
            } else {
                (
                    p.indices
                        .iter()
                        .flat_map(|&i| (i as u16).to_le_bytes())
                        .collect(),
                    5123,
                )
            };
            let v = b.view(&bytes, Some(34963));
            let idx = b.accessor(v, comp, p.indices.len(), "SCALAR", None);
            jprims.push(
                json!({"attributes": attrs, "indices": idx, "material": p.material, "mode": 4}),
            );
        }
        meshes.push(json!({"name": a.nodes[*node].name, "primitives": jprims}));
        nodes[*node]["mesh"] = json!(meshes.len() - 1);
        if let Some(s) = a.primitives[prims[0]].skin {
            nodes[*node]["skin"] = json!(s);
        }
    }

    // Animations
    let mut animations = Vec::new();
    for an in &a.animations {
        let mut samplers_j = Vec::new();
        let mut channels_j = Vec::new();
        for ch in &an.channels {
            let t = b.floats(&ch.times, "SCALAR", 1, None, true);
            let kind = if ch.target == Target::Rotation {
                "VEC4"
            } else {
                "VEC3"
            };
            let o = b.floats(&ch.values, kind, ch.target.components(), None, false);
            samplers_j.push(json!({
                "input": t,
                "output": o,
                "interpolation": match ch.interpolation {
                    Interpolation::Step => "STEP",
                    Interpolation::Linear => "LINEAR",
                    Interpolation::CubicSpline => "CUBICSPLINE",
                },
            }));
            channels_j.push(json!({
                "sampler": samplers_j.len() - 1,
                "target": {"node": ch.node, "path": match ch.target {
                    Target::Translation => "translation",
                    Target::Rotation => "rotation",
                    Target::Scale => "scale",
                }},
            }));
        }
        if !channels_j.is_empty() {
            animations
                .push(json!({"name": an.name, "samplers": samplers_j, "channels": channels_j}));
        }
    }

    doc["scene"] = json!(0);
    doc["scenes"] = json!([{"name": a.name, "nodes": roots}]);
    doc["nodes"] = json!(nodes);
    if !meshes.is_empty() {
        doc["meshes"] = json!(meshes);
    }
    if !skins.is_empty() {
        doc["skins"] = json!(skins);
    }
    if !materials.is_empty() {
        doc["materials"] = json!(materials);
    }
    if !textures.is_empty() {
        doc["textures"] = json!(textures);
        doc["samplers"] = json!(samplers);
    }
    if !images.is_empty() {
        doc["images"] = json!(images);
    }
    if !animations.is_empty() {
        doc["animations"] = json!(animations);
    }
    if uses_transform {
        doc["extensionsUsed"] = json!(["KHR_texture_transform"]);
    }
    doc["accessors"] = json!(b.accessors);
    doc["bufferViews"] = json!(b.views);
    doc["buffers"] = json!([{"byteLength": b.bin.len()}]);

    let mut json_bytes = serde_json::to_vec(&doc)?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    while !b.bin.len().is_multiple_of(4) {
        b.bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + b.bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(b.bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&b.bin);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Material, Node, Primitive};

    #[test]
    fn writes_a_parsable_glb() {
        let a = Asset {
            name: "t".into(),
            nodes: vec![Node {
                name: "n".into(),
                parent: None,
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            }],
            primitives: vec![Primitive {
                name: "p".into(),
                node: 0,
                material: 0,
                positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                indices: vec![0, 1, 2],
                ..Default::default()
            }],
            materials: vec![Material {
                name: "m".into(),
                shader: "s".into(),
                keywords: vec![],
                base_color: None,
                base_color_factor: [1.0; 4],
                metallic: 0.0,
                roughness: 1.0,
                alpha: AlphaMode::Opaque,
                vertex_colors: false,
            }],
            ..Default::default()
        };
        let glb = to_glb(&a).unwrap();
        assert_eq!(&glb[..4], b"glTF");
        let total = u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize;
        assert_eq!(total, glb.len());
        let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json: Value = serde_json::from_slice(&glb[20..20 + jlen]).unwrap();
        assert_eq!(
            json["meshes"][0]["primitives"][0]["attributes"]["POSITION"],
            0
        );
        assert_eq!(json["accessors"][0]["max"], json!([1.0, 1.0, 0.0]));
    }
}
