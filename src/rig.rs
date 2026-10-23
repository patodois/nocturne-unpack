//! The transform hierarchy of one prefab: names, paths, local transforms and the components
//! hanging on each node.

use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};

use crate::bundle::{class, Bundle};
use crate::unity::{GameObject, Transform};

/// CRC-32 (the zlib polynomial) of a byte string. Unity addresses the transforms an animation
/// clip drives by the CRC-32 of their path below the animated root.
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        t
    });
    let mut crc = !0u32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// A component of a node, by class id and object id.
#[derive(Clone, Copy, Debug)]
pub struct Component {
    pub class: i32,
    pub id: i64,
}

#[derive(Clone, Debug)]
pub struct RigNode {
    pub name: String,
    /// Index of the parent in [`Rig::nodes`]; `None` for the root.
    pub parent: Option<usize>,
    /// Local transform in Unity's space (left-handed): translation, quaternion x y z w, scale.
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// Names from below the root joined with `/`; empty for the root itself.
    pub path: String,
    pub game_object: i64,
    pub transform: i64,
    pub active: bool,
    pub components: Vec<Component>,
}

/// A prefab's nodes, parents before children.
#[derive(Debug, Default)]
pub struct Rig {
    pub nodes: Vec<RigNode>,
    by_transform: HashMap<i64, usize>,
}

impl Rig {
    /// Reads the hierarchy below a GameObject.
    pub fn from_game_object(bundle: &Bundle, root_go: i64) -> Result<Rig> {
        let go: GameObject = bundle.read(root_go)?;
        let tr_id = go
            .m_Component
            .iter()
            .find_map(|c| {
                let id = c.component.local()?;
                (bundle.class_of(id) == Some(class::TRANSFORM)).then_some(id)
            })
            .ok_or_else(|| anyhow!("GameObject {root_go} has no Transform"))?;
        let mut rig = Rig::default();
        rig.add(bundle, tr_id, None, "")?;
        Ok(rig)
    }

    fn add(
        &mut self,
        bundle: &Bundle,
        tr_id: i64,
        parent: Option<usize>,
        path: &str,
    ) -> Result<()> {
        if self.nodes.len() > 100_000 {
            bail!("transform hierarchy is unreasonably large (cycle?)");
        }
        let tr: Transform = bundle.read(tr_id)?;
        let go_id = tr
            .m_GameObject
            .local()
            .ok_or_else(|| anyhow!("Transform {tr_id} has no GameObject"))?;
        let go: GameObject = bundle.read(go_id)?;
        let mut components = Vec::new();
        for c in &go.m_Component {
            if let Some(id) = c.component.local() {
                if let Some(cl) = bundle.class_of(id) {
                    components.push(Component { class: cl, id });
                }
            }
        }
        let own = if parent.is_none() {
            String::new()
        } else if path.is_empty() {
            go.m_Name.clone()
        } else {
            format!("{path}/{}", go.m_Name)
        };
        let idx = self.nodes.len();
        self.by_transform.insert(tr_id, idx);
        self.nodes.push(RigNode {
            name: go.m_Name,
            parent,
            translation: [
                tr.m_LocalPosition.x,
                tr.m_LocalPosition.y,
                tr.m_LocalPosition.z,
            ],
            rotation: [
                tr.m_LocalRotation.x,
                tr.m_LocalRotation.y,
                tr.m_LocalRotation.z,
                tr.m_LocalRotation.w,
            ],
            scale: [tr.m_LocalScale.x, tr.m_LocalScale.y, tr.m_LocalScale.z],
            path: own.clone(),
            game_object: go_id,
            transform: tr_id,
            active: go.m_IsActive,
            components,
        });
        for child in &tr.m_Children {
            if let Some(id) = child.local() {
                self.add(bundle, id, Some(idx), &own)?;
            }
        }
        Ok(())
    }

    /// Node index of a Transform object.
    pub fn node_of_transform(&self, transform_id: i64) -> Option<usize> {
        self.by_transform.get(&transform_id).copied()
    }

    /// Index of the first node (in depth-first order) that has a component of this class.
    pub fn first_with_class(&self, class_id: i32) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.components.iter().any(|c| c.class == class_id))
    }

    /// Paths of the nodes at and below `root`, relative to it (`""` for `root` itself); `None`
    /// for nodes elsewhere in the rig.
    pub fn paths_below(&self, root: usize) -> Vec<Option<String>> {
        let mut out: Vec<Option<String>> = vec![None; self.nodes.len()];
        out[root] = Some(String::new());
        for i in 0..self.nodes.len() {
            if i == root {
                continue;
            }
            let Some(p) = self.nodes[i].parent else {
                continue;
            };
            if let Some(parent_path) = &out[p] {
                let path = if parent_path.is_empty() {
                    self.nodes[i].name.clone()
                } else {
                    format!("{parent_path}/{}", self.nodes[i].name)
                };
                out[i] = Some(path);
            }
        }
        out
    }

    /// Whether a node and all its ancestors are active.
    pub fn active_in_hierarchy(&self, mut i: usize) -> bool {
        loop {
            if !self.nodes[i].active {
                return false;
            }
            match self.nodes[i].parent {
                Some(p) => i = p,
                None => return true,
            }
        }
    }

    /// Path hash to node index. Two nodes with the same path (twin siblings) collide; the first
    /// one wins, as it does in Unity.
    pub fn path_hashes(&self) -> HashMap<u32, usize> {
        let mut m = HashMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            m.entry(crc32(n.path.as_bytes())).or_insert(i);
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_values() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
