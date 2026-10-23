use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use nocturne_unpack::bundle::Bundle;
use nocturne_unpack::demon;
use nocturne_unpack::gltf;
use nocturne_unpack::install::{Install, DEFAULT_GAME_DIR};
use nocturne_unpack::unity::*;

#[derive(Parser)]
#[command(
    version,
    about = "Read the Unity bundles of Shin Megami Tensei III Nocturne HD Remaster"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Export a demon bundle (e.g. dvl0x01) as a skinned, textured .glb with its animation clips
    Export {
        /// A bundle name or path
        bundle: String,
        out: PathBuf,
        /// Which prefab of the bundle, by part of its asset path
        #[arg(long)]
        prefab: Option<String>,
        #[arg(long)]
        no_textures: bool,
        #[arg(long)]
        no_animations: bool,
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
    /// Load every demon bundle and count failures
    Check {
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
        /// Only bundles whose name contains this text
        #[arg(long)]
        only: Option<String>,
        /// Skip texture decoding
        #[arg(long)]
        no_textures: bool,
    },
    /// Object classes, texture formats and the like across all bundles
    Census {
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
        #[arg(long)]
        only: Option<String>,
    },
    /// Diagnostics for the clips of a demon bundle
    Anim {
        bundle: String,
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
    /// Print the objects of one class of a bundle as JSON (for exploring)
    Dump {
        bundle: String,
        class: i32,
        #[arg(long, default_value_t = 0)]
        nth: usize,
        /// Print the type tree first
        #[arg(long)]
        tree: bool,
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
}

fn rss_mb() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    // macOS reports bytes, Linux kilobytes
    #[cfg(target_os = "macos")]
    let bytes = ru.ru_maxrss as f64;
    #[cfg(not(target_os = "macos"))]
    let bytes = ru.ru_maxrss as f64 * 1024.0;
    bytes / 1048576.0
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Export {
            bundle,
            out,
            prefab,
            no_textures,
            no_animations,
            game,
        } => {
            let install = Install::open(&game)?;
            let path = install.resolve(&bundle)?;
            let b = Bundle::open(&path)?;
            let opts = demon::Options {
                textures: !no_textures,
                animations: !no_animations,
            };
            let asset = demon::load(&install, &b, prefab.as_deref(), &opts)?;
            let glb = gltf::to_glb(&asset)?;
            std::fs::write(&out, &glb).with_context(|| format!("write {}", out.display()))?;
            println!("{} -> {} ({} bytes)", asset.name, out.display(), glb.len());
            for (k, v) in demon::summary(&asset) {
                println!("  {k}: {v}");
            }
            for an in &asset.animations {
                println!(
                    "  clip {} {:.3} s, {} channels",
                    an.name,
                    an.duration,
                    an.channels.len()
                );
            }
            if !asset.report.left_out.is_empty() {
                println!("left out:");
                for (k, v) in &asset.report.left_out {
                    println!("  {v} x {k}");
                }
            }
        }
        Command::Check {
            game,
            only,
            no_textures,
        } => {
            let install = Install::open(&game)?;
            let t0 = Instant::now();
            let opts = demon::Options {
                textures: !no_textures,
                animations: true,
            };
            let (mut ok, mut failed) = (0, 0);
            let mut report = nocturne_unpack::asset::Report::default();
            let mut totals: BTreeMap<&'static str, usize> = BTreeMap::new();
            for p in install.demon_bundles() {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if only.as_deref().is_some_and(|o| !name.contains(o)) {
                    continue;
                }
                let r = Bundle::open(&p).and_then(|b| demon::load(&install, &b, None, &opts));
                match r {
                    Ok(a) => {
                        ok += 1;
                        for (k, v) in demon::summary(&a) {
                            *totals.entry(k).or_default() += v;
                        }
                        report.merge(&a.report);
                    }
                    Err(e) => {
                        failed += 1;
                        println!("FAIL {name}: {e:#}");
                    }
                }
            }
            println!(
                "{ok} demons loaded, {failed} failures, {:.1} s, peak RSS {:.0} MB",
                t0.elapsed().as_secs_f32(),
                rss_mb()
            );
            println!("totals: {totals:?}");
            if !report.left_out.is_empty() {
                println!("left out (all demons):");
                for (k, v) in &report.left_out {
                    println!("  {v} x {k}");
                }
            }
            if failed > 0 {
                std::process::exit(1);
            }
        }
        Command::Census { game, only } => census(&game, only.as_deref())?,
        Command::Anim { bundle, game } => anim_diag(&game, &bundle)?,
        Command::Dump {
            bundle,
            class,
            nth,
            tree,
            game,
        } => {
            let install = Install::open(&game)?;
            let b = Bundle::open(&install.resolve(&bundle)?)?;
            let ids = b.objects_of(class);
            let id = *ids
                .get(nth)
                .with_context(|| format!("only {} objects of class {class}", ids.len()))?;
            if tree {
                println!("{}", b.type_tree_dump(id)?);
            }
            let v: serde_json::Value = b.read(id)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
    }
    Ok(())
}

fn census(game: &std::path::Path, only: Option<&str>) -> Result<()> {
    let install = Install::open(game)?;
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut tex: BTreeMap<String, usize> = BTreeMap::new();
    let mut versions: BTreeMap<String, usize> = BTreeMap::new();
    let (mut ok, mut failed, mut no_tree) = (0, 0, 0);
    for p in install
        .base_bundles()
        .into_iter()
        .chain(install.dlc_bundles())
    {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if only.is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let b = match Bundle::open(&p) {
            Ok(b) => b,
            Err(e) => {
                failed += 1;
                println!("FAIL {name}: {e:#}");
                continue;
            }
        };
        ok += 1;
        if !b.has_type_tree() {
            no_tree += 1;
        }
        *versions.entry(b.unity_version()).or_default() += 1;
        for (_, c) in b.objects() {
            *classes.entry(Bundle::class_name(c)).or_default() += 1;
        }
        for id in b.objects_of(nocturne_unpack::bundle::class::TEXTURE_2D) {
            match b.read::<Texture2D>(id) {
                Ok(t) => {
                    let k = format!(
                        "{} ({}) {}",
                        nocturne_unpack::texture::format_name(t.m_TextureFormat),
                        t.m_TextureFormat,
                        if t.m_StreamData.size > 0 {
                            "streamed"
                        } else {
                            "inline"
                        }
                    );
                    *tex.entry(k).or_default() += 1;
                }
                Err(e) => println!("FAIL texture {id} in {name}: {e:#}"),
            }
        }
    }
    println!("bundles read: {ok}, failed: {failed}, without type tree: {no_tree}");
    println!("Unity versions: {versions:?}");
    println!("peak RSS {:.0} MB", rss_mb());
    println!("objects by class:");
    for (k, v) in &classes {
        println!("  {k}: {v}");
    }
    println!("Texture2D by format:");
    for (k, v) in &tex {
        println!("  {k}: {v}");
    }
    Ok(())
}

fn anim_diag(game: &std::path::Path, bundle: &str) -> Result<()> {
    use nocturne_unpack::{clip, rig};
    let install = Install::open(game)?;
    let b = Bundle::open(&install.resolve(bundle)?)?;
    let roots = demon::prefab_roots(&b)?;
    let r = rig::Rig::from_game_object(&b, roots[0].1)?;
    let root = r.first_with_class(95).unwrap_or(0);
    let paths = r.paths_below(root);
    let by_hash: std::collections::HashMap<u32, usize> = paths
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.as_ref().map(|p| (rig::crc32(p.as_bytes()), i)))
        .collect();
    let tos: std::collections::HashMap<u32, String> = b
        .objects_of(90)
        .iter()
        .flat_map(|&id| b.read::<Avatar>(id).map(|a| a.m_TOS).unwrap_or_default())
        .collect();
    if std::env::var_os("NOCT_PATHS").is_some() {
        for (i, p) in paths.iter().enumerate() {
            println!("node {i} {:?}", p);
        }
    }
    for id in demon::controller_clips(&b, &r, root)? {
        let c: AnimationClip = b.read(id)?;
        let cc = clip::read_curves(&c)?;
        let slots = clip::slots(&c.m_ClipBindingConstant.genericBindings);
        println!(
            "clip {} start {} stop {} bindings {} curves {}",
            c.m_Name,
            cc.start,
            cc.stop,
            slots.len(),
            cc.curves.len()
        );
        for s in &slots {
            let Some(prop) = s.prop else { continue };
            let node = by_hash.get(&s.binding.path);
            let comps: Vec<&clip::Curve> = (0..s.curve_count)
                .map(|k| &cc.curves[s.first_curve + k])
                .collect();
            let name = tos
                .get(&s.binding.path)
                .cloned()
                .unwrap_or_else(|| format!("hash {}", s.binding.path));
            let ch = if prop == clip::Prop::Euler {
                clip::build_euler_channel(&comps, clip::euler_order(s.binding.customType).unwrap())?
            } else {
                clip::build_channel(prop, &comps)?
            };
            if node.is_none() {
                println!("  UNRESOLVED {:?} {:?} {}", prop, name, ch.times.len());
            } else if prop == clip::Prop::Euler && ch.times.len() > 1 {
                println!(
                    "  EULER {name} custom {} keys {} (+{} approx) maxerr {:.3}",
                    s.binding.customType,
                    ch.times.len(),
                    ch.approximated,
                    ch.max_error_deg
                );
            } else if ch.steps_encoded > 0 {
                println!(
                    "  MIXED {:?} {name} keys {} steps {} interp {:?}",
                    prop,
                    ch.times.len(),
                    ch.steps_encoded,
                    ch.interp
                );
            }
        }
    }
    Ok(())
}
