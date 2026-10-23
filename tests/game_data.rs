//! Tests against a real install. Skipped unless `NOCTURNE_GAME_DIR` points at the game folder (the
//! one holding `smt3hd_Data`), e.g. `NOCTURNE_GAME_DIR=~/Games/smt3hd cargo test --release`.

use std::path::PathBuf;

use nocturne_unpack::bundle::{class, Bundle};
use nocturne_unpack::clip;
use nocturne_unpack::install::Install;
use nocturne_unpack::prefab::{self, Options};
use nocturne_unpack::unity::{AnimationClip, Mesh, Texture2D};
use nocturne_unpack::{mesh, texture};

fn install() -> Option<Install> {
    let dir = PathBuf::from(std::env::var_os("NOCTURNE_GAME_DIR")?);
    Some(Install::open(&dir).expect("NOCTURNE_GAME_DIR is not a Nocturne HD install"))
}

#[test]
fn every_base_bundle_opens_with_type_trees() {
    let Some(install) = install() else { return };
    let bundles = install.base_bundles();
    assert!(bundles.len() > 2000, "{} bundles", bundles.len());
    // One at a time: a bundle's objects are held in memory while it is open
    for p in bundles.iter().step_by(37) {
        let b = Bundle::open(p).unwrap_or_else(|e| panic!("{}: {e:#}", p.display()));
        assert!(b.has_type_tree(), "{}", p.display());
        assert!(b.unity_version().starts_with("20"), "{}", b.unity_version());
    }
}

#[test]
fn streamed_texture_ranges_come_from_the_res_s_entry() {
    let Some(install) = install() else { return };
    let b = Bundle::open(&install.resolve("dvl0x01").unwrap()).unwrap();
    let id = b.objects_of(class::TEXTURE_2D)[0];
    let t: Texture2D = b.read(id).unwrap();
    assert!(t.image_data.is_empty() && t.m_StreamData.size > 0);
    let raw = b.read_stream(&t.m_StreamData).unwrap();
    assert_eq!(raw.len(), t.m_StreamData.size as usize);
    // The largest level is DXT1 or DXT5: at least a byte per pixel / 2
    let img = texture::decode(&b, &t).unwrap();
    assert_eq!(
        (img.width as i32, img.height as i32),
        (t.m_Width, t.m_Height)
    );
    assert_eq!(img.rgba.len(), img.width as usize * img.height as usize * 4);
}

#[test]
fn demon_mesh_matches_what_another_reader_gave() {
    let Some(install) = install() else { return };
    let b = Bundle::open(&install.resolve("dvl0x01").unwrap()).unwrap();
    // The body mesh of the prefab has 2,078 vertices (UnityPy reads the same numbers)
    let m = b
        .objects_of(class::MESH)
        .into_iter()
        .filter_map(|id| b.read::<Mesh>(id).ok())
        .find(|m| m.m_Name == "0x01 body" && m.m_VertexData.m_VertexCount == 2078)
        .expect("body mesh");
    let d = mesh::decode(&b, &m).unwrap();
    assert_eq!(d.positions.len(), 2078);
    let near = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-7);
    assert!(near(
        &d.positions[0],
        &[-0.139_159_05, 0.002_260_491_5, 0.394_623_43]
    ));
    assert!(near(
        &d.normals[0],
        &[-0.947_794_9, -0.279_134_8, -0.154_169_92]
    ));
    assert!(near(&d.uv0[0], &[0.658_536_9, 0.824_610_8]));
    assert_eq!(
        (d.joints[0], d.weights[0]),
        ([2, 0, 0, 0], [1.0, 0.0, 0.0, 0.0])
    );
    assert_eq!(d.bind_poses.len(), 35);
    assert_eq!(d.submeshes.len(), 13);
    assert_eq!(&d.submeshes[0][..9], &[0, 1, 2, 2, 3, 4, 3, 5, 4]);
}

#[test]
fn demon_clip_samples_like_the_polynomials() {
    let Some(install) = install() else { return };
    let b = Bundle::open(&install.resolve("dvl0x01").unwrap()).unwrap();
    let roots = prefab::prefab_roots(&b).unwrap();
    let rig = nocturne_unpack::rig::Rig::from_game_object(
        &b,
        roots[prefab::main_prefab(&b, &roots).unwrap()].1,
    )
    .unwrap();
    let root = rig.first_with_class(class::ANIMATOR).unwrap();
    let clips = prefab::controller_clips(&b, &rig, root).unwrap();
    assert_eq!(clips.len(), 5);
    let c: AnimationClip = b
        .read(
            *clips
                .iter()
                .find(|&&id| b.read::<AnimationClip>(id).unwrap().m_Name == "b00")
                .unwrap(),
        )
        .unwrap();
    let cc = clip::read_curves(&c).unwrap();
    assert_eq!(cc.gaps, 0);
    let slots = clip::slots(&c.m_ClipBindingConstant.genericBindings);
    // "p Pelvis/b Spine": a rotation of four curves
    let hash = nocturne_unpack::rig::crc32(b"p Pelvis/b Spine");
    let s = slots
        .iter()
        .find(|s| s.binding.path == hash && s.prop == Some(clip::Prop::Rotation))
        .unwrap();
    let comps: Vec<&clip::Curve> = (0..4).map(|k| &cc.curves[s.first_curve + k]).collect();
    let ch = clip::build_channel(clip::Prop::Rotation, &comps).unwrap();
    let q = clip::sample_channel(&ch, 0.5);
    // Evaluated independently from the raw streamed data (Python, double precision), normalised
    let want = [0.003_974_342_6, -0.010_238_058, 0.057_769_693, 0.998_269_5];
    for i in 0..4 {
        assert!((q[i] - want[i]).abs() < 1e-6, "{q:?} vs {want:?}");
    }
}

#[test]
fn demon_0x01_loads_with_skin_and_clips() {
    let Some(install) = install() else { return };
    let b = Bundle::open(&install.resolve("dvl0x01").unwrap()).unwrap();
    let a = prefab::load(&install, &b, prefab::Which::Main, &Options::default()).unwrap();
    assert_eq!(a.name, "devil_0x01");
    assert_eq!(a.nodes.len(), 68);
    assert_eq!(a.skins.len(), 1);
    assert_eq!(a.skins[0].joints.len(), 35);
    assert_eq!(a.animations.len(), 5);
    assert!(a.primitives.len() >= 30);
    assert!(a.images.len() >= 16);
    // Every channel's node exists and times increase
    for an in &a.animations {
        for c in &an.channels {
            assert!(c.node < a.nodes.len());
            assert!(c.times.windows(2).all(|w| w[0] < w[1]));
            assert!(*c.times.last().unwrap() <= an.duration + 1e-6);
        }
    }
    let glb = nocturne_unpack::gltf::to_glb(&a).unwrap();
    assert_eq!(&glb[..4], b"glTF");
}

#[test]
fn every_demon_loads() {
    let Some(install) = install() else { return };
    let opts = Options {
        textures: false,
        animations: true,
    };
    let mut loaded = 0;
    for p in install.demon_bundles() {
        let b = Bundle::open(&p).unwrap();
        let roots = prefab::prefab_roots(&b).unwrap();
        if roots.is_empty() {
            // The bundles without a model only carry a data table and sounds
            assert!(b.objects_of(class::MESH).is_empty(), "{}", p.display());
            continue;
        }
        let a = prefab::load(&install, &b, prefab::Which::Main, &opts)
            .unwrap_or_else(|e| panic!("{}: {e:#}", p.display()));
        assert!(!a.primitives.is_empty(), "{}", p.display());
        loaded += 1;
    }
    assert!(loaded >= 230, "{loaded} demons");
}
