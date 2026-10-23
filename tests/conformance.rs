//! The conformance checker of rewrite-bridge against a real install (feature `bridge`). Skipped
//! unless `NOCTURNE_GAME_DIR` points at the game folder.

#![cfg(feature = "bridge")]

use std::path::PathBuf;

use nocturne_unpack::bridge::Nocturne;
use rewrite_bridge::conformance::{check_animated, check_scene, Expect};
use rewrite_bridge::Provider;

fn provider() -> Option<Nocturne> {
    let dir = PathBuf::from(std::env::var_os("NOCTURNE_GAME_DIR")?);
    Some(Nocturne::open(&dir).expect("NOCTURNE_GAME_DIR is not a Nocturne HD install"))
}

#[test]
fn demons_have_no_conformance_errors() {
    let Some(p) = provider() else { return };
    // A demon with Euler clips, one with skinned parts and rigid ones, a snake
    for id in ["dvl0x01", "dvl0x07", "dvl0x4b", "dvl0xc0d"] {
        let model = p.load_animated(id).unwrap();
        let report = check_animated(
            &model,
            &Expect::default().allowing("scale.height_out_of_range"),
        );
        assert!(report.passed(), "{id}: {report}");
        // Triangles wind the way their normals say: the X mirror and the reversed indices agree
        assert!(!report.has("geometry.winding_flipped"), "{id}: {report}");
        assert!(!report.has("skin.inverse_bind_singular"), "{id}: {report}");
    }
}

#[test]
fn field_scenes_have_no_conformance_errors_or_empty_scenes() {
    let Some(p) = provider() else { return };
    for id in ["f002/f002_001", "f012/f012_002", "f015/f015_001"] {
        let scene = p.load_scene(id).unwrap();
        let report = check_scene(&scene, &Expect::default());
        assert!(report.passed(), "{id}: {report}");
        assert!(!report.has("scene.empty"), "{id}: {report}");
    }
}
