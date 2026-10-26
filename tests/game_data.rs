//! Tests against a real install. Skipped unless `NOCTURNE_GAME_DIR` points at the game folder (the
//! one holding `smt3hd_Data`), e.g. `NOCTURNE_GAME_DIR=~/Games/smt3hd cargo test --release`.

use std::path::{Path, PathBuf};

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

// The command line, as a user runs it

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn cli(game: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_nocturne-unpack"));
    c.env("NOCTURNE_GAME_DIR", game)
        .env_remove("RUST_BACKTRACE");
    c
}

fn game_dir() -> Option<PathBuf> {
    PathBuf::from(std::env::var_os("NOCTURNE_GAME_DIR")?).into()
}

#[test]
fn a_closed_pipe_ends_the_command_quietly() {
    // `dump --tree | head`: the reader goes away after the first line of 50,000
    let Some(game) = game_dir() else { return };
    let mut child = cli(&game)
        .args(["dump", "dvl0x01", "74", "--nth", "0", "--tree"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    assert!(!first.is_empty());
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "{err}");
    assert!(!err.contains("Broken pipe"), "{err}");
}

#[test]
fn game_is_accepted_before_and_after_the_command() {
    let Some(game) = game_dir() else { return };
    let before = cli(Path::new("/nonexistent"))
        .arg("--game")
        .arg(&game)
        .args(["list", "demons", "--filter", "dvl0x01"])
        .output()
        .unwrap();
    let after = cli(Path::new("/nonexistent"))
        .args(["list", "demons", "--filter", "dvl0x01", "--game"])
        .arg(&game)
        .output()
        .unwrap();
    assert!(
        before.status.success(),
        "{}",
        String::from_utf8_lossy(&before.stderr)
    );
    assert_eq!(before.stdout, after.stdout);
    assert!(String::from_utf8_lossy(&before.stdout).contains("dvl0x01"));
}

#[test]
fn a_bundle_without_a_prefab_is_an_error_not_a_panic() {
    let Some(game) = game_dir() else { return };
    let install = Install::open(&game).unwrap();
    // 25 demon bundles hold only a data table and sounds
    let data_only = install
        .demon_bundles()
        .into_iter()
        .find(|p| {
            Bundle::open(p)
                .and_then(|b| prefab::prefab_roots(&b))
                .is_ok_and(|r| r.is_empty())
        })
        .expect("a demon bundle without a prefab");
    for args in [["anim"], ["probe"]] {
        let out = cli(&game).args(args).arg(&data_only).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {err}");
        assert!(err.contains("no prefab"), "{err}");
    }
}

/// Counts the triangles that wind with and against their own vertex normals: the cross product
/// of two edges against the sum of the three normals, which is what rewrite-bridge's
/// `geometry.winding_flipped` judges (triangles with no area or an undecided angle are skipped).
fn orientation(positions: &[[f32; 3]], normals: &[[f32; 3]], indices: &[u32]) -> (usize, usize) {
    let (mut with, mut against) = (0, 0);
    for t in indices.as_chunks::<3>().0 {
        let p = t.map(|i| positions[i as usize].map(f64::from));
        let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let (e1, e2) = (sub(p[1], p[0]), sub(p[2], p[0]));
        let cross = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let mut n = [0.0f64; 3];
        for &i in t {
            for (k, v) in n.iter_mut().enumerate() {
                *v += f64::from(normals[i as usize][k]);
            }
        }
        let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let scale = len(cross) * len(n);
        if scale.is_nan() || scale <= 1e-12 {
            continue;
        }
        let cos = (cross[0] * n[0] + cross[1] * n[1] + cross[2] * n[2]) / scale;
        if cos > 0.01 {
            with += 1;
        } else if cos < -0.01 {
            against += 1;
        }
    }
    (with, against)
}

/// `check-conformance --kind level` exits with status 1 because of `geometry.winding_flipped` in 37
/// field prefabs. The converted triangles must say the same as the Unity mesh they came from, so
/// that the finding is the game's data (a mesh stored against its normals) and not the X mirror
/// or the reversed indices. `f003` holds `md_hoji_01_01`, 120 triangles all against their normals.
#[test]
fn field_meshes_keep_their_winding_against_normals_through_the_conversion() {
    let Some(install) = install() else { return };
    let mut against_in_game = 0;
    for name in ["f003", "f004", "f023"] {
        let b = Bundle::open(&install.resolve(name).unwrap()).unwrap();
        let asset = prefab::load(
            &install,
            &b,
            prefab::Which::All,
            &Options {
                textures: false,
                animations: false,
            },
        )
        .unwrap();
        let mut meshes: std::collections::HashMap<i64, mesh::MeshData> = Default::default();
        let mut checked = 0;
        for p in &asset.primitives {
            if p.normals.is_empty() {
                continue;
            }
            let raw = meshes.entry(p.mesh_id).or_insert_with(|| {
                let m: Mesh = b.read(p.mesh_id).unwrap();
                mesh::decode(&b, &m).unwrap()
            });
            let sub: usize = p.name.rsplit('_').next().unwrap().parse().unwrap();
            let unity = orientation(&raw.positions, &raw.normals, &raw.submeshes[sub]);
            let bridge = orientation(&p.positions, &p.normals, &p.indices);
            assert_eq!(bridge, unity, "{name}: {}", p.name);
            against_in_game += usize::from(unity.1 > unity.0);
            checked += 1;
        }
        assert!(checked > 20, "{name}: {checked} primitives with normals");
    }
    // md_hoji_01_01 in f003, the room meshes of f023
    assert!(
        against_in_game >= 4,
        "{against_in_game} meshes stored against their normals"
    );
}

/// `geometry.non_finite` in `f004/f004_001`: the smoke meshes store NaN in two normals each, as
/// 32-bit floats. The check reads the bytes itself, so it does not depend on the decoder.
#[test]
fn the_smoke_meshes_of_f004_store_nan_normals() {
    let Some(install) = install() else { return };
    let b = Bundle::open(&install.resolve("f004").unwrap()).unwrap();
    let mut smoke = 0;
    for id in b.objects_of(class::MESH) {
        let m: Mesh = b.read(id).unwrap();
        if !m.m_Name.starts_with("md_01all_smoke_new") {
            continue;
        }
        let normal = m.m_VertexData.m_Channels[mesh::CH_NORMAL];
        assert_eq!(
            (normal.format, normal.dimension & 0x0F),
            (0, 3),
            "{}: float32 x 3",
            m.m_Name
        );
        let data = if m.m_VertexData.m_DataSize.is_empty() {
            b.read_stream(&m.m_StreamData).unwrap()
        } else {
            m.m_VertexData.m_DataSize.clone()
        };
        // Stream 0 starts at byte 0; its stride is the sum of its channels' sizes
        let stride: usize = m
            .m_VertexData
            .m_Channels
            .iter()
            .filter(|c| c.stream == 0 && c.dimension & 0x0F > 0)
            .map(|c| {
                (c.dimension & 0x0F) as usize
                    * if c.format == 0 {
                        4
                    } else if c.format == 2 {
                        1
                    } else {
                        2
                    }
            })
            .sum();
        let nan = (0..m.m_VertexData.m_VertexCount as usize)
            .filter(|v| {
                let at = v * stride + normal.offset as usize;
                (0..3).any(|k| {
                    let bits =
                        u32::from_le_bytes(data[at + 4 * k..at + 4 * k + 4].try_into().unwrap());
                    bits & 0x7F80_0000 == 0x7F80_0000 && bits & 0x007F_FFFF != 0
                })
            })
            .count();
        assert_eq!(
            nan, 2,
            "{}: vertices with a NaN normal in the file",
            m.m_Name
        );
        smoke += 1;
    }
    assert_eq!(smoke, 7);
}
