use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use nocturne_unpack::bundle::Bundle;
use nocturne_unpack::gltf;
use nocturne_unpack::install::{Install, DEFAULT_GAME_DIR, GAME_DIR_ENV};
use nocturne_unpack::prefab;
use nocturne_unpack::unity::*;

#[derive(Parser)]
#[command(
    version,
    about = "Read the Unity bundles of Shin Megami Tensei III Nocturne HD Remaster"
)]
struct Cli {
    /// Name every material property that is not carried over
    #[arg(short, long, global = true)]
    verbose: bool,
    /// The game folder (the one that holds `smt3hd_Data`), before or after the command
    #[arg(long, global = true, env = GAME_DIR_ENV, default_value = DEFAULT_GAME_DIR)]
    game: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarise the install, list demons or fields, or show what a bundle holds
    List {
        /// `demons`, `fields`, a bundle name or path; nothing for a summary of all bundles
        what: Option<String>,
        /// Only bundles whose name contains this text (with `demons` or `fields`)
        #[arg(long)]
        filter: Option<String>,
    },
    /// Export a demon bundle (e.g. dvl0x01) as a skinned, textured .glb with its animation clips
    Export {
        /// A bundle name or path
        bundle: String,
        out: PathBuf,
        /// Which prefab of the bundle, by part of its asset path (default: the one with the
        /// most renderers)
        #[arg(long = "prefab", conflicts_with = "all_prefabs")]
        which: Option<String>,
        /// Export every prefab of the bundle (a field is made of several)
        #[arg(long)]
        all_prefabs: bool,
        #[arg(long)]
        no_textures: bool,
        #[arg(long)]
        no_animations: bool,
    },
    /// Load every demon bundle and count failures
    Check {
        /// Only bundles whose name contains this text
        #[arg(long)]
        only: Option<String>,
        /// `demons` (the default) or `fields`; a field is loaded with all its prefabs at once
        #[arg(long, default_value = "demons")]
        kind: String,
        /// Skip texture decoding
        #[arg(long)]
        no_textures: bool,
    },
    /// Object classes, texture formats and the like across all bundles
    Census {
        /// Only bundles whose name contains this text
        #[arg(long)]
        only: Option<String>,
        /// Also decode every texture and mesh and count the failures
        #[arg(long)]
        decode: bool,
    },
    /// Load every demon and field prefab through the rewrite-bridge Provider and validate it
    /// (feature `bridge`)
    #[cfg(feature = "bridge")]
    CheckBridge {
        /// Only assets whose id or name contains this text
        #[arg(long)]
        only: Option<String>,
        /// `demons`, `fields` or `all`
        #[arg(long, default_value = "all")]
        kind: String,
        /// Decode textures and build the materials with them (much slower, and the largest
        /// fields need more than 1 GB)
        #[arg(long)]
        textures: bool,
    },
    /// Print numbers for cross-checking against another reader (JSON lines)
    Probe {
        bundle: String,
        /// A mesh by name: vertex count, first vertex, bone weights, bind pose
        #[arg(long)]
        mesh: Option<String>,
        /// A node by path below the animator: its local transform
        #[arg(long)]
        node: Option<String>,
        /// A clip by name (with --node and --time): the sampled transform
        #[arg(long)]
        clip: Option<String>,
        #[arg(long, default_value_t = 0.0)]
        time: f32,
    },
    /// Check the provider's output against the bridge's conventions (feature `bridge`). Takes the
    /// options of `rewrite_bridge::conformance::cli` (`--id`, `--filter`, `--kind`, `--limit`,
    /// `--allow`, `-v`, `--strict`, `--compare-load`), plus `--game DIR` and `--no-textures`
    #[cfg(feature = "bridge")]
    CheckConformance {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        args: Vec<String>,
    },
    /// Diagnostics for the clips of a demon bundle
    Anim { bundle: String },
    /// Write one texture of a bundle as PNG (the largest level), or list the textures
    Texture {
        bundle: String,
        /// The texture's name; without it the bundle's textures are listed
        name: Option<String>,
        /// Output PNG
        #[arg(short, long)]
        out: Option<PathBuf>,
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
    },
}

/// Hands freed memory back to the system, so that a sweep over many bundles does not keep what
/// earlier ones used (macOS keeps freed blocks resident otherwise).
fn release_memory() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means all zones; the call only releases free memory
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}

/// Peak resident set size in MB; 0 where getrusage is not available (Windows)
fn rss_mb() -> f64 {
    #[cfg(unix)]
    {
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
        // macOS reports bytes, Linux kilobytes
        #[cfg(target_os = "macos")]
        let bytes = ru.ru_maxrss as f64;
        #[cfg(not(target_os = "macos"))]
        let bytes = ru.ru_maxrss as f64 * 1024.0;
        bytes / 1048576.0
    }
    #[cfg(not(unix))]
    0.0
}

/// Rust ignores SIGPIPE, so `println!` panics when the reader of a pipe goes away
/// (`dump --tree | head`). Back to the default, the process just ends, as it does for `cat`.
fn reset_sigpipe() {
    #[cfg(unix)]
    // SAFETY: setting the default disposition of a signal calls no Rust code
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn main() -> Result<()> {
    reset_sigpipe();
    let cli = Cli::parse();
    let verbose = cli.verbose;
    let game = cli.game;
    match cli.command {
        Command::List { what, filter } => list(&game, what.as_deref(), filter.as_deref())?,
        Command::Export {
            bundle,
            out,
            which,
            all_prefabs,
            no_textures,
            no_animations,
        } => {
            let install = Install::open(&game)?;
            let path = install.resolve(&bundle)?;
            let b = Bundle::open(&path)?;
            let opts = prefab::Options {
                textures: !no_textures,
                animations: !no_animations,
            };
            let asset = prefab::load(&install, &b, pick(&which, all_prefabs), &opts)?;
            let glb = gltf::to_glb(&asset)?;
            std::fs::write(&out, &glb).with_context(|| format!("write {}", out.display()))?;
            println!("{} -> {} ({} bytes)", asset.name, out.display(), glb.len());
            for (k, v) in prefab::summary(&asset) {
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
            print_report(&asset.report, verbose);
        }
        Command::Check {
            only,
            kind,
            no_textures,
        } => {
            let install = Install::open(&game)?;
            anyhow::ensure!(
                kind == "demons" || kind == "fields",
                "--kind is demons or fields"
            );
            let fields = kind == "fields";
            let t0 = Instant::now();
            let opts = prefab::Options {
                textures: !no_textures,
                animations: true,
            };
            let (mut ok, mut failed, mut data_only) = (0, 0, 0);
            let mut report = nocturne_unpack::asset::Report::default();
            let mut totals: BTreeMap<&'static str, usize> = BTreeMap::new();
            let mut bundles = 0;
            let mut shaders: BTreeMap<String, usize> = BTreeMap::new();
            let mut tiled = 0;
            let list = if fields {
                install.field_bundles()
            } else {
                install.demon_bundles()
            };
            for p in list {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if only.as_deref().is_some_and(|o| !name.contains(o)) {
                    continue;
                }
                bundles += 1;
                let b = match Bundle::open(&p) {
                    Ok(b) => b,
                    Err(e) => {
                        failed += 1;
                        println!("FAIL {name}: {e:#}");
                        continue;
                    }
                };
                // Some demon bundles only hold a data table and sounds
                if prefab::prefab_roots(&b).is_ok_and(|r| r.is_empty())
                    && b.objects_of(nocturne_unpack::bundle::class::MESH)
                        .is_empty()
                {
                    data_only += 1;
                    continue;
                }
                let which = if fields {
                    prefab::Which::All
                } else {
                    prefab::Which::Main
                };
                match prefab::load(&install, &b, which, &opts) {
                    Ok(a) => {
                        ok += 1;
                        for m in &a.materials {
                            *shaders.entry(m.shader.clone()).or_default() += 1;
                            tiled += usize::from(
                                m.base_color.as_ref().is_some_and(|t| t.transform.is_some()),
                            );
                        }
                        for (k, v) in prefab::summary(&a) {
                            *totals.entry(k).or_default() += v;
                        }
                        report.merge(&a.report);
                    }
                    Err(e) => {
                        failed += 1;
                        println!("FAIL {name}: {e:#}");
                    }
                }
                release_memory();
            }
            println!(
                "{bundles} {kind} bundles: {ok} loaded, {data_only} without a Unity model (only a data table and sounds, or console-era files), {failed} failures; {:.1} s, peak RSS {:.0} MB",
                t0.elapsed().as_secs_f32(),
                rss_mb()
            );
            println!("totals: {totals:?}");
            println!("materials by shader: {shaders:?}; {tiled} with a tiled or offset _MainTex");
            println!("(all {kind})");
            print_report(&report, verbose);
            if failed > 0 {
                std::process::exit(1);
            }
        }
        Command::Census { only, decode } => census(&game, only.as_deref(), decode)?,
        #[cfg(feature = "bridge")]
        Command::CheckBridge {
            only,
            kind,
            textures,
        } => check_bridge(&game, only.as_deref(), &kind, textures)?,
        #[cfg(feature = "bridge")]
        Command::CheckConformance { args } => {
            // `--game` before the command is the global one; the checker's own options come after
            let (mut game, mut textures) = (game, true);
            let mut rest = Vec::new();
            let mut it = args.into_iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--game" => game = PathBuf::from(it.next().context("--game needs a folder")?),
                    "--no-textures" => textures = false,
                    _ => rest.push(a),
                }
            }
            let provider = nocturne_unpack::bridge::Nocturne::open(&game)?.with_textures(textures);
            std::process::exit(rewrite_bridge::conformance::cli::run(&provider, rest));
        }
        Command::Probe {
            bundle,
            mesh,
            node,
            clip,
            time,
        } => probe(&game, &bundle, mesh, node, clip, time)?,
        Command::Anim { bundle } => anim_diag(&game, &bundle)?,
        Command::Texture { bundle, name, out } => {
            texture_cmd(&game, &bundle, name.as_deref(), out.as_deref())?
        }
        Command::Dump {
            bundle,
            class,
            nth,
            tree,
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

fn census(game: &std::path::Path, only: Option<&str>, decode: bool) -> Result<()> {
    let install = Install::open(game)?;
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut tex: BTreeMap<String, usize> = BTreeMap::new();
    let mut versions: BTreeMap<String, usize> = BTreeMap::new();
    let (mut ok, mut failed, mut no_tree) = (0, 0, 0);
    let mut tex_decoded: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let (mut meshes_ok, mut meshes_failed) = (0usize, 0usize);
    let mut errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut peak_at = (0.0f64, String::new());
    let mut examples: BTreeMap<String, String> = BTreeMap::new();
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
                    *tex.entry(k.clone()).or_default() += 1;
                    examples
                        .entry(k)
                        .or_insert_with(|| format!("{name}: {}", t.m_Name));
                    if decode {
                        let fmt = format!(
                            "{} ({})",
                            nocturne_unpack::texture::format_name(t.m_TextureFormat),
                            t.m_TextureFormat
                        );
                        let e = tex_decoded.entry(fmt).or_default();
                        match nocturne_unpack::texture::decode(&b, &t) {
                            Ok(_) => e.0 += 1,
                            Err(err) => {
                                e.1 += 1;
                                let msg = format!("texture: {err:#}");
                                *errors.entry(msg.chars().take(100).collect()).or_default() += 1;
                            }
                        }
                    }
                }
                Err(e) => println!("FAIL texture {id} in {name}: {e:#}"),
            }
        }
        if decode {
            for id in b.objects_of(nocturne_unpack::bundle::class::MESH) {
                match b
                    .read::<Mesh>(id)
                    .and_then(|m| nocturne_unpack::mesh::decode(&b, &m))
                {
                    Ok(_) => meshes_ok += 1,
                    Err(err) => {
                        meshes_failed += 1;
                        *errors
                            .entry(format!("mesh: {err:#}").chars().take(100).collect())
                            .or_default() += 1;
                    }
                }
            }
        }
        release_memory();
        let r = rss_mb();
        if r > peak_at.0 {
            peak_at = (r, name.clone());
        }
    }
    println!("bundles read: {ok}, failed: {failed}, without type tree: {no_tree}");
    println!(
        "peak RSS reached while reading {} ({:.0} MB)",
        peak_at.1, peak_at.0
    );
    println!("Unity versions: {versions:?}");
    println!("peak RSS {:.0} MB", rss_mb());
    println!("objects by class:");
    for (k, v) in &classes {
        println!("  {k}: {v}");
    }
    println!("Texture2D by format:");
    for (k, v) in &tex {
        println!("  {k}: {v}   (e.g. {})", examples[k]);
    }
    if decode {
        println!("decoded textures (ok / failed) by format:");
        for (k, (a, f)) in &tex_decoded {
            println!("  {k}: {a} / {f}");
        }
        println!("decoded meshes: {meshes_ok} ok, {meshes_failed} failed");
        for (k, v) in &errors {
            println!("  {v} x {k}");
        }
    }
    Ok(())
}

fn anim_diag(game: &std::path::Path, bundle: &str) -> Result<()> {
    use nocturne_unpack::{clip, rig};
    let install = Install::open(game)?;
    let b = Bundle::open(&install.resolve(bundle)?)?;
    let roots = prefab::prefab_roots(&b)?;
    let r = rig::Rig::from_game_object(&b, roots[prefab::main_prefab(&b, &roots)?].1)?;
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
    for id in prefab::controller_clips(&b, &r, root)? {
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
                clip::build_euler_channel(&comps, clip::euler_order_of(s.binding.customType)?)?
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

#[cfg(feature = "bridge")]
fn check_bridge(
    game: &std::path::Path,
    only: Option<&str>,
    kind: &str,
    textures: bool,
) -> Result<()> {
    use rewrite_bridge::{AssetKind, Provider, SceneModel};
    let t0 = Instant::now();
    let provider = nocturne_unpack::bridge::Nocturne::open(game)?.with_textures(textures);
    let assets = provider
        .list(only.unwrap_or(""))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let (mut ok, mut failed) = (0, 0);
    let (mut demons, mut fields) = (0, 0);
    let (mut meshes, mut clips, mut channels, mut verts) = (0usize, 0usize, 0usize, 0usize);
    let (mut instances, mut models) = (0usize, 0usize);
    let mut peak = (0.0f64, String::new());
    for d in &assets {
        let is_level = d.kind == AssetKind::Level;
        if (kind == "demons" && is_level) || (kind == "fields" && !is_level) {
            continue;
        }
        if is_level {
            fields += 1;
            // A field prefab loads both ways: as one static model and as a scene
            // The static model is checked and dropped before the scene is built
            let r = provider
                .load(&d.id)
                .and_then(|m| m.validate().map_err(Into::into))
                .and_then(|()| provider.load_scene(&d.id));
            match r {
                Ok(sc) => {
                    ok += 1;
                    instances += sc.instances.len();
                    models += sc.models.len();
                    for m in &sc.models {
                        if let SceneModel::Inline(m) = m {
                            meshes += m.meshes.len();
                            verts += m.meshes.iter().map(|x| x.positions.len()).sum::<usize>();
                        }
                    }
                }
                Err(e) => {
                    failed += 1;
                    println!("FAIL {}: {e}", d.id);
                }
            }
        } else {
            demons += 1;
            match provider.load_animated(&d.id) {
                Ok(m) => {
                    ok += 1;
                    meshes += m.model.meshes.len();
                    verts += m
                        .model
                        .meshes
                        .iter()
                        .map(|x| x.positions.len())
                        .sum::<usize>();
                    clips += m.animations.len();
                    channels += m.animations.iter().map(|a| a.channels.len()).sum::<usize>();
                    // The plain model is the animated one's model
                    match provider.load(&d.id) {
                        Ok(plain) if plain == m.model => {}
                        Ok(_) => {
                            failed += 1;
                            println!("FAIL {}: load differs from load_animated().model", d.id);
                        }
                        Err(e) => {
                            failed += 1;
                            println!("FAIL {}: {e}", d.id);
                        }
                    }
                }
                Err(e) => {
                    failed += 1;
                    println!("FAIL {}: {e}", d.id);
                }
            }
        }
        release_memory();
        let r = rss_mb();
        if r > peak.0 {
            peak = (r, d.id.clone());
        }
    }
    println!(
        "peak RSS reached while loading {} ({:.0} MB)",
        peak.1, peak.0
    );
    println!(
        "{ok} of {} assets loaded and validated ({demons} demons, {fields} field prefabs), {failed} failures",
        demons + fields
    );
    println!(
        "{meshes} meshes, {verts} vertices, {clips} clips, {channels} channels, {models} scene models placed by {instances} instances; {:.1} s, peak RSS {:.0} MB",
        t0.elapsed().as_secs_f32(),
        rss_mb()
    );
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn print_report(r: &nocturne_unpack::asset::Report, verbose: bool) {
    if r.left_out.is_empty() && r.unmapped.is_empty() {
        return;
    }
    println!("left out:");
    for (k, v) in &r.left_out {
        println!("  {v} x {k}");
    }
    for l in r.unmapped_lines(verbose) {
        println!("  {l}");
    }
}

fn pick(which: &Option<String>, all: bool) -> prefab::Which<'_> {
    match (which, all) {
        (_, true) => prefab::Which::All,
        (Some(w), _) => prefab::Which::Named(w),
        _ => prefab::Which::Main,
    }
}

/// Numbers for cross-checking against another reader, as JSON lines.
fn probe(
    game: &std::path::Path,
    bundle: &str,
    mesh_name: Option<String>,
    node: Option<String>,
    clip_name: Option<String>,
    time: f32,
) -> Result<()> {
    use nocturne_unpack::{clip, mesh, rig};
    use serde_json::json;
    let install = Install::open(game)?;
    let b = Bundle::open(&install.resolve(bundle)?)?;
    let roots = prefab::prefab_roots(&b)?;
    let root_go = roots[prefab::main_prefab(&b, &roots)?].1;
    let r = rig::Rig::from_game_object(&b, root_go)?;
    let root = r
        .first_with_class(nocturne_unpack::bundle::class::ANIMATOR)
        .unwrap_or(0);
    let paths = r.paths_below(root);
    if let Some(name) = mesh_name {
        // The mesh the prefab's renderers use (a bundle can hold several of the same name)
        for n in &r.nodes {
            for c in &n.components {
                if c.class != nocturne_unpack::bundle::class::SKINNED_MESH_RENDERER {
                    continue;
                }
                let rd: Renderer = b.read(c.id)?;
                let Some(id) = rd.m_Mesh.local() else {
                    continue;
                };
                let m: Mesh = b.read(id)?;
                if m.m_Name != name {
                    continue;
                }
                let d = mesh::decode(&b, &m)?;
                println!(
                    "{}",
                    json!({
                        "mesh": d.name, "vertices": d.positions.len(),
                        "position0": d.positions.first(), "normal0": d.normals.first(), "uv0_0": d.uv0.first(),
                        "joints0": d.joints.first(), "weights0": d.weights.first(),
                        "submeshes": d.submeshes.len(),
                        "first_indices_sub0": d.submeshes.first().map(|s| s.iter().take(9).collect::<Vec<_>>()),
                        "bind_poses": d.bind_poses.len(), "bind_pose0": d.bind_poses.first(),
                        "bones_in_renderer": rd.m_Bones.len(),
                    })
                );
            }
        }
    }
    let Some(path) = &node else { return Ok(()) };
    let Some(ni) = paths
        .iter()
        .position(|p| p.as_deref() == Some(path.as_str()))
    else {
        anyhow::bail!("no node {path:?}");
    };
    let n = &r.nodes[ni];
    println!(
        "{}",
        json!({"node": path, "unity_translation": n.translation, "unity_rotation": n.rotation, "unity_scale": n.scale, "hash": rig::crc32(path.as_bytes())})
    );
    let Some(cn) = clip_name else { return Ok(()) };
    let hash = rig::crc32(path.as_bytes());
    for id in prefab::controller_clips(&b, &r, root)? {
        let c: AnimationClip = b.read(id)?;
        if c.m_Name != cn {
            continue;
        }
        let cc = clip::read_curves(&c)?;
        let slots = clip::slots(&c.m_ClipBindingConstant.genericBindings);
        for s in slots.iter().filter(|s| s.binding.path == hash) {
            let Some(prop) = s.prop else { continue };
            let comps: Vec<&clip::Curve> = (0..s.curve_count)
                .map(|k| &cc.curves[s.first_curve + k])
                .collect();
            let ch = if prop == clip::Prop::Euler {
                let order = clip::euler_order_of(s.binding.customType)?;
                clip::build_euler_channel(&comps, order)?
            } else {
                clip::build_channel(prop, &comps)?
            };
            // The curves' polynomials evaluated directly, and the channel as glTF samples it
            let direct: Vec<f32> = comps.iter().map(|c| c.value(time)).collect();
            let euler_q = if prop == clip::Prop::Euler {
                let order = clip::euler_order_of(s.binding.customType)?;
                Some(clip::euler_to_quaternion(
                    [direct[0], direct[1], direct[2]],
                    order,
                ))
            } else {
                None
            };
            println!(
                "{}",
                json!({"clip": cn, "prop": format!("{prop:?}"), "time": time, "unity_direct_components": direct, "unity_euler_quaternion": euler_q, "unity_channel_sample": clip::sample_channel(&ch, time), "keys": ch.times.len(), "custom_type": s.binding.customType})
            );
        }
        // And through the asset the exporter writes (glTF space)
        let a = prefab::load(
            &install,
            &b,
            prefab::Which::Main,
            &prefab::Options {
                textures: false,
                animations: true,
            },
        )?;
        if let Some(an) = a.animations.iter().find(|a| a.name == cn) {
            for ch in an.channels.iter().filter(|c| c.node == ni) {
                println!(
                    "{}",
                    json!({"asset_channel": format!("{:?}", ch.target), "interpolation": format!("{:?}", ch.interpolation), "time": time, "gltf_sample": ch.sample(time)})
                );
            }
        }
    }
    Ok(())
}

/// The family of a bundle: its name up to the first digit, without language and platform
/// suffixes (`dvl0x01` is `dvl`, `f045` is `f`, `common_steam_de` is `common`).
fn family(name: &str) -> String {
    const SUFFIXES: [&str; 13] = [
        "de", "en", "es", "fr", "it", "ja", "ko", "zh", "ps", "steam", "switch", "xbox", "xboxone",
    ];
    let kept: Vec<&str> = name.split('_').filter(|t| !SUFFIXES.contains(t)).collect();
    let joined = kept.join("_");
    let end = joined
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(joined.len());
    joined[..end].trim_end_matches('_').to_string()
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1048576.0)
}

fn file_size(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn list(game: &std::path::Path, what: Option<&str>, filter: Option<&str>) -> Result<()> {
    let install = Install::open(game)?;
    match what {
        None => {
            // One row per distinct name: the DLC folders repeat base game bundles (the base game's
            // file is the one counted), so "in DLC" is how many of a family are also found there
            let mut seen: std::collections::HashMap<String, (String, u64, bool)> =
                std::collections::HashMap::new();
            for (p, dlc) in install
                .base_bundles()
                .into_iter()
                .map(|p| (p, false))
                .chain(install.dlc_bundles().into_iter().map(|p| (p, true)))
            {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                let size = file_size(&p);
                let e = seen
                    .entry(name.clone())
                    .or_insert_with(|| (family(&name), size, false));
                e.2 |= dlc;
            }
            let mut fam: BTreeMap<String, (usize, u64, usize)> = BTreeMap::new();
            for (family, size, dlc) in seen.into_values() {
                let e = fam.entry(family).or_default();
                e.0 += 1;
                e.1 += size;
                e.2 += usize::from(dlc);
            }
            println!(
                "{} (base game {}, DLC folders {})",
                install.game.display(),
                install.pc.display(),
                install.dlc.len()
            );
            println!(
                "{:<26} {:>8} {:>6} {:>12}",
                "family", "bundles", "in DLC", "size"
            );
            for (k, (n, size, dlc)) in &fam {
                println!("{k:<26} {n:>8} {dlc:>6} {:>12}", mb(*size));
            }
        }
        Some(kind @ ("demons" | "fields")) => {
            let bundles = if kind == "demons" {
                install.demon_bundles()
            } else {
                install.field_bundles()
            };
            println!(
                "{:<12} {:>10}  {:>8}  largest prefab",
                "bundle", "size", "prefabs"
            );
            for p in bundles {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if filter.is_some_and(|f| !name.contains(f)) {
                    continue;
                }
                let b = Bundle::open(&p)?;
                let roots = prefab::prefab_roots(&b)?;
                let main = prefab::main_prefab(&b, &roots)
                    .ok()
                    .and_then(|i| roots.get(i));
                println!(
                    "{name:<12} {:>10}  {:>8}  {}",
                    mb(file_size(&p)),
                    roots.len(),
                    main.map_or("(no model)".to_string(), |(k, _)| k
                        .rsplit('/')
                        .next()
                        .unwrap_or(k)
                        .to_string())
                );
            }
        }
        Some(bundle) => list_bundle(&install, bundle)?,
    }
    Ok(())
}

/// The tag of a console-era Atlus file: three capital letters or digits and a fourth (or a zero
/// byte) at an offset of 0, 4, 8 or 12.
fn text_magic(data: &[u8]) -> String {
    for off in [0usize, 4, 8, 12] {
        if let Some(m) = data.get(off..off + 4) {
            let cap = |c: &u8| c.is_ascii_uppercase() || c.is_ascii_digit();
            if m[0].is_ascii_uppercase() && m[1..3].iter().all(cap) && (cap(&m[3]) || m[3] == 0) {
                let tag: String = m
                    .iter()
                    .map(|&c| {
                        if c == 0 {
                            "\\0".to_string()
                        } else {
                            (c as char).to_string()
                        }
                    })
                    .collect();
                return format!("{tag} at {off}");
            }
        }
    }
    match data.get(..4) {
        Some(h) => format!(
            "none (starts {:02x}{:02x}{:02x}{:02x})",
            h[0], h[1], h[2], h[3]
        ),
        None => "none (empty)".to_string(),
    }
}

fn list_bundle(install: &Install, bundle: &str) -> Result<()> {
    use nocturne_unpack::bundle::class;
    let path = install.resolve(bundle)?;
    let b = Bundle::open(&path)?;
    println!(
        "{} ({}, Unity {}, type trees: {})",
        path.display(),
        mb(file_size(&path)),
        b.unity_version(),
        b.has_type_tree()
    );
    for (entry, size) in b.entries() {
        println!("entry {entry}: {}", mb(size));
    }
    if let Some(&id) = b.objects_of(class::ASSET_BUNDLE).first() {
        let ab: AssetBundle = b.read(id)?;
        if !ab.m_Dependencies.is_empty() {
            println!("depends on: {}", ab.m_Dependencies.join(", "));
        }
        let prefabs: Vec<&str> = ab
            .m_Container
            .iter()
            .map(|(k, _)| k.as_str())
            .filter(|k| k.ends_with(".prefab"))
            .collect();
        if !prefabs.is_empty() {
            println!("prefabs:");
            for k in prefabs {
                println!("  {k}");
            }
        }
    }
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    for (_, c) in b.objects() {
        *classes.entry(Bundle::class_name(c)).or_default() += 1;
    }
    println!("objects:");
    for (k, v) in &classes {
        println!("  {k}: {v}");
    }
    let texts = b.objects_of(class::TEXT_ASSET);
    if !texts.is_empty() {
        println!("TextAssets (name, bytes, tag):");
        let mut rows = Vec::new();
        for id in texts {
            let t: TextAsset = b.read(id)?;
            rows.push((
                t.m_Name.clone(),
                t.m_Script.0.len(),
                text_magic(&t.m_Script.0),
            ));
        }
        rows.sort();
        for (name, len, magic) in rows {
            println!("  {name:<28} {len:>9}  {magic}");
        }
    }
    Ok(())
}

fn texture_cmd(
    game: &std::path::Path,
    bundle: &str,
    name: Option<&str>,
    out: Option<&std::path::Path>,
) -> Result<()> {
    use nocturne_unpack::{bundle::class, texture};
    let install = Install::open(game)?;
    let b = Bundle::open(&install.resolve(bundle)?)?;
    for id in b.objects_of(class::TEXTURE_2D) {
        let t: Texture2D = b.read(id)?;
        match name {
            None => println!(
                "{:<28} {}x{} {} mips {} {}",
                t.m_Name,
                t.m_Width,
                t.m_Height,
                texture::format_name(t.m_TextureFormat),
                t.m_MipCount,
                if t.m_StreamData.size > 0 {
                    "streamed"
                } else {
                    "inline"
                }
            ),
            Some(n) if n == t.m_Name => {
                let img = texture::decode(&b, &t)?;
                let out = out.context("--out is needed to write the texture")?;
                texture::write_png(out, &img)?;
                println!(
                    "{} {}x{} {} -> {}",
                    t.m_Name,
                    img.width,
                    img.height,
                    texture::format_name(t.m_TextureFormat),
                    out.display()
                );
                return Ok(());
            }
            Some(_) => {}
        }
    }
    if let Some(n) = name {
        anyhow::bail!("no texture named {n:?} in {bundle}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_of(args: &[&str]) -> PathBuf {
        Cli::try_parse_from(args).unwrap().game
    }

    #[test]
    fn game_goes_before_or_after_the_command() {
        let want = PathBuf::from("/games/smt");
        for args in [
            &["nocturne-unpack", "--game", "/games/smt", "check"][..],
            &["nocturne-unpack", "check", "--game", "/games/smt"][..],
            &["nocturne-unpack", "list", "demons", "--game", "/games/smt"][..],
            &[
                "nocturne-unpack",
                "dump",
                "dvl0x01",
                "74",
                "--tree",
                "--game",
                "/games/smt",
            ][..],
            &["nocturne-unpack", "--game=/games/smt", "census", "--decode"][..],
        ] {
            assert_eq!(game_of(args), want, "{args:?}");
        }
    }

    #[cfg(feature = "bridge")]
    #[test]
    fn check_conformance_keeps_its_own_options() {
        // Everything after the command is the checker's, but --game still reaches the global
        for args in [
            &[
                "nocturne-unpack",
                "--game",
                "/g",
                "check-conformance",
                "--kind",
                "level",
            ][..],
            &[
                "nocturne-unpack",
                "check-conformance",
                "--kind",
                "level",
                "--game",
                "/g",
            ][..],
            &[
                "nocturne-unpack",
                "check-conformance",
                "--game",
                "/g",
                "--kind",
                "level",
            ][..],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let Command::CheckConformance { args: rest } = cli.command else {
                panic!("{args:?}")
            };
            // The checker's own options reach it, and the folder is found by the global option or
            // by the manual scan of `rest`
            assert!(rest.iter().any(|a| a == "--kind") && rest.iter().any(|a| a == "level"));
            assert!(
                cli.game.as_path() == std::path::Path::new("/g") || rest.iter().any(|a| a == "/g")
            );
        }
    }
}
