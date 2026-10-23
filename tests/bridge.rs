//! The rewrite-bridge provider against a real install (feature `bridge`). Skipped unless
//! `NOCTURNE_GAME_DIR` points at the game folder.

#![cfg(feature = "bridge")]

use std::path::PathBuf;

use nocturne_unpack::bridge::Nocturne;
use rewrite_bridge::{AssetKind, NotSupported, Provider, SceneModel};

fn provider() -> Option<Nocturne> {
    let dir = PathBuf::from(std::env::var_os("NOCTURNE_GAME_DIR")?);
    Some(Nocturne::open(&dir).expect("NOCTURNE_GAME_DIR is not a Nocturne HD install"))
}

#[test]
fn lists_demons_and_field_prefabs() {
    let Some(p) = provider() else { return };
    let all = p.list("").unwrap();
    let demons = all
        .iter()
        .filter(|a| a.kind == AssetKind::Character)
        .count();
    let levels = all.iter().filter(|a| a.kind == AssetKind::Level).count();
    assert_eq!(
        demons, 234,
        "demons with a model (259 bundles, 25 hold only data)"
    );
    assert!(levels > 1000, "{levels} field prefabs");
    let first = p.list("dvl0x01").unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, "dvl0x01");
    assert_eq!(first[0].name, "devil_0x01");
    assert!(p
        .list("f002/f002_001")
        .unwrap()
        .iter()
        .any(|a| a.id == "f002/f002_001"));
}

#[test]
fn a_demon_loads_with_its_clips() {
    let Some(p) = provider() else { return };
    let m = p.load_animated("dvl0x01").unwrap();
    assert_eq!(m.validate(), Ok(()));
    assert_eq!(m.animations.len(), 5);
    assert_eq!(m.model.skins.len(), 1);
    assert!(m.model.materials.iter().any(|x| x.base_color.is_some()));
    // `load` is the animated model's model
    assert_eq!(p.load("dvl0x01").unwrap(), m.model);
    // The root of the prefab is first, parents come before their children
    assert!(m.model.nodes[0].parent.is_none());
}

#[test]
fn a_demon_with_euler_clips_validates() {
    let Some(p) = provider() else { return };
    // Every clip of dvl0x07 is made of Euler rotation curves
    let m = p.load_animated("dvl0x07").unwrap();
    assert_eq!(m.validate(), Ok(()));
    assert!(m.animations.iter().all(|a| !a.channels.is_empty()));
}

#[test]
fn a_field_prefab_loads_as_a_scene() {
    let Some(p) = provider() else { return };
    let scene = p.load_scene("f002/f002_001").unwrap();
    assert_eq!(scene.validate(), Ok(()));
    assert!(!scene.instances.is_empty());
    assert!(scene
        .models
        .iter()
        .all(|m| matches!(m, SceneModel::Inline(_))));
    // Demons are not scenes
    let e = p.load_scene("dvl0x01").unwrap_err();
    assert!(e.downcast_ref::<NotSupported>().is_some());
    // The same prefab as one static model
    let model = p.load("f002/f002_001").unwrap();
    assert_eq!(model.validate(), Ok(()));
}
