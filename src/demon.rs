//! Demons: the `dvl0x..` bundles. Each holds the prefab of one demon (a GameObject hierarchy
//! with rigid meshes on bones and skinned ones), its meshes, materials, textures and the
//! animation clips of its AnimatorController.
//!
//! [`load`] reads one bundle into an [`Asset`]: nodes in glTF's space, one primitive per
//! sub-mesh of every enabled renderer, materials with their `_MainTex`, skins, and the clips of
//! the demon's controller as animations on the rig. What cannot be carried over is counted in
//! the asset's [`Report`].

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use anyhow::{anyhow, ensure, Context, Result};

use crate::asset::{
    self, space, AlphaMode, Animation, Asset, Image, Interpolation, Node, Primitive, Sampler, Skin,
    Target, TextureRef,
};
use crate::bundle::{class, Bundle};
use crate::clip::{self, Interp, Prop};
use crate::install::Install;
use crate::mesh::{self, MeshData};
use crate::rig::Rig;
use crate::unity::{
    AnimationClip, Animator, AnimatorController, AssetBundle, Material as UMaterial, Mesh,
    MeshFilter, Renderer, Texture2D,
};

/// What [`load`] was asked to do.
#[derive(Clone, Debug)]
pub struct Options {
    /// Decode textures (off for fast structural checks)
    pub textures: bool,
    /// Convert animation clips
    pub animations: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            textures: true,
            animations: true,
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The prefab roots listed in the bundle's asset table (`.prefab` entries), as GameObject ids.
pub fn prefab_roots(bundle: &Bundle) -> Result<Vec<(String, i64)>> {
    let ab_id = *bundle
        .objects_of(class::ASSET_BUNDLE)
        .first()
        .ok_or_else(|| anyhow!("no AssetBundle object"))?;
    let ab: AssetBundle = bundle.read(ab_id)?;
    Ok(ab
        .m_Container
        .iter()
        .filter(|(k, _)| k.ends_with(".prefab"))
        .filter_map(|(k, v)| {
            v.asset.local().and_then(|id| {
                (bundle.class_of(id) == Some(class::GAME_OBJECT)).then(|| (k.clone(), id))
            })
        })
        .collect())
}

/// The animation clips of the controller on the node that carries the Animator, in order.
pub fn controller_clips(bundle: &Bundle, rig: &Rig, anim_root: usize) -> Result<Vec<i64>> {
    let mut clip_ids: Vec<i64> = Vec::new();
    if let Some(c) = rig.nodes[anim_root]
        .components
        .iter()
        .find(|c| c.class == class::ANIMATOR)
    {
        let a: Animator = bundle.read(c.id)?;
        if let Some(cid) = a.m_Controller.local() {
            let ctl: AnimatorController = bundle.read(cid)?;
            for p in &ctl.m_AnimationClips {
                if let Some(id) = p.local() {
                    if !clip_ids.contains(&id) {
                        clip_ids.push(id);
                    }
                }
            }
        }
    }
    Ok(clip_ids)
}

struct Loader<'a> {
    install: &'a Install,
    bundle: &'a Bundle,
    opts: &'a Options,
    asset: Asset,
    /// Texture object id to index into `asset.images`
    images: HashMap<i64, usize>,
    /// Material object id to index into `asset.materials`
    materials: HashMap<i64, usize>,
    meshes: HashMap<i64, Rc<MeshData>>,
}

/// Loads the demon prefab of a bundle. With several prefabs, `prefab` picks one by (part of)
/// its asset path; without it the first one is used.
pub fn load(
    install: &Install,
    bundle: &Bundle,
    prefab: Option<&str>,
    opts: &Options,
) -> Result<Asset> {
    let roots = prefab_roots(bundle)?;
    let (path, go) = match prefab {
        Some(p) => roots
            .iter()
            .find(|(k, _)| k.contains(p))
            .ok_or_else(|| anyhow!("no prefab matching {p:?}"))?,
        None => roots
            .first()
            .ok_or_else(|| anyhow!("the bundle has no prefab"))?,
    };
    let rig = Rig::from_game_object(bundle, *go)?;
    let name = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".prefab")
        .to_string();
    let mut l = Loader {
        install,
        bundle,
        opts,
        asset: Asset {
            name,
            ..Default::default()
        },
        images: HashMap::new(),
        materials: HashMap::new(),
        meshes: HashMap::new(),
    };
    l.nodes(&rig);
    l.renderers(&rig)?;
    if opts.animations {
        l.animations(&rig)?;
    }
    Ok(l.asset)
}

impl Loader<'_> {
    fn nodes(&mut self, rig: &Rig) {
        for n in &rig.nodes {
            self.asset.nodes.push(Node {
                name: n.name.clone(),
                parent: n.parent,
                translation: space::position(n.translation),
                rotation: space::rotation(n.rotation),
                scale: n.scale,
            });
        }
    }

    fn renderers(&mut self, rig: &Rig) -> Result<()> {
        for (ni, node) in rig.nodes.iter().enumerate() {
            let filter = node
                .components
                .iter()
                .find(|c| c.class == class::MESH_FILTER);
            let rend = node.components.iter().find(|c| {
                c.class == class::MESH_RENDERER || c.class == class::SKINNED_MESH_RENDERER
            });
            let Some(rend) = rend else { continue };
            let r: Renderer = self.bundle.read(rend.id)?;
            let skinned = rend.class == class::SKINNED_MESH_RENDERER;
            let mesh_ptr = if skinned {
                r.m_Mesh
            } else if let Some(f) = filter {
                self.bundle.read::<MeshFilter>(f.id)?.m_Mesh
            } else {
                self.asset.report.add("MeshRenderer without MeshFilter");
                continue;
            };
            if !r.m_Enabled || !rig.active_in_hierarchy(ni) {
                self.asset
                    .report
                    .add("renderer on an inactive object (skipped)");
                continue;
            }
            let Some(mesh_id) = mesh_ptr.local() else {
                self.asset.report.add("renderer without a mesh");
                continue;
            };
            self.primitives(rig, ni, mesh_id, &r, skinned)
                .with_context(|| format!("mesh of node {:?}", node.name))?;
        }
        Ok(())
    }

    fn mesh(&mut self, id: i64) -> Result<Rc<MeshData>> {
        if !self.meshes.contains_key(&id) {
            let m: Mesh = self.bundle.read(id)?;
            let d = mesh::decode(self.bundle, &m)?;
            self.meshes.insert(id, Rc::new(d));
        }
        Ok(self.meshes[&id].clone())
    }

    fn primitives(
        &mut self,
        rig: &Rig,
        ni: usize,
        mesh_id: i64,
        r: &Renderer,
        skinned: bool,
    ) -> Result<()> {
        // Materials first (they decide whether vertex colours are kept)
        let mut mats = Vec::new();
        for m in &r.m_Materials {
            mats.push(match m.local() {
                Some(id) => Some(self.material(id)?),
                None => None,
            });
        }
        let data = self.mesh(mesh_id)?;
        let skin_index = if skinned {
            Some(self.skin(rig, r, &data)?)
        } else {
            None
        };
        let nsub = data.submeshes.len();
        if nsub > mats.len().max(1) {
            self.asset
                .report
                .add("sub-meshes beyond the renderer's material list (use the last material)");
        }
        let mut prims = Vec::new();
        for (si, tris) in data.submeshes.iter().enumerate() {
            if tris.is_empty() {
                continue;
            }
            let mat_slot = mats.get(si).or(mats.last()).copied().flatten();
            let material = match mat_slot {
                Some(m) => m,
                None => self.default_material(),
            };
            let keep_colors = self.asset.materials[material].vertex_colors;
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let mut used: Vec<usize> = Vec::new();
            let mut idx = Vec::with_capacity(tris.len());
            for t in tris.as_chunks::<3>().0 {
                // Reversed winding: the mirror turns Unity's clockwise front faces
                // into counter-clockwise ones
                for &v in &[t[0], t[2], t[1]] {
                    let n = *remap.entry(v).or_insert_with(|| {
                        used.push(v as usize);
                        (used.len() - 1) as u32
                    });
                    idx.push(n);
                }
            }
            let pick = |v: &Vec<[f32; 3]>| -> Vec<[f32; 3]> {
                used.iter().filter_map(|&i| v.get(i)).copied().collect()
            };
            let mut p = Primitive {
                name: format!("{}_{si}", data.name),
                node: ni,
                skin: skin_index,
                material,
                positions: pick(&data.positions)
                    .into_iter()
                    .map(space::position)
                    .collect(),
                indices: idx,
                ..Default::default()
            };
            if !data.normals.is_empty() {
                p.normals = pick(&data.normals)
                    .into_iter()
                    .map(space::position)
                    .collect();
            }
            if !data.tangents.is_empty() {
                p.tangents = used
                    .iter()
                    .map(|&i| space::tangent(data.tangents[i]))
                    .collect();
            }
            if !data.uv0.is_empty() {
                // Unity's v runs up the texture, glTF's runs down
                p.uvs = used
                    .iter()
                    .map(|&i| [data.uv0[i][0], 1.0 - data.uv0[i][1]])
                    .collect();
            }
            if !data.uv1.is_empty() {
                self.asset.report.add("second UV set");
            }
            if !data.colors.is_empty() {
                if keep_colors {
                    p.colors = used.iter().map(|&i| data.colors[i]).collect();
                } else {
                    self.asset
                        .report
                        .add("vertex colours the material does not read");
                }
            }
            if skinned && !data.joints.is_empty() {
                p.joints = used.iter().map(|&i| data.joints[i]).collect();
                p.weights = used.iter().map(|&i| data.weights[i]).collect();
            }
            ensure!(
                p.positions.len() == used.len(),
                "mesh {:?} indexes vertices that do not exist",
                data.name
            );
            prims.push(p);
        }
        self.asset.primitives.extend(prims);
        Ok(())
    }

    fn skin(&mut self, rig: &Rig, r: &Renderer, data: &MeshData) -> Result<usize> {
        ensure!(
            !data.joints.is_empty(),
            "skinned mesh {:?} has no bone indices",
            data.name
        );
        ensure!(
            data.bind_poses.len() == r.m_Bones.len(),
            "mesh {:?} has {} bind poses for {} bones",
            data.name,
            data.bind_poses.len(),
            r.m_Bones.len()
        );
        let mut joints = Vec::new();
        for b in &r.m_Bones {
            let node = b
                .local()
                .and_then(|id| rig.node_of_transform(id))
                .ok_or_else(|| anyhow!("a bone of mesh {:?} is not in the prefab", data.name))?;
            joints.push(node);
        }
        let max = data.joints.iter().flatten().copied().max().unwrap_or(0) as usize;
        ensure!(
            max < joints.len(),
            "mesh {:?} weights bone {max} of {}",
            data.name,
            joints.len()
        );
        let skeleton = r
            .m_RootBone
            .local()
            .and_then(|id| rig.node_of_transform(id));
        self.asset.skins.push(Skin {
            joints,
            inverse_bind: data
                .bind_poses
                .iter()
                .map(space::matrix_to_column_major)
                .collect(),
            skeleton,
        });
        Ok(self.asset.skins.len() - 1)
    }

    fn default_material(&mut self) -> usize {
        if let Some(&i) = self.materials.get(&0) {
            return i;
        }
        self.asset.report.add("sub-mesh without a material");
        self.asset.materials.push(asset::Material {
            name: "(no material)".into(),
            shader: "(none)".into(),
            keywords: vec![],
            base_color: None,
            base_color_factor: [1.0; 4],
            metallic: 0.0,
            roughness: 1.0,
            alpha: AlphaMode::Opaque,
            vertex_colors: false,
        });
        let i = self.asset.materials.len() - 1;
        self.materials.insert(0, i);
        i
    }

    fn image(&mut self, tex_id: i64) -> Result<(usize, Sampler)> {
        let t: Texture2D = self.bundle.read(tex_id)?;
        let sampler = Sampler {
            mag_filter: if t.m_TextureSettings.m_FilterMode == 0 {
                9728
            } else {
                9729
            },
            min_filter: match t.m_TextureSettings.m_FilterMode {
                0 => 9728,
                1 => 9729,
                _ => 9987,
            },
            wrap_s: wrap(t.m_TextureSettings.m_WrapU),
            wrap_t: wrap(t.m_TextureSettings.m_WrapV),
        };
        if let Some(&i) = self.images.get(&tex_id) {
            return Ok((i, sampler));
        }
        let img: Image = crate::texture::decode(self.bundle, &t)?;
        self.asset.images.push(img);
        let i = self.asset.images.len() - 1;
        self.images.insert(tex_id, i);
        Ok((i, sampler))
    }

    fn material(&mut self, id: i64) -> Result<usize> {
        if let Some(&i) = self.materials.get(&id) {
            return Ok(i);
        }
        let m: UMaterial = self.bundle.read(id)?;
        let shader = self.install.shader_name(self.bundle, &m.m_Shader);
        let keywords: Vec<String> = m
            .m_ShaderKeywords
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let standard = shader == "Standard";
        let float = |k: &str| {
            m.m_SavedProperties
                .m_Floats
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| *v)
        };
        let color = |k: &str| {
            m.m_SavedProperties
                .m_Colors
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, c)| *c)
        };
        let has = |k: &str| keywords.iter().any(|w| w == k);

        // Alpha mode from Unity's own keywords
        let alpha = if has("_ALPHATEST_ON") {
            AlphaMode::Mask(float("_Cutoff").unwrap_or(0.5))
        } else if has("_ALPHABLEND_ON") || has("_ALPHAPREMULTIPLY_ON") {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        };

        // _MainTex
        let mut base_color = None;
        if self.opts.textures {
            if let Some((_, te)) = m
                .m_SavedProperties
                .m_TexEnvs
                .iter()
                .find(|(k, _)| k == "_MainTex")
            {
                match te.m_Texture.local() {
                    Some(tid) => {
                        let (image, sampler) = self.image(tid)?;
                        let (s, o) = ([te.m_Scale.x, te.m_Scale.y], [te.m_Offset.x, te.m_Offset.y]);
                        let transform = (s != [1.0, 1.0] || o != [0.0, 0.0])
                            // Unity's offset is measured up from the bottom, glTF's down from the top
                            .then(|| ([o[0], 1.0 - s[1] - o[1]], s));
                        base_color = Some(TextureRef {
                            image,
                            sampler,
                            transform,
                        });
                    }
                    None if te.m_Texture.m_PathID != 0 => {
                        self.asset.report.add("_MainTex in another bundle");
                    }
                    None => {}
                }
            }
        }

        // Everything else the material sets
        for (k, te) in &m.m_SavedProperties.m_TexEnvs {
            if k != "_MainTex" && te.m_Texture.m_PathID != 0 {
                self.asset
                    .report
                    .add(format!("{shader}: texture {k} not mapped"));
            }
        }
        let (mut factor, mut metallic, mut roughness) = ([1.0f32; 4], 0.0, 1.0);
        if standard {
            // Direct: the colour (stored gamma-encoded, glTF wants linear RGB), metallic and smoothness
            if let Some(c) = color("_Color") {
                factor = [
                    srgb_to_linear(c.r),
                    srgb_to_linear(c.g),
                    srgb_to_linear(c.b),
                    c.a,
                ];
            }
            metallic = float("_Metallic").unwrap_or(0.0);
            roughness = 1.0 - float("_Glossiness").unwrap_or(0.5);
        } else {
            for (k, _) in &m.m_SavedProperties.m_Colors {
                self.asset
                    .report
                    .add(format!("{shader}: colour {k} not mapped"));
            }
            for (k, _) in &m.m_SavedProperties.m_Floats {
                self.asset
                    .report
                    .add(format!("{shader}: value {k} not mapped"));
            }
        }
        let vertex_colors = has("ENABLE_VERTEXCOLOR");
        self.asset.materials.push(asset::Material {
            name: m.m_Name,
            shader,
            keywords,
            base_color,
            base_color_factor: factor,
            metallic,
            roughness,
            alpha,
            vertex_colors,
        });
        let i = self.asset.materials.len() - 1;
        self.materials.insert(id, i);
        Ok(i)
    }

    fn animations(&mut self, rig: &Rig) -> Result<()> {
        let anim_root = rig.first_with_class(class::ANIMATOR).unwrap_or(0);
        let clip_ids = controller_clips(self.bundle, rig, anim_root)?;
        let paths = rig.paths_below(anim_root);
        let mut by_hash: HashMap<u32, usize> = HashMap::new();
        for (i, p) in paths.iter().enumerate() {
            if let Some(p) = p {
                by_hash.entry(crate::rig::crc32(p.as_bytes())).or_insert(i);
            }
        }
        for id in clip_ids {
            let c: AnimationClip = self.bundle.read(id)?;
            let anim = self
                .convert_clip(&c, &by_hash)
                .with_context(|| format!("clip {:?}", c.m_Name))?;
            self.asset.animations.push(anim);
        }
        Ok(())
    }

    fn convert_clip(
        &mut self,
        c: &AnimationClip,
        by_hash: &HashMap<u32, usize>,
    ) -> Result<Animation> {
        ensure!(!c.m_Legacy, "legacy animation clip");
        let curves = clip::read_curves(c)?;
        let slots = clip::slots(&c.m_ClipBindingConstant.genericBindings);
        let total: usize = slots.iter().map(|s| s.curve_count).sum();
        ensure!(
            total == curves.curves.len(),
            "bindings own {total} curves but the clip has {}",
            curves.curves.len()
        );
        for (what, n) in clip::unread_bindings(&slots) {
            self.asset
                .report
                .add_n(format!("animation binding: {what}"), n);
        }
        let start = curves.start;
        let duration = (curves.stop - start).max(0.0);
        let mut channels: Vec<asset::Channel> = Vec::new();
        let mut taken = std::collections::HashSet::new();
        for s in &slots {
            let Some(prop) = s.prop else { continue };
            let Some(&node) = by_hash.get(&s.binding.path) else {
                self.asset
                    .report
                    .add("animation binding for a path not in the hierarchy");
                continue;
            };
            let comps: Vec<&clip::Curve> = (0..s.curve_count)
                .map(|k| &curves.curves[s.first_curve + k])
                .collect();
            if comps.iter().any(|c| c.keys.is_empty()) {
                self.asset.report.add("animation curve without keys");
                continue;
            }
            let mut ch = if prop == Prop::Euler {
                let Some(order) = clip::euler_order(s.binding.customType) else {
                    self.asset
                        .report
                        .add("Euler curve with an unknown rotation order");
                    continue;
                };
                clip::build_euler_channel(&comps, order)?
            } else {
                clip::build_channel(prop, &comps)?
            };
            if start != 0.0 {
                for t in ch.times.iter_mut() {
                    *t -= start;
                }
            }
            self.asset.report.add_n(
                "jumps inside smooth curves, written one float step early",
                ch.steps_encoded,
            );
            if prop == Prop::Euler && ch.times.len() > 1 {
                self.asset
                    .report
                    .add("Euler rotation curves converted to quaternion keys");
                self.asset
                    .report
                    .add_n("extra keys added to Euler curves", ch.approximated);
                if ch.max_error_deg > clip::EULER_TOLERANCE_DEG {
                    self.asset
                        .report
                        .add("Euler curves still above the angle tolerance between keys");
                }
            }
            let target = match prop {
                Prop::Position => Target::Translation,
                Prop::Rotation | Prop::Euler => Target::Rotation,
                Prop::Scale => Target::Scale,
            };
            if !taken.insert((node, target)) {
                self.asset
                    .report
                    .add("second binding for a node property in one clip");
                continue;
            }
            channels.push(to_gltf_channel(node, target, ch));
        }
        Ok(Animation {
            name: c.m_Name.clone(),
            duration,
            channels,
        })
    }
}

fn wrap(mode: i32) -> u32 {
    match mode {
        1 | 3 => 33071,
        2 => 33648,
        _ => 10497,
    }
}

/// Converts a channel to glTF's space and layout.
fn to_gltf_channel(node: usize, target: Target, ch: clip::Channel) -> asset::Channel {
    let n = target.components();
    let conv = |v: &[f32]| -> Vec<f32> {
        match target {
            Target::Translation => space::position([v[0], v[1], v[2]]).to_vec(),
            Target::Rotation => space::rotation([v[0], v[1], v[2], v[3]]).to_vec(),
            Target::Scale => v.to_vec(),
        }
    };
    let mut values = Vec::new();
    let keys = ch.times.len();
    let cubic = ch.interp == Interp::Cubic;
    for k in 0..keys {
        let v = &ch.values[k * n..(k + 1) * n];
        if cubic {
            values.extend(conv(&ch.in_tangents[k * n..(k + 1) * n]));
        }
        let mut val = conv(v);
        if target == Target::Rotation {
            let len = val.iter().map(|x| x * x).sum::<f32>().sqrt();
            if len > 0.0 {
                val.iter_mut().for_each(|x| *x /= len);
            }
        }
        values.extend(val);
        if cubic {
            values.extend(conv(&ch.out_tangents[k * n..(k + 1) * n]));
        }
    }
    asset::Channel {
        node,
        target,
        interpolation: match ch.interp {
            Interp::Step => Interpolation::Step,
            Interp::Linear => Interpolation::Linear,
            Interp::Cubic => Interpolation::CubicSpline,
        },
        times: ch.times,
        values,
    }
}

/// A short summary of an asset for `info`-style output.
pub fn summary(a: &Asset) -> BTreeMap<&'static str, usize> {
    let mut m = BTreeMap::new();
    m.insert("nodes", a.nodes.len());
    m.insert("primitives", a.primitives.len());
    m.insert(
        "vertices",
        a.primitives.iter().map(|p| p.positions.len()).sum(),
    );
    m.insert(
        "triangles",
        a.primitives.iter().map(|p| p.indices.len() / 3).sum(),
    );
    m.insert("skins", a.skins.len());
    m.insert("materials", a.materials.len());
    m.insert("images", a.images.len());
    m.insert("animations", a.animations.len());
    m.insert(
        "channels",
        a.animations.iter().map(|x| x.channels.len()).sum(),
    );
    m
}
