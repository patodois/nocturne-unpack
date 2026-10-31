# nocturne-unpack

[![rewrite-bridge on crates.io](https://img.shields.io/crates/v/rewrite-bridge.svg?label=rewrite-bridge)](https://crates.io/crates/rewrite-bridge)
[![rewrite-bridge docs](https://img.shields.io/docsrs/rewrite-bridge?label=docs.rs)](https://docs.rs/rewrite-bridge)

A small Rust crate and command line tool that reads the game data of Shin Megami Tensei III Nocturne HD Remaster
(the Steam build for Windows) from your own install:

- the Unity asset bundles in `smt3hd_Data/StreamingAssets/PC` and in the `dlc/<n>` folders (UnityFS, Unity 2019.4,
  every bundle carries its own type trees), with the `.resS` streams they point to
- demons (`dvl0x..`): the prefab's transform hierarchy, rigid and skinned meshes with bone weights, textures, materials
  and the clips of the demon's animator, exported as a skinned, textured glTF 2.0 binary (`.glb`) with all its animations
- fields (`f...`): the Unity part of a field, meaning the prefabs with their meshes, materials, textures and transforms
- the console-era Atlus files that fields carry as TextAssets (`.TB`, `.LB`, `.f1`, `.f2`, `.bf`, ...) are only listed, with
  their tags and sizes

This project contains no game code or game data, and nothing extracted from the game is committed: no textures, meshes,
dumps, screenshots or fixtures. It only reads files from a copy of the game you already have installed, so you need your
own copy. Tests that need the game skip themselves when it is not there.

## Download

Programs built for each release are on the [Releases page](https://github.com/patodois/nocturne-unpack/releases). Pick the archive for your system:

| System | Archive |
|--------|---------|
| macOS, Apple silicon | `nocturne-unpack-<tag>-macos-arm64.tar.gz` |
| macOS, Intel | `nocturne-unpack-<tag>-macos-x86_64.tar.gz` |
| Linux, x86_64 (glibc 2.35 or newer) | `nocturne-unpack-<tag>-linux-x86_64.tar.gz` |
| Windows, x86_64 | `nocturne-unpack-<tag>-windows-x86_64.zip` |

Each archive holds the program, this README, both license files and an `INSTALL.txt`. `SHA256SUMS` on the release page has
the checksum of every archive (`shasum -a 256 -c SHA256SUMS` on macOS, `sha256sum -c SHA256SUMS` on Linux,
`Get-FileHash` on Windows). They are built with the `bridge` feature, so the rewrite-bridge provider is included.

`SHA256SUMS.sig` on the same release signs `SHA256SUMS` with the project's Ed25519 release key, so you can also check
that the list itself came from here. With the downloads in one folder (needs OpenSSL 3):

```sh
sh tools/verify-release.sh nocturne-unpack v0.2.0 path/to/folder
```

The signed text is `osp release patodois/nocturne-unpack <tag>`, a newline, then `SHA256SUMS`, so a signature from another release
does not verify. The public key is in `RELEASE-KEY.pem` (SHA-256 of its DER form
`26c35ed9930be42fd266852135e2e81eb74c36ed9a2d0c527e86210a3dcb0946`), and the script carries the same key.

**The binaries contain no game data.** They are the program only; it reads the files of a copy of Shin Megami Tensei III Nocturne HD Remaster that you already have
installed.

They are not code-signed, so the first run may be blocked:

- **macOS (Gatekeeper)**: a file downloaded in a browser is quarantined, and macOS refuses to open it ("Apple could not
  verify ..."). Remove the mark once, from the folder you extracted to: `xattr -d com.apple.quarantine ./nocturne-unpack`. Or open
  System Settings > Privacy & Security and press "Open Anyway" after the first refusal.
- **Windows (SmartScreen)**: "Windows protected your PC" appears the first time. Choose "More info", then "Run anyway". Or
  right-click the downloaded `.zip`, open Properties and tick "Unblock" before you extract it.

To build it yourself instead, see [Build](#build).

## What you need

- **Your own copy of the game: the Steam build for Windows** (Steam app 1413480). The files are the same on every
  system, so this tool also runs on macOS and Linux, but Steam only installs the game on Windows. To get the files on
  a Mac or Linux machine, use [SteamCMD](https://developer.valvesoftware.com/wiki/SteamCMD), Valve's command line
  Steam client, and ask it for the Windows depot (you log in yourself; this tool never asks for a password):

  ```sh
  steamcmd +@sSteamCmdForcePlatformType windows +force_install_dir ~/Games/smt3hd \
           +login <your steam user> +app_update 1413480 validate +quit
  ```

  Steam asks for your password and Steam Guard code there. The account must own the game. Installing it through
  Steam under CrossOver or Proton also gives you the folder; anything that does works.
- **The game folder** is the one that holds `smt3hd_Data` (and `smt3hd.exe`). Where it is depends on how you got it:

  | how you installed | game folder |
  |---|---|
  | Steam on Windows | your Steam library's `steamapps\common\<the game's folder>` (in Steam: right click the game, Manage, Browse local files) |
  | SteamCMD, as above | the `+force_install_dir` you gave |
  | this tool's default | `~/Games/smt3hd` (the author's layout: any folder works) |

  Point the tool at it with `--game <folder>` on a command, or once for the whole shell with
  `export NOCTURNE_GAME_DIR=<folder>`; `--game` wins over the variable, which wins over the default. A leading `~/`
  is expanded. `--game` also accepts the `smt3hd_Data/StreamingAssets/PC` folder itself.
- **Rust**, the stable toolchain from [rustup.rs](https://rustup.rs) (edition 2021; 1.88 or newer, which is what
  [bcn-decode](https://crates.io/crates/bcn-decode) asks for; tested with 1.88.0 and 1.99). Every dependency comes from
  crates.io, [unity-bundle](#container-layer) and (with the `bridge` feature)
  [rewrite-bridge](https://crates.io/crates/rewrite-bridge) included, so `git` is not needed to build.
  A clean release build takes about 11 seconds on an M-series Mac with 4 jobs (`cargo build --release -j 4`), and the
  `bridge` feature adds about 5 more.
- **Disk and memory.** Nothing is extracted or copied; the game folder is about 12 GB. Each command opens one bundle at
  a time. A 16 GB machine is enough for everything below; the heaviest sweep needs about 2 GB (numbers next to
  [Usage](#usage)).

## Words used here

- **Unity, asset bundle**: the game is a Unity game. Its models, textures, animations and sounds are stored in *asset
  bundles* (UnityFS files, Unity 2019.4), one file per bundle and with no file extension. A bundle carries its own *type
  trees*, the field layout of every object it holds, which is how this tool reads them with nothing hardcoded.
- **`.resS`**: a stream next to a bundle's data, where big payloads (texture pixels) live. It is read in ranges.
- **Object, class id**: a bundle is a list of objects, each of a Unity class with a number
  ([see below](#unity-class-ids)): `GameObject`, `Transform`, `Mesh`, `Material`, `Texture2D`, `AnimationClip` ...
- **Prefab, GameObject, Transform**: a prefab is a saved scene object: a tree of GameObjects, each with a Transform
  (position, rotation, scale), and components such as a mesh and its renderer.
- **Skinned mesh, bone weights**: a mesh deformed by a skeleton: every vertex says which bones move it and how much.
  A *rigid* mesh is one piece fixed to a bone.
- **Animator, clip**: a clip (`AnimationClip`) is one animation made of curves over time; the demon's Animator lists
  its clips.
- **Demon** (`dvl0x01`, ...): a creature of the game, one bundle each. **Field** (`f012`, ...): a place you walk through,
  made of several prefabs (one per block of the map). **DLC folders**: `dlc/<n>` next to the base game; see
  [the DLC folders](#the-dlc-folders-repeat-the-base-game).
- **TextAsset**: a file stored raw. The fields carry console-era Atlus files this way; they are only listed.
- **glTF, `.glb`**: the open 3D format the exporter writes (binary form); Blender and every viewer open it.
- **[rewrite-bridge](https://github.com/patodois/rewrite-bridge)**: a small library with common model types, so rewrites
  of different games can load each other's assets into one scene. A **Provider** is a game's reader that implements it;
  this crate has one (feature `bridge`). To write one for another game, see
  [docs/WRITING_A_PROVIDER.md](https://github.com/patodois/rewrite-bridge/blob/main/docs/WRITING_A_PROVIDER.md).
- **Other providers**: [wf-unpack](https://github.com/patodois/wf-unpack) (Warframe),
  [wt-unpack](https://github.com/patodois/wt-unpack) (War Thunder) and
  [hytale-unpack](https://github.com/patodois/hytale-unpack) (Hytale) read their games the same way and return the same
  types, so a demon from here can stand next to their characters.
- **[bridge-testbed](https://github.com/patodois/bridge-testbed)**: a [Bevy](https://bevy.org) app that loads the assets
  of every provider side by side, to check scale, orientation and animation across games by eye. It needs your own game
  installs and no game data is in it.
- **Conformance checker**: the bridge's test that a provider's output has the right units, orientation and winding;
  see [Conformance](#conformance).

## Build

```sh
cargo build --release
cargo build --release --features bridge     # adds the rewrite-bridge provider, check-bridge and check-conformance

export PATH=$PWD/target/release:$PATH       # so `nocturne-unpack` works from anywhere (this shell only);
                                            # or write `cargo run --release --` in front of each command below
```

## Usage

`--game` is the game folder (the one that holds `smt3hd_Data`), written before or after the command; it defaults to
`$NOCTURNE_GAME_DIR`, else `~/Games/smt3hd`, and also accepts the `StreamingAssets/PC` folder itself. Bundles are named as in that folder
(`dvl0x01`) or given as a path. Bundles have no file extension, and the file called `Pc` in that folder is Unity's
AssetBundleManifest, not a bundle of assets.

```sh
export NOCTURNE_GAME_DIR=~/Games/smt3hd     # or add --game ~/Games/smt3hd to each command

# Families of bundles with counts and sizes; the demons or fields; or what one bundle holds
nocturne-unpack list
nocturne-unpack list demons --filter dvl0x0
nocturne-unpack list fields
nocturne-unpack list f012                    # objects, prefabs, TextAssets with their tags and sizes

# A demon as a skinned, textured .glb with every clip of its animator; --prefab picks another
# prefab of the bundle, --no-textures and --no-animations leave those out
nocturne-unpack export dvl0x01 dvl0x01.glb
# A field prefab (a field is made of several, one per block, each in its own frame) or all of them
nocturne-unpack export f012 f012_002.glb --prefab f012_002
nocturne-unpack export f012 f012_all.glb --all-prefabs

# Load every demon bundle, count failures, report the peak memory; --kind fields does the same
# for the field bundles (all prefabs of a bundle at once)
nocturne-unpack check
nocturne-unpack check --kind fields --no-textures

# One texture of a bundle as PNG (the largest level), or the textures of a bundle with their formats
nocturne-unpack texture dvl0x01
nocturne-unpack texture dvl0x01 0x01_t05 -o t05.png

# Through the rewrite-bridge provider (build with --features bridge): load and validate every demon (with
# its clips) and every field prefab (as a model and as a scene); --textures builds the materials with their
# textures too
nocturne-unpack check-bridge
nocturne-unpack check-bridge --kind demons --textures

# The rewrite-bridge conformance checker on what the provider returns (options as in its README, plus
# --game and --no-textures); --kind character for the demons, --kind level for field prefabs.
# Exit status 1 means an asset has an error, which is the case for some field prefabs (see Conformance);
# --allow CODE leaves a finding out
nocturne-unpack check-conformance --kind character --compare-load
nocturne-unpack check-conformance --id f012/f012_002 -v

# Object classes and texture formats across all bundles; --decode also decodes every texture and mesh
nocturne-unpack census --decode

# Exploring: one object as JSON (with its type tree), clip diagnostics, numbers for cross-checks.
# The tree of a clip is about 50,000 lines; the command ends quietly when `head` stops reading
nocturne-unpack dump dvl0x01 74 --nth 0 --tree | head -40     # 74 is the Unity class id of AnimationClip
nocturne-unpack anim dvl0x07
nocturne-unpack probe dvl0x01 --mesh "0x01 body" --node "p Pelvis/b Spine" --clip b00 --time 0.5
```

`-v` names every material property that is not carried over instead of counting them.

**Time and memory** (release build, M-series Mac, 16 GB, 23 October 2026; "peak" is the peak resident size; the first
run after a restart is slower while the game files are read from disk): `list` under 0.1 s; `list fields` 1 s and 410 MB
(it opens every field bundle); `export` of one demon 0.2 s and 90 MB, of one field prefab 0.4 s and 120 MB; `check` 2 s
and 350 MB; `check --kind fields --no-textures` 3 s and 520 MB; `census --decode` 17 s and 850 MB;
`check-bridge --kind demons --textures` 7 s and 530 MB; `check-bridge --kind fields` 33 s and 480 MB (with `--textures`
the largest fields need more than 1 GB); `check-conformance --kind character` 4 s and 480 MB (6 s with
`--compare-load`); `check-conformance --kind level` 28 s and 2.0 GB (the one heavy command: it keeps a whole scene at a
time).

Every bundle is opened on its own: the file is memory-mapped, its objects are parsed into memory and the `.resS`
streams are read in ranges, decompressing only the compression blocks a request touches. The sweeps (`check`,
`census`) never hold more than one bundle.

The tests in `tests/` run against a real install when `NOCTURNE_GAME_DIR` points at the game folder, and skip
themselves without it:

```sh
NOCTURNE_GAME_DIR=~/Games/smt3hd cargo test --release --features bridge
```

### Unity class ids

`dump <bundle> <class>` takes the number of a Unity class, as Unity's own class ID reference lists them (the tool
only knows the ones it reads; `list <bundle>` prints how many objects of each class a bundle has):

| id | class | what it is here |
|---:|---|---|
| 1 | `GameObject` | a node of a prefab's tree, with a name |
| 4 | `Transform` | a node's position, rotation and scale |
| 21 | `Material` | a surface: shader, colours, textures |
| 23 | `MeshRenderer` | draws a rigid mesh |
| 28 | `Texture2D` | an image, usually streamed from the `.resS` |
| 33 | `MeshFilter` | links a node to its `Mesh` |
| 43 | `Mesh` | vertices and triangles |
| 48 | `Shader` | a shader (the game's own are in the `common` bundle) |
| 49 | `TextAsset` | a raw file, such as the console-era Atlus files |
| **74** | `AnimationClip` | **one animation: curves of positions, rotations, scales over time** |
| 83 | `AudioClip` | a sound (not read) |
| 90 | `Avatar` | the skeleton's path table of an Animator |
| 91 | `AnimatorController` | the Animator's list of clips and states |
| 95 | `Animator` | puts a controller on a node |
| 137 | `SkinnedMeshRenderer` | draws a skinned mesh with its bones |
| 142 | `AssetBundle` | the bundle's own table of contents: asset paths and dependencies |

So `dump dvl0x01 74 --nth 0 --tree` prints the first animation clip of demon 1 and the type tree it was read with
(`--nth` counts objects of that class, from 0). That is a lot of lines (the clip's samples); pipe it to `head`, or to a
file.

### Where names come from

The tool makes no names up. Bundle names are the file names in the game folder (`dvl0x01` is the game's own number for a demon, `0x01`; `f012` is a field). The prefab path comes from the bundle's table of contents
(`.../devil_0x01/devil_0x01.prefab`, shown by `list dvl0x01`). Names of meshes (`0x01 body`), nodes
(`p Pelvis/b Spine`, a path below the Animator), clips (`b00`, `b04`), materials, textures and TextAssets are the
`m_Name` stored in the objects themselves, as the game's artists named them. A demon's in-game name is not among
them; if the game keeps it next to the model, it is in the data table (`d0x01.PB`), which is not read.

### The DLC folders repeat the base game

`dlc/<n>` holds copies of bundles that are already in the base game (the same file names, byte for byte for most, a
slightly different rebuild for a few), not new ones: 3,087 files are 2,250 distinct names. The same name in two places is
one bundle, and the base game's file is the one used. `list`, `check` and the provider list each name once; before
this, `list fields` showed `f001m` twice, because it is in the base game and in `dlc/2`. `census` still reads every
file, so it counts the copies.

## Container layer

The bundles are read with [unity-bundle](https://crates.io/crates/unity-bundle) 0.1.0 ([source](https://github.com/patodois/unity-bundle);
ours, MIT OR Apache-2.0), written for these files: UnityFS bundles (container format version 6), serialized files of
format versions 17 and 21 (the game ships both: 29 bundles were written by Unity 2018.4.13f1, the rest by 2019.4), LZ4
blocks, ranged reads of the `.resS` streams (only the compression blocks that cover a range are decompressed), and a serde
deserializer that reads objects through the type trees stored in the bundles. Earlier versions of this tool (0.1.x)
read the bundles with [rabex](https://github.com/jakobhellermann/RustyAssetBundleEXtractor) through a fork of ours with
three small changes (`NOTES-upstream.md` keeps their story). unity-bundle follows rabex's design as its reference and
was compared with that fork as a black box on every bundle of the game: identical decompressed blocks, type trees and
parsed values (see its README). Nothing has to be added to your `Cargo.toml` for it, and there is no
git dependency and no `[patch]` section.

With that, all 3,087 bundle files (2,250 in the base game, 837 in the DLC folders, which repeat base names) open, and every object decodes through the
type tree stored in its own bundle. No field layout is hardcoded for the Unity version.

## What a demon holds

A `dvl0x..` bundle holds one prefab (four also hold a second one: an afterimage, an older or an alternate version; the one
with the most renderers is the default) and everything it uses:

- the GameObject / Transform hierarchy, with rigid meshes (`MeshFilter` + `MeshRenderer`) on many bones and a few
  `SkinnedMeshRenderer`s
- `Mesh` objects, `Material`s, `Texture2D`s (streamed from the `.resS`), an `AnimatorController` whose clips are
  Mecanim generic clips (not legacy, not humanoid), and `Avatar`s
- sounds and a `.pb.bytes` data table, which are not read

25 of the 257 demon bundles have no model at all, only the data table and sounds.

### Conversion

- x is negated to turn Unity's left-handed space into glTF's right-handed one (positions, normals, tangents, bind poses,
  rotations as `x, -y, -z, w`, and the winding of triangles), so demons face +z. Units stay Unity's, taken as metres.
  UVs are flipped to start at the top of the image, which is also how the textures are written.
- Vertex channels are read with the stream layout the mesh states (stream offsets rounded to 16 bytes, channel formats
  and dimensions from `m_VertexData`): position, normal, tangent, colour, two UV sets, blend weights and indices. A mesh
  without a weight channel but with indices has weight 1 on its bone.
- Sub-meshes become one glTF primitive each, with its own vertices. A skinned renderer becomes a skin: joints are its
  bones, inverse bind matrices are the mesh's bind poses.
- Textures: DXT1, DXT5, BC7, RGB24, RGBA32 and Alpha8, the largest level only.
- Materials map what is direct. For `Standard`: `_MainTex`, `_Color` (converted to linear), `_Metallic`, `_Glossiness`,
  the alpha mode from the keywords `_ALPHATEST_ON` (with `_Cutoff`), `_ALPHABLEND_ON` and `_ALPHAPREMULTIPLY_ON`. For the
  `Atlus/...` shaders, only `_MainTex` and the alpha mode; metallic 0 and roughness 1 are the neutral choice, not game
  data. The texture's own filter and wrap modes become the glTF sampler, and a material's tiling and offset become
  `KHR_texture_transform`.
- Animation. A clip's curves come in three groups, streamed, dense and constant. Every curve is read as keys with the cubic
  polynomial of the segment that starts there, and a binding's path CRC-32 is matched to the paths of the nodes below the
  Animator (all 67 entries of an `Avatar` table of `dvl0x01` match that way). Position, scale and quaternion rotation
  channels are exact: cubic segments become glTF `CUBICSPLINE` keys, which holds the same Hermite segment, straight
  segments `LINEAR`, holds `STEP`. A jump inside a smooth curve is written as a key one float step before the jump.
  Rotation keys are normalised, as glTF requires. Keys outside the clip's start and stop are cut to its range.

## rewrite-bridge

With the `bridge` feature, `src/bridge.rs` implements [rewrite-bridge](https://github.com/patodois/rewrite-bridge)'s
`Provider` (rewrite-bridge `0.2`, from [crates.io](https://crates.io/crates/rewrite-bridge)), like wf-unpack's.

- `list` returns every demon (id is the bundle name, `dvl0x01`, kind Character) and every field prefab that has
  something to draw (`f012/f012_002`, kind Level); prefabs of only transforms are left out.
- `load_animated` returns a validated demon with its clips as `Animation`s and `Channel`s on the model's nodes; `load`
  is the same model. For a field prefab, `load` gives its static model.
- `load_scene` (field prefabs) returns a scene: one inline model per distinct mesh with its materials, placed by an
  instance with the node's world transform. The game data has almost no instancing (about 66,984 placements of 66,425 models).
  A world matrix with shear cannot be a translation, rotation and scale; it is counted and the closest transform kept.
- The bridge has no place for texture samplers and tiling, vertex colours or a second UV set. Lights and cameras are not
  read.

To use the provider from another crate, depend on both. Every crate in one build has to end up with the same copy of
rewrite-bridge, or `Nocturne` implements a `Provider` that your code's `Provider` is not, and the compiler reports
"multiple different versions of crate `rewrite_bridge`". From crates.io Cargo does this for you: every `0.2`
requirement (this crate asks for `0.2.1`, your `"0.2"` accepts it) resolves to one 0.2.x release and is built once.
A requirement on another minor line (`"0.3"`) would be a second copy, so stay on the same `0.x` as this crate.
`cargo tree -i rewrite-bridge` must show a single copy. No `[patch]` section is needed.

```toml
[dependencies]
nocturne-unpack = { version = "0.2", features = ["bridge"] }
rewrite-bridge = "0.2"
```

```rust,no_run
use rewrite_bridge::Provider;

fn main() -> Result<(), rewrite_bridge::BoxError> {
    // The game folder: a leading ~/ is expanded; a &str, a String or a Path all do
    let nocturne = nocturne_unpack::bridge::Nocturne::open("~/Games/smt3hd")?;
    let demon = nocturne.load_animated("dvl0x01")?;
    println!("{} nodes, {} clips", demon.model.nodes.len(), demon.animations.len());
    Ok(())
}
```

This block is compiled by `cargo test --features bridge`, so it cannot drift from the API.

## Conformance

`check-conformance` runs rewrite-bridge's conformance checker (`rewrite_bridge::conformance`) over the provider's output.
Over all 232 demons (`--kind character --compare-load`, 6 s) it reports no errors. Over all 1,039 field prefabs
(`--kind level`, 28 s, 2.0 GB) it reports two error codes, so that command exits with status 1:

- `geometry.winding_flipped`: 287 meshes in 37 field prefabs have 54 to 100 % of their triangles wound against their own
  stored normals (names such as `room*`, `kabeL*`, `Plane*` and `md_*`). They are the game's data, not the conversion:
  for every primitive of `f003`, `f004` and `f023` the count of triangles with and against their normals is the same in the
  Unity mesh and in the converted one (a test; it fails if the reversed indices are taken out), and the 232 demons give no such
  finding. The 287 split by their material: 237 have the keyword `LIGHTOFF_USE` (their material switches the
  lighting off), 48 are the lit meshes `room10`, `room27` and `room44` of `f023` and `f023m`, one is a one-triangle mesh (`aa_0` in `f662`), and one has `_Cull` 0, two-sided (`md_hoji_01_01`
  in `f003`). The provider does not guess which side is meant to face out.
- `geometry.non_finite`: 7 meshes of `f004/f004_001` (`md_01all_smoke_new*`) have NaN normals in 2 vertices each. The file
  stores them as 32-bit floats with those bit patterns (a test reads the bytes itself), next to positions of 1e11 to 1e12 and
  an instance scale of 1.5e-12 (`scene.instance_scale_extreme`): the smoke is stored huge and scaled down.

**Exit status.** It is the checker's, the same as in `hytale-unpack` and `mc-unpack`, which pass their arguments to the same
function: 0 when no asset has an error, 1 when one does (a warning only with `--strict`), 2 for a usage error. So
`--kind level` exits 1 because of the two codes above, which are the game's data. `--allow CODE` (repeatable) leaves a code out;
with `--allow geometry.winding_flipped --allow geometry.non_finite` the 1,039 field prefabs exit 0 (the warnings below remain).

What it reports besides is the game's data, not the provider, and stays as it is (`-v` lists it). Counts are for the
demons unless a field prefab is named:

- `skin.bind_pose_mismatch` (info, 212 demons): Unity's bind poses are the bone's world-to-local matrix times the
  *renderer's* local-to-world one, so the node pose is not the inverse of the inverse bind matrices when the
  skinned mesh's node is not at the origin. glTF's skinning gives the same result as Unity's at rest, and the
  renders at rest are undeformed.
- `orientation.feet_off_ground` (75 demons), `scale.height_out_of_range` (15: nine to 25 m tall, and one 0.1 m object):
  where a demon's origin is and how big it is are the prefab's; the game places and scales them elsewhere. A demon
  such as `dvl0x0e` stays about 1.8 m above the origin in every frame checked of its clips `b00` and `b12`.
- `front.not_plus_z` (46 demons): the checker compares named points (`eye`, `tail`) with the middle of the bounding
  box. For some demons that is right (`dvl0x0e` faces -X at rest, as its renders show), for others it is not (`dvl0x0c`
  and `dvl0x10` face +Z; their limbs and weapons reach forward, and `h_eye` is an effect anchor, not an eye).
  The data has no front axis, so none is guessed.
- `material.blend_opaque_texture` (34 demons, 792 field prefabs): these are additive materials (`_DstBlend` 1) and
  materials whose alpha comes from vertex colours (not exported). glTF has no additive mode, so they come out as `BLEND`.
  `material.opaque_transparent_texture` (22 demons, 294 field prefabs): the alpha channel of an opaque material's texture is data for the Atlus
  shader, not transparency (render queue 2000, blend factors one and zero). Alpha modes follow Unity's keywords, and
  in every Atlus material of the demons they agree with `_DstBlend` and the render queue (blend keywords go with
  factor 1 or 10 and queue 3000, alpha test with queue 2450, the rest is opaque).
- `orientation.not_upright` (6 demons): snakes and dragons, 7 m long and 1 m high. `animation.zero_scale` (`dvl0xc0d`):
  the node `ebony` is hidden by scaling it to zero. `animation.pose_extent_mismatch` (4): a part that moves 9 m away from
  the body during a clip (`dvl0x0e` `b12`), a skeleton that retracts to a fifth of its size (`dvl0x95` `b01`). `animation.duplicate_name` (4 demons) / `animation.empty`
  (`dvl0x110`, `dvl0x40`, `dvl0x73`): the animator lists two clips with the same name (in `dvl0x110` the second has 119
  bindings against 171) and one of them resolves to no channel. `scene.instance_scale_extreme` (16 field prefabs, such as `f676_010`): an object scaled to 8e-6 to hide it.
  Field prefabs also give `scale.too_large` (14 prefabs, up to kilometres: skies and floors), `scale.too_small` (2) and
  `geometry.degenerate_triangles` (154).

## Verification

Tested against the Steam (Windows) build found in `~/Games/smt3hd` on 2026-10-23 (Unity 2019.4.9f1 and 2019.4.10f1; 29
bundles are 2018.4.13f1).

All numbers are from one run on a 16 GB machine, one bundle open at a time; "peak" is the peak resident size.

- `census --decode`: all 3,087 bundle files (2,250 in the base game, 837 in the DLC folders, which repeat base names; the
  `Pc` manifest is not counted) open, every one with type trees, none failing; 15,930 `Texture2D` (Alpha8 10, DXT1
  10,742, DXT5 3,455, BC7 41, RGB24 1,179, RGBA32 503) and all 112,166 `Mesh` objects decode without a failure (peak
  850 MB, 17 s). The classes of its 1,468,690 objects include 435,455
  GameObject, 408,793 Transform, 119,629 MeshRenderer, 3,749 SkinnedMeshRenderer, 35,323 Material, 12,552 AudioClip,
  9,594 AnimationClip, 8,568 TextAsset and 406 Shader; `nocturne-unpack census` prints them all.
- `check`: 257 demon bundles: 232 load, 25 hold no model (a data table and sounds only), 0 failures. 532,268 triangles,
  529,738 vertices, 4,513 primitives, 327 skins, 2,808 materials, 2,017 textures and 1,733 clips with 246,607 channels
  decode in 2.1 s with a peak of 350 MB. The materials use `Atlus/Atlus_CHAR` (2,786), `Atlus/AtlusMat` (10),
  `Atlus/Atlus_CHAR2` (3) and `Standard` (9); 116 have a tiled or offset `_MainTex`.
- `check-bridge`: 232 of 232 demons load and validate as `AnimatedModel`s with textures (7 s, peak 530 MB), and all
  1,039 listed field prefabs (of 1,066 in the 182 field bundles; the others have nothing to draw) load as a static model
  and as a validated scene (33 s, peak 480 MB without textures): 99,774 meshes, 14.2 million vertices, 66,425 scene
  models placed by 66,984 instances.
- **Cross-checks** against UnityPy 1.25.4, which reads the same bundles independently, for the vertex count, first vertex
  (position, normal, UV, bone weights and indices), first indices and bind pose of a skinned mesh, a bone's local
  transform, the path CRC-32, and a clip's sampled position and rotation (UnityPy has no sampler, so the oracle
  evaluates the raw streamed keys itself, in double precision): on `dvl0x01`, `dvl0x95` and `dvl0x07` every integer and
  every mesh and transform value is identical; the largest difference in a sampled clip value is 2.9e-8 (a float
  against a double). On `dvl0x07`, where the clip is made of Euler curves, the written channel is 0.003 and 0.001 degrees
  from the true curve at two times between keys. Textures of all six formats are pixel-identical to Pillow's independent
  decoders (DXT1, DXT5, BC7) and to the raw data (RGB24, RGBA32, Alpha8).
- bcn-decode 0.1.0 from crates.io (25 October 2026, replacing texture2ddecoder): all 15,930 `Texture2D` of the base game
  and the DLC folders decode to the same RGBA pixels as before, and the 232 demon exports and 3 field exports are
  byte-identical; `check`, `check --kind fields`, `check-bridge` and `check-conformance` print the same.
- unity-bundle 0.1.0 from crates.io (25 October 2026, replacing the rabex fork; version 0.2.0 of this tool): against a
  build of 0.1.1 (rabex), the RGBA pixels of all 15,930 `Texture2D` and the decoded data of all 112,166 `Mesh` objects
  hash the same, the `.glb` files of the 232 demons with a model and of the 162 field bundles with a prefab to draw
  (`--all-prefabs`) are byte-identical, and the exports of the other 45 bundles fail in both with the same status; `list`,
  `check`, `check --kind fields`, `check-bridge`, `check-conformance` (demons with `--compare-load`, and fields) and
  `census --decode` print the same except for times and peak memory (the largest, `check-conformance --kind level`, peaks
  at 2.0 GB in both). The tests pass with Rust 1.88.0 as well as 1.99.
- Of the 234 demon files exported before the duplicate names were folded into 232 (not repeated since), every one passes the Khronos glTF validator with no errors (warnings only: a skinned mesh
  below the root, a clip that also animates the node that carries a skinned mesh). A sample of 44 field prefabs passes too.
- Renders, made with three.js in headless Chromium: demons in the bind pose and in the middle of a clip, and a field.
  They are not in the repository.
- `cargo test --release --features bridge` with `NOCTURNE_GAME_DIR` set: 51 unit tests, 11 tests against the install (three
  of them run the command line), 8 against the install through the bridge (6 provider, 2 conformance) and 2 doctests (this
  file's Rust code and `Nocturne::open`); without the variable only the unit tests and doctests run. `cargo clippy --all-targets -- -D warnings` is clean,
  with and without the `bridge` feature.

## Format notes

What this version learned or confirmed from the data; the module docs in `src/clip.rs`, `src/mesh.rs`, `src/texture.rs`
and `src/prefab.rs` describe the layouts.

- **Bundles.** UnityFS version 6 written by Unity 2019.4.9f1 and 2019.4.10f1 (29 bundles are 2018.4.13f1), LZ4HC blocks,
  every bundle with type trees. The header is followed directly by the compressed block info (flags `0x43`: LZ4HC,
  blocks and directory combined); there is no padding to 16 bytes. A bundle's serialized file is named
  `CAB-<hash>`, and streamed data is in `CAB-<hash>.resS` and `.resource` entries that `m_StreamData` points into
  (`archive:/CAB-.../CAB-....resS`, offset, size).
- **References between bundles.** `m_FileID` 1 in a demon or field points into `common`, the bundle that holds the
  shared shaders (`Atlus/Atlus_CHAR`, `Atlus/AtlusMat`, ...) and the toon ramp texture; the `externals` list of the
  serialized file gives its `CAB-...` name and the `AssetBundle` object lists the dependency bundles by name. The
  `Standard` shader inside each demon bundle is used by 9 materials of all the demons' prefabs; the other materials use the Atlus shaders.
- **Prefabs.** The `AssetBundle` object's `m_Container` lists the assets by path; `.prefab` entries give the GameObject
  to start from. A demon bundle also contains the GameObjects of its FBX imports (`devil_0x01@b00` and so on) with
  their own Animators, Avatars and duplicate clips, which the prefab does not use. The Animator may sit on a child of
  the prefab root, and the clips address nodes by path below it.
- **Path hashes.** A generic binding's `path` is the CRC-32 (zlib polynomial) of the node's path below the Animator's
  object, names joined with `/`, the Animator's own object being the empty path. Every entry of every `Avatar`'s
  `m_TOS` table of `dvl0x01` matches that way.
- **Streamed clips.** Frames of `time, key count, keys`, a key being `curve index, a, b, c, d` with the segment's
  polynomial `a s^3 + b s^2 + c s + d`. The first frame's time is the lowest finite float (not minus infinity) and
  holds each curve's value before its first key, the last frame has time +infinity and no keys. Curves end with a key
  whose polynomial is flat. Across all demon clips every smooth segment of 0.1 ms or more ends within 1e-3 of the next
  key's value, which is how the coefficient order was confirmed.
- **Generic bindings.** Transform bindings own 3 curves (position, scale, Euler) or 4 (quaternion); any other binding owns
  one. For Euler rotations `customType` is Unity's rotation order (`0` XYZ ... `4` ZXY, the first axis turning first):
  the pelvis of `dvl0x07` is within 13 degrees of its rest rotation with order 0 and 113 degrees away with ZXY, and the
  clips look right only with it.
- **Meshes.** Vertex streams start 16-byte aligned; `dimension` carries the component count in its low four bits (a
  half-float normal channel has dimension `0x34`); bone indices are `UInt32` and weights `Float`, two or four per
  vertex, or indices alone for a single bone. Index buffers are read as 16-bit (32-bit is supported), sub-meshes are triangle lists, and no mesh uses mesh compression.
- **Quaternion curves.** In the 94,806 rotation channels of the demons no two successive keys lie in opposite hemispheres
  (`q` and `-q`), as Unity's importer keeps its quaternion keys continuous. The tool negates such a key (and its
  tangents) if it ever meets one, so that every player interpolates the short way, and counts it in the report.
- **Static batching.** None of the 99,271 mesh and skinned mesh renderers of the demons and fields is statically batched
  (`m_StaticBatchInfo.subMeshCount` is 0 in all). Unity bakes the world transform into the vertices of a batched mesh, so
  such a renderer would have to be placed at the origin and not at its node; the range of sub-meshes is read as stored.
- **Textures.** Row 0 is the bottom row. Only the largest level, which is the first, is read.

## Left out

Counts come from `check` (all 257 demon bundles, 232 with a model) and `check --kind fields` (all 182 field bundles);
`-v` names the material properties.

**Demons**

- Animation bindings that drive anything but a Transform's position, rotation, scale or Euler angles: GameObject active
  flags (3), renderer material properties (190) and one more kind of renderer property (67). Bindings whose path
  is not in the prefab's hierarchy (2,422) are skipped.
- Euler rotation curves (3,720 channels in 25 demons) are written as quaternion keys. They are exact at the game's keys
  and between them within 0.05 degrees (the largest error of a channel has a median of 0.049 and 99% of the channels are
  within 0.102), found by adding 222,265 keys; in 146 channels an interval still misses that after five halvings, the worst by
  0.33 degrees. Mecanim interpolates the angles and
  converts them at every instant, which is not a polynomial in the quaternion, so this cannot be exact.
- Rotation keys are normalised (glTF requires unit quaternions), which moves them by the amount the stored ones differ
  from unit length. One jump inside a smooth curve is written one float step early. Keys outside a clip's start and
  stop (1,512) are cut to the range, with the value the curve has at the edge.
- Clip settings: loop flags, events, root motion and orientation settings, and the animator controller's states,
  transitions and parameters (only its list of clips is used). The bridge and glTF have no loop flag.
- Geometry: the second UV set (42 meshes), vertex colours (407 meshes whose shader does not read them, 24 whose shader
  does but blends them with parameters that are not mapped), blend shapes (the demons have none), the `h_*` nodes as
  attachment points (they stay plain nodes, no socket is declared because the data does not say what they are).
  Renderers on inactive objects or disabled are skipped (435); one renderer has no mesh. In 34 skinned renderers the
  root bone is not above all the bones, so no skeleton is declared (glTF requires that); in one a bone sits in several
  slots and is merged.
- Materials: for the `Atlus/...` shaders everything except `_MainTex` and the alpha mode (the shaders have 112 to 129
  properties, among them the `_Toon` and `_Ramp1` textures, colours such as `_Color`, `_MainColor` and `_Specular`,
  fog and light vectors), the cull mode and blend state of the shader's passes (nothing is double sided), render queue
  and the other keywords (they are kept as glTF `extras`). Metallic 0 and roughness 1 for those materials are neutral
  values, not game data. `Standard` materials map their `_MainTex`, `_Color`, `_Metallic`, `_Glossiness` and alpha mode;
  their other textures are not mapped. Only the largest level of a texture is read.
- Not read: sounds (`AudioClip`), the `.pb.bytes` demon data tables, scripts (`MonoBehaviour`), sprites and UI objects.

**Fields**

- Only the Unity part is read. The console-era Atlus files that every field carries as TextAssets are listed by
  `list <bundle>` with their tag and size and nothing else: `.TB` (`TXP0`), `.LB` (`TBN`, `INF`), `.f1` and `.f2`
  (`FLD1`, `FLD2`), `.bf` (`FLW0`), `.amb` (`ATMP`), and `.INF`, `.SKY`, `.NPL`, `.WAP`, `.D3P` with no tag. How the
  blocks of a field sit relative to each other is in those files, so a scene is one prefab, not a whole field.
- Lights (49,273 `Light` objects in the install; Unity's intensities have no photometric unit, so none is mapped),
  cameras, colliders, particle systems, and the legacy `Animation` components (15,011 in the install) with their
  clips.
- Of 182 field bundles, 20 have no Unity model at all. In the others: renderers on inactive objects (4,970) are skipped,
  843 renderers have no mesh, 19,128 meshes have a second UV set, 42 sub-meshes have no material, 15 have no entry in
  the renderer's list (the last material is used), 9,450 meshes carry vertex colours the shader does not read and 78,013
  carry ones it reads (not mapped), one has blend shapes. Texture tiling is not applied in the bridge (no field for it)
  and was not set on any field material.
- 1 of 1,066 prefabs (`f804_060`) has a skinned renderer whose mesh has no bone weights; it is drawn rigid.


## Credits

- [rabex](https://github.com/jakobhellermann/RustyAssetBundleEXtractor) by Rudolf Kolbe and Jakob Hellermann (MIT or
  Apache-2.0): the reference that [unity-bundle](https://crates.io/crates/unity-bundle) (ours, which reads the bundles now)
  followed for the structure of its type tree reader and deserializer (its Provenance section says exactly what was read),
  and the oracle it was checked against. Versions 0.1.x of this tool read the bundles with rabex, through our fork with
  three changes. It is no longer a dependency of this crate.
- [AssetStudio](https://github.com/Perfare/AssetStudio) and [AssetRipper](https://github.com/AssetRipper/AssetRipper)
  (both MIT): their public readers were the reference for the semantics of the streamed clip format, generic bindings,
  vertex stream layout and the 2019.4 bundle header. No code was copied.
- [UnityPy](https://github.com/K0lb3/UnityPy) was used as an independent oracle to cross-check numbers; it is not part of
  this project.
- [bcn-decode](https://crates.io/crates/bcn-decode) (ours, written from the public specifications: BC1 to BC7)
  decodes the block-compressed textures, [png](https://crates.io/crates/png) writes PNGs. The earlier build used
  texture2ddecoder, which is no longer a dependency.
- Every dependency comes from crates.io; there is no git or path dependency.

## Disclaimer

Not affiliated with or endorsed by SEGA or Atlus. Shin Megami Tensei and Nocturne are trademarks of their owners. SEGA's
end user license agreement for the game restricts reverse engineering except where the law allows it. This is stated as
a fact, not as legal advice: read the agreement that came with your copy and check what applies where you live before
you use this tool. Use it for reading and studying game data only; do not redistribute game assets.

## License

Licensed under either of MIT or Apache-2.0, at your option.
