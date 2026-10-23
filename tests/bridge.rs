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
        demons, 232,
        "demons with a model (257 bundles, 25 hold only data)"
    );
    assert!(levels > 900 && levels < 1066, "{levels} field prefabs");
    // the DLC folders repeat base game bundles; a demon must not be listed twice
    let mut ids: Vec<&str> = all
        .iter()
        .filter(|a| a.kind == AssetKind::Character)
        .map(|a| a.id.as_str())
        .collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "demons listed twice");
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
fn prefabs_with_nothing_to_draw_are_not_listed() {
    let Some(p) = provider() else { return };
    // f740_001 holds transforms only. Listing it would hand out a scene with no instances.
    assert!(p.list("f740/f740_001").unwrap().is_empty());
    // It still loads, as an empty scene, if asked for by id
    assert!(p.load_scene("f740/f740_001").unwrap().instances.is_empty());
    // The listed ones that were checked have something to place
    for id in ["f002/f002_001", "f012/f012_002", "f804/f804_060"] {
        assert_eq!(p.list(id).unwrap().len(), 1, "{id}");
        assert!(!p.load_scene(id).unwrap().instances.is_empty(), "{id}");
    }
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

#[test]
fn open_takes_anything_that_is_a_path() {
    let Some(dir) = std::env::var_os("NOCTURNE_GAME_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let text = dir.to_str().unwrap().to_string();
    // &str, String, &Path, PathBuf and &PathBuf
    let counts = [
        Nocturne::open(text.as_str()).unwrap(),
        Nocturne::open(text.clone()).unwrap(),
        Nocturne::open(dir.as_path()).unwrap(),
        Nocturne::open(dir.clone()).unwrap(),
        Nocturne::open(&dir).unwrap(),
    ]
    .map(|p| p.install().demon_bundles().len());
    assert!(
        counts.iter().all(|&n| n == counts[0] && n > 200),
        "{counts:?}"
    );
}
