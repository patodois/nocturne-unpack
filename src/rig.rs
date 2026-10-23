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

/// The most nodes one rig may hold (the largest prefab of the game has far fewer).
const MAX_NODES: usize = 100_000;

/// The most bytes the paths of all the nodes of a rig may take together. A path holds the names
/// of every ancestor, so a chain of n nodes takes n squared bytes.
const MAX_PATH_BYTES: usize = 64 << 20;

/// What the walk of a hierarchy needs from one Transform and its GameObject.
struct NodeRead {
    name: String,
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
    game_object: i64,
    active: bool,
    components: Vec<Component>,
    /// Transform ids of the children, in order
    children: Vec<i64>,
}

fn read_node(bundle: &Bundle, tr_id: i64) -> Result<NodeRead> {
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
    Ok(NodeRead {
        name: go.m_Name,
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
        game_object: go_id,
        active: go.m_IsActive,
        components,
        children: tr.m_Children.iter().filter_map(|c| c.local()).collect(),
    })
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
        Self::from_game_objects(bundle, &[root_go])
    }

    /// Reads the hierarchies below several GameObjects into one rig; each is a root of it.
    pub fn from_game_objects(bundle: &Bundle, roots: &[i64]) -> Result<Rig> {
        let mut transforms = Vec::with_capacity(roots.len());
        for &root_go in roots {
            let go: GameObject = bundle.read(root_go)?;
            let tr_id = go
                .m_Component
                .iter()
                .find_map(|c| {
                    let id = c.component.local()?;
                    (bundle.class_of(id) == Some(class::TRANSFORM)).then_some(id)
                })
                .ok_or_else(|| anyhow!("GameObject {root_go} has no Transform"))?;
            transforms.push(tr_id);
        }
        Self::from_transforms(&transforms, |id| read_node(bundle, id))
    }

    /// Number of components of the given classes in the rig.
    pub fn count_components(&self, classes: &[i32]) -> usize {
        self.nodes
            .iter()
            .flat_map(|n| &n.components)
            .filter(|c| classes.contains(&c.class))
            .count()
    }

    /// Walks the hierarchies below the given Transforms, parents before children, reading each
    /// node with `read`. The children come from the file, so the walk does not trust them to
    /// form a tree: it keeps its own stack (a deep chain cannot overflow the thread's), refuses a
    /// Transform it has already visited (a cycle, or a child listed twice), and stops at
    /// [`MAX_NODES`] nodes and [`MAX_PATH_BYTES`] bytes of node paths, which each hold the names
    /// of all their ancestors.
    fn from_transforms(
        roots: &[i64],
        mut read: impl FnMut(i64) -> Result<NodeRead>,
    ) -> Result<Rig> {
        let mut rig = Rig::default();
        let mut path_bytes = 0usize;
        // (Transform, parent node); the last one pushed is read first
        let mut stack: Vec<(i64, Option<usize>)> = Vec::new();
        for &root in roots.iter().rev() {
            stack.push((root, None));
        }
        while let Some((tr_id, parent)) = stack.pop() {
            if rig.by_transform.contains_key(&tr_id) {
                bail!("transform {tr_id} is reached twice in the hierarchy (a cycle, or a child listed twice)");
            }
            if rig.nodes.len() >= MAX_NODES {
                bail!("transform hierarchy is unreasonably large (more than {MAX_NODES} nodes)");
            }
            let n = read(tr_id)?;
            let own = match parent {
                None => String::new(),
                Some(p) if rig.nodes[p].path.is_empty() => n.name.clone(),
                Some(p) => format!("{}/{}", rig.nodes[p].path, n.name),
            };
            path_bytes = path_bytes.saturating_add(own.len());
            if path_bytes > MAX_PATH_BYTES {
                bail!("transform hierarchy is unreasonably deep (its node paths take more than {MAX_PATH_BYTES} bytes)");
            }
            let idx = rig.nodes.len();
            rig.by_transform.insert(tr_id, idx);
            rig.nodes.push(RigNode {
                name: n.name,
                parent,
                translation: n.translation,
                rotation: n.rotation,
                scale: n.scale,
                path: own,
                game_object: n.game_object,
                transform: tr_id,
                active: n.active,
                components: n.components,
            });
            for &child in n.children.iter().rev() {
                stack.push((child, Some(idx)));
            }
        }
        Ok(rig)
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

    /// Whether `ancestor` is `node` or one of its ancestors.
    pub fn is_ancestor_or_self(&self, ancestor: usize, mut node: usize) -> bool {
        loop {
            if node == ancestor {
                return true;
            }
            match self.nodes[node].parent {
                Some(p) => node = p,
                None => return false,
            }
        }
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

    /// A hierarchy given as `(name, children)` by Transform id, without a bundle.
    fn reader<'a>(
        tree: &'a HashMap<i64, (String, Vec<i64>)>,
    ) -> impl FnMut(i64) -> Result<NodeRead> + 'a {
        move |id| {
            let (name, children) = tree.get(&id).ok_or_else(|| anyhow!("no transform {id}"))?;
            Ok(NodeRead {
                name: name.clone(),
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
                game_object: id,
                active: true,
                components: vec![],
                children: children.clone(),
            })
        }
    }

    fn chain(len: i64, name: &str) -> HashMap<i64, (String, Vec<i64>)> {
        (1..=len)
            .map(|i| {
                (
                    i,
                    (name.to_string(), if i < len { vec![i + 1] } else { vec![] }),
                )
            })
            .collect()
    }

    #[test]
    fn a_tree_comes_out_parents_first_with_paths() {
        let tree = HashMap::from([
            (1, ("root".to_string(), vec![2, 3])),
            (2, ("a".to_string(), vec![4])),
            (3, ("b".to_string(), vec![])),
            (4, ("c".to_string(), vec![])),
        ]);
        let rig = Rig::from_transforms(&[1], reader(&tree)).unwrap();
        let names: Vec<_> = rig.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["root", "a", "c", "b"]);
        let paths: Vec<_> = rig.nodes.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["", "a", "a/c", "b"]);
        assert_eq!(rig.nodes[2].parent, Some(1));
        assert_eq!(rig.node_of_transform(4), Some(2));
    }

    #[test]
    fn a_child_cycle_is_an_error_not_a_stack_overflow() {
        // 1 -> 2 -> 3 -> 1
        let tree = HashMap::from([
            (1, ("a".to_string(), vec![2])),
            (2, ("b".to_string(), vec![3])),
            (3, ("c".to_string(), vec![1])),
        ]);
        let e = Rig::from_transforms(&[1], reader(&tree))
            .unwrap_err()
            .to_string();
        assert!(e.contains("reached twice"), "{e}");
    }

    #[test]
    fn a_child_listed_twice_is_an_error() {
        let tree = HashMap::from([
            (1, ("a".to_string(), vec![2, 2])),
            (2, ("b".to_string(), vec![])),
        ]);
        assert!(Rig::from_transforms(&[1], reader(&tree)).is_err());
        // The same root twice as well
        assert!(Rig::from_transforms(&[2, 2], reader(&tree)).is_err());
    }

    #[test]
    fn a_deep_chain_does_not_use_the_call_stack() {
        // Far deeper than any prefab
        let tree = chain(4_000, "n");
        let rig = Rig::from_transforms(&[1], reader(&tree)).unwrap();
        assert_eq!(rig.nodes.len(), 4_000);
        assert_eq!(rig.nodes[3_999].parent, Some(3_998));
    }

    #[test]
    fn long_names_down_a_chain_hit_the_path_budget() {
        // Paths hold every ancestor's name, so this would take about 100 * 20000^2 / 2 bytes
        let tree = chain(20_000, &"x".repeat(100));
        let e = Rig::from_transforms(&[1], reader(&tree))
            .unwrap_err()
            .to_string();
        assert!(e.contains("node paths"), "{e}");
    }

    #[test]
    fn too_many_nodes_is_an_error() {
        let tree: HashMap<i64, (String, Vec<i64>)> = (0..=MAX_NODES as i64)
            .map(|i| {
                let children = if i == 0 {
                    (1..=MAX_NODES as i64).collect()
                } else {
                    vec![]
                };
                (i, (String::new(), children))
            })
            .collect();
        assert!(Rig::from_transforms(&[0], reader(&tree)).is_err());
    }

    #[test]
    fn crc32_known_values() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
