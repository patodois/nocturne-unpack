//! [`rewrite_bridge::Provider`] for Shin Megami Tensei III Nocturne HD Remaster: lists the demon
//! bundles of an install and loads them as bridge [`Model`]s and [`AnimatedModel`]s.
//!
//! The geometry, nodes and clips come from [`crate::prefab::load`], already in the bridge's
//! conventions: x is negated to turn Unity's left-handed space into the bridge's right-handed
//! one, so demons face +z; UVs run down the image; units are Unity's, metres. Every sub-mesh
//! of a renderer is one bridge mesh. Clips become [`Animation`]s on the model's nodes.
//!
//! The bridge has no place for a few things the glTF export keeps: a texture's sampler and tiling
//! (`KHR_texture_transform`), vertex colours, and the second UV set.

use std::sync::Mutex;

use anyhow::Result;
use rewrite_bridge::{
    AlphaMode, AnimatedModel, Animation, AssetInfo, AssetKind, BoxError, Channel, Image, Instance,
    Interpolation, Material, Mesh, Model, Node, NotSupported, Provider, Scene, SceneModel, Skin,
    Target, Transform,
};

use crate::asset::{self, Asset};
use crate::bundle::Bundle;
use crate::install::Install;
use crate::prefab::{self, Options};
use crate::rig::Rig;

pub struct Nocturne {
    install: Install,
    textures: bool,
    /// Demon bundles that hold a model, as `(id, prefab name)`, read once
    index: Mutex<Option<Vec<(String, String)>>>,
    /// Field prefabs as `(id, name)`: the id is `bundle/prefab`
    field_index: Mutex<Option<Vec<(String, String)>>>,
}

impl Nocturne {
    /// Opens a game folder (the one that holds `smt3hd_Data`); a leading `~/` is expanded.
    ///
    /// ```no_run
    /// use rewrite_bridge::Provider;
    ///
    /// # fn main() -> Result<(), rewrite_bridge::BoxError> {
    /// let nocturne = nocturne_unpack::bridge::Nocturne::open("~/Games/smt3hd")?;
    /// let demon = nocturne.load_animated("dvl0x01")?;
    /// println!("{} nodes", demon.model.nodes.len());
    /// # Ok(())
    /// # }
    /// ```
    pub fn open(game: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Nocturne {
            install: Install::open(game.as_ref())?,
            textures: true,
            index: Mutex::new(None),
            field_index: Mutex::new(None),
        })
    }

    /// With `false`, materials keep their names but get no textures, which makes loading much
    /// faster (for checks).
    pub fn with_textures(mut self, textures: bool) -> Self {
        self.textures = textures;
        self
    }

    pub fn install(&self) -> &Install {
        &self.install
    }

    /// Ids (bundle names) and prefab names of the demon bundles that have a model.
    pub fn demons(&self) -> Vec<(String, String)> {
        let mut guard = self.index.lock().unwrap();
        guard
            .get_or_insert_with(|| {
                let mut v = Vec::new();
                for p in self.install.demon_bundles() {
                    let Some(id) = p.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    let Ok(b) = Bundle::open(&p) else { continue };
                    if let Ok(roots) = prefab::prefab_roots(&b) {
                        if let Some((path, _)) = prefab::main_prefab(&b, &roots)
                            .ok()
                            .and_then(|i| roots.get(i))
                        {
                            let name = path
                                .rsplit('/')
                                .next()
                                .unwrap_or(path)
                                .trim_end_matches(".prefab")
                                .to_string();
                            v.push((id.to_string(), name));
                        }
                    }
                }
                v
            })
            .clone()
    }

    /// Ids and names of the field prefabs (`f012/f012_002`): a field bundle holds one prefab
    /// per block of the field, each in its own coordinate frame.
    pub fn fields(&self) -> Vec<(String, String)> {
        let mut guard = self.field_index.lock().unwrap();
        guard
            .get_or_insert_with(|| {
                let mut v = Vec::new();
                for p in self.install.field_bundles() {
                    let Some(bundle_name) = p.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    let Ok(b) = Bundle::open(&p) else { continue };
                    for (path, go) in prefab::prefab_roots(&b).unwrap_or_default() {
                        // A prefab with nothing to draw is not a scene worth listing
                        let has_geometry = Rig::from_game_object(&b, go)
                            .is_ok_and(|rig| prefab::has_geometry(&b, &rig));
                        if !has_geometry {
                            continue;
                        }
                        let stem = path.rsplit('/').next().unwrap_or(&path);
                        let stem = stem.trim_end_matches(".prefab");
                        v.push((format!("{bundle_name}/{stem}"), stem.to_string()));
                    }
                }
                v
            })
            .clone()
    }

    fn load_asset(&self, id: &str) -> Result<Asset> {
        let (bundle_name, which) = match id.split_once('/') {
            Some((b, stem)) => (b, Some(format!("/{stem}.prefab"))),
            None => (id, None),
        };
        let bundle = Bundle::open(&self.install.resolve(bundle_name)?)?;
        let which = match &which {
            Some(w) => prefab::Which::Named(w),
            None => prefab::Which::Main,
        };
        prefab::load(
            &self.install,
            &bundle,
            which,
            &Options {
                textures: self.textures,
                animations: true,
            },
        )
    }
}

fn to_image(img: &asset::Image) -> Image {
    Image {
        width: img.width,
        height: img.height,
        rgba: img.rgba.clone(),
    }
}

fn material(a: &Asset, m: &asset::Material) -> Material {
    Material {
        name: m.name.clone(),
        base_color: m.base_color.as_ref().map(|t| to_image(&a.images[t.image])),
        base_color_factor: m.base_color_factor,
        metallic: m.metallic,
        roughness: m.roughness,
        alpha: match m.alpha {
            asset::AlphaMode::Opaque => AlphaMode::Opaque,
            asset::AlphaMode::Mask(c) => AlphaMode::Mask(c),
            asset::AlphaMode::Blend => AlphaMode::Blend,
        },
        ..Material::default()
    }
}

/// Converts a decoded asset to a bridge model with its clips. The asset is consumed so that
/// vertex data is moved, not copied.
pub fn convert(a: Asset) -> AnimatedModel {
    let materials: Vec<Material> = a.materials.iter().map(|m| material(&a, m)).collect();
    let Asset {
        name,
        nodes,
        primitives,
        skins,
        animations,
        ..
    } = a;
    let nodes: Vec<Node> = nodes
        .into_iter()
        .map(|n| Node {
            name: n.name,
            parent: n.parent,
            translation: n.translation,
            rotation: n.rotation,
            scale: n.scale,
        })
        .collect();
    let skins: Vec<Skin> = skins
        .into_iter()
        .map(|s| Skin {
            joints: s.joints,
            inverse_bind: s.inverse_bind,
        })
        .collect();
    let meshes: Vec<Mesh> = primitives
        .into_iter()
        .map(|p| Mesh {
            name: p.name,
            node: p.node,
            material: p.material,
            positions: p.positions,
            normals: p.normals,
            tangents: p.tangents,
            uvs: p.uvs,
            joints: p.joints,
            weights: p.weights,
            skin: p.skin,
            indices: p.indices,
        })
        .collect();
    let mut m = AnimatedModel::new(Model {
        name,
        nodes,
        meshes,
        materials,
        skins,
    });
    m.animations = animations
        .into_iter()
        .map(|an| {
            let last = an
                .channels
                .iter()
                .filter_map(|c| c.times.last().copied())
                .fold(0.0f32, f32::max);
            Animation {
                name: an.name,
                duration_s: an.duration.max(last),
                channels: an
                    .channels
                    .into_iter()
                    .map(|c| {
                        Channel::new(
                            c.node,
                            match c.target {
                                asset::Target::Translation => Target::Translation,
                                asset::Target::Rotation => Target::Rotation,
                                asset::Target::Scale => Target::Scale,
                            },
                            match c.interpolation {
                                asset::Interpolation::Step => Interpolation::Step,
                                asset::Interpolation::Linear => Interpolation::Linear,
                                asset::Interpolation::CubicSpline => Interpolation::CubicSpline,
                            },
                            c.times,
                            c.values,
                        )
                    })
                    .collect(),
            }
        })
        .collect();
    m
}

/// What a scene conversion could not carry over.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SceneNotes {
    /// Nodes whose world matrix has shear
    pub sheared: usize,
    /// Nodes with a zero scale
    pub collapsed: usize,
}

/// Converts a field prefab to a bridge scene: one model per distinct mesh (with its materials),
/// placed by an instance with the node's world transform.
pub fn scene(mut a: Asset) -> (Scene, SceneNotes) {
    let world = crate::transform::world_transforms(&a.nodes);
    let mut by_node: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for (i, p) in a.primitives.iter().enumerate() {
        by_node.entry(p.node).or_default().push(i);
    }
    let mut notes = SceneNotes::default();
    let mut scene = Scene {
        name: a.name.clone(),
        ..Scene::default()
    };
    let mut models: std::collections::HashMap<Vec<(i64, usize)>, usize> = Default::default();
    for (node, prims) in by_node {
        let w = world[node];
        if w.scale.contains(&0.0) {
            notes.collapsed += 1;
            continue;
        }
        if w.sheared {
            notes.sheared += 1;
        }
        let key: Vec<(i64, usize)> = prims
            .iter()
            .map(|&i| (a.primitives[i].mesh_id, a.primitives[i].material))
            .collect();
        let model = *models.entry(key).or_insert_with(|| {
            // Materials this mesh uses, renumbered
            let mut used: Vec<usize> = Vec::new();
            let mut meshes = Vec::new();
            for &i in &prims {
                let p = &mut a.primitives[i];
                let m = used
                    .iter()
                    .position(|&u| u == p.material)
                    .unwrap_or_else(|| {
                        used.push(p.material);
                        used.len() - 1
                    });
                // Moved out: a mesh that comes up again shares the model built here
                meshes.push(Mesh {
                    name: std::mem::take(&mut p.name),
                    node: 0,
                    material: m,
                    positions: std::mem::take(&mut p.positions),
                    normals: std::mem::take(&mut p.normals),
                    tangents: std::mem::take(&mut p.tangents),
                    uvs: std::mem::take(&mut p.uvs),
                    joints: vec![],
                    weights: vec![],
                    skin: None,
                    indices: std::mem::take(&mut p.indices),
                });
            }
            let materials = used
                .iter()
                .map(|&u| material(&a, &a.materials[u]))
                .collect();
            scene.models.push(SceneModel::Inline(Model {
                name: a.nodes[node].name.clone(),
                nodes: vec![Node::new(a.nodes[node].name.clone(), None)],
                meshes,
                materials,
                skins: vec![],
            }));
            scene.models.len() - 1
        });
        scene.instances.push(Instance::new(
            model,
            Transform {
                translation: w.translation,
                rotation: w.rotation,
                scale: w.scale,
            },
        ));
    }
    (scene, notes)
}

/// An error with its whole chain of causes in the message (a boxed `anyhow` error shows only the
/// outermost one).
fn boxed(e: anyhow::Error) -> BoxError {
    format!("{e:#}").into()
}

impl Provider for Nocturne {
    fn game(&self) -> &str {
        "Shin Megami Tensei III Nocturne HD Remaster"
    }

    fn list(&self, filter: &str) -> Result<Vec<AssetInfo>, BoxError> {
        let demons = self
            .demons()
            .into_iter()
            .map(|(id, name)| (id, name, AssetKind::Character));
        let fields = self
            .fields()
            .into_iter()
            .map(|(id, name)| (id, name, AssetKind::Level));
        Ok(demons
            .chain(fields)
            .filter(|(id, name, _)| id.contains(filter) || name.contains(filter))
            .map(|(id, name, kind)| AssetInfo { id, name, kind })
            .collect())
    }

    fn load(&self, id: &str) -> Result<Model, BoxError> {
        Ok(self.load_animated(id)?.model)
    }

    fn load_animated(&self, id: &str) -> Result<AnimatedModel, BoxError> {
        let m = convert(self.load_asset(id).map_err(boxed)?);
        m.validate()?;
        Ok(m)
    }

    /// A field prefab (`f012/f012_002`) as a scene.
    fn load_scene(&self, id: &str) -> Result<Scene, BoxError> {
        if !id.contains('/') {
            return Err(Box::new(NotSupported {
                game: self.game().to_string(),
                what: "scenes of demons".to_string(),
            }));
        }
        let (scene, _) = scene(self.load_asset(id).map_err(boxed)?);
        scene.validate()?;
        Ok(scene)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Interpolation as AI, Node as AN, Primitive, Skin as AS, Target as AT};

    fn asset() -> Asset {
        let node = |name: &str, parent| AN {
            name: name.into(),
            parent,
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        Asset {
            name: "t".into(),
            nodes: vec![node("root", None), node("bone", Some(0))],
            primitives: vec![Primitive {
                name: "p".into(),
                node: 0,
                skin: Some(0),
                material: 0,
                positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                joints: vec![[1, 0, 0, 0]; 3],
                weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
                indices: vec![0, 1, 2],
                ..Default::default()
            }],
            skins: vec![AS {
                joints: vec![0, 1],
                inverse_bind: vec![[0.0; 16]; 2],
                skeleton: Some(0),
            }],
            materials: vec![asset::Material {
                name: "m".into(),
                shader: "s".into(),
                keywords: vec![],
                base_color: None,
                base_color_factor: [1.0; 4],
                metallic: 0.0,
                roughness: 1.0,
                alpha: asset::AlphaMode::Mask(0.5),
                vertex_colors: false,
            }],
            animations: vec![asset::Animation {
                name: "a".into(),
                duration: 1.0,
                channels: vec![asset::Channel {
                    node: 1,
                    target: AT::Translation,
                    interpolation: AI::Linear,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                }],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn converts_to_a_valid_animated_model() {
        let m = convert(asset());
        assert_eq!(m.validate(), Ok(()));
        assert_eq!(m.model.meshes[0].skin, Some(0));
        assert_eq!(m.model.materials[0].alpha, AlphaMode::Mask(0.5));
        assert_eq!(m.animations[0].channels[0].node, 1);
        assert_eq!(m.animations[0].duration_s, 1.0);
    }
}
