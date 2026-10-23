# nocturne-unpack

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

## Build

```sh
cargo build --release
cargo build --release --features bridge     # adds the rewrite-bridge provider and check-bridge
```

## Usage

`--game` is the game folder (the one that holds `smt3hd_Data`) and defaults to `~/Games/smt3hd`. It also accepts the
`StreamingAssets/PC` folder itself. Bundles are named as in that folder (`dvl0x01`) or given as a path. Bundles have no
file extension, and the file called `PC` is Unity's AssetBundleManifest.

```sh
G=~/Games/smt3hd

# Families of bundles with counts and sizes; the demons or fields; or what one bundle holds
nocturne-unpack list --game $G
nocturne-unpack list demons --game $G --filter dvl0x0
nocturne-unpack list fields --game $G
nocturne-unpack list f012 --game $G          # objects, prefabs, TextAssets with their tags and sizes

# A demon as a skinned, textured .glb with every clip of its animator; --prefab picks another
# prefab of the bundle, --no-textures and --no-animations leave those out
nocturne-unpack export dvl0x01 dvl0x01.glb --game $G
# A field prefab (a field is made of several, one per block, each in its own frame) or all of them
nocturne-unpack export f012 f012_002.glb --prefab f012_002 --game $G
nocturne-unpack export f012 f012_all.glb --all-prefabs --game $G

# Load every demon bundle (base game and DLC), count failures, report the peak memory; --kind fields
# does the same for the field bundles (all prefabs of a bundle at once)
nocturne-unpack check --game $G
nocturne-unpack check --game $G --kind fields --no-textures

# One texture of a bundle as PNG (the largest level), or the textures of a bundle with their formats
nocturne-unpack texture dvl0x01 --game $G
nocturne-unpack texture dvl0x01 0x01_t05 -o t05.png --game $G

# Through the rewrite-bridge provider: load and validate every demon (with its clips) and every field prefab
# (as a model and as a scene); --textures builds the materials with their textures too
nocturne-unpack check-bridge --game $G
nocturne-unpack check-bridge --game $G --kind demons --textures

# The rewrite-bridge conformance checker on what the provider returns (options as in its README, plus
# --game and --no-textures); --kind character for the demons, --kind level for field prefabs
nocturne-unpack check-conformance --game $G --kind character --compare-load
nocturne-unpack check-conformance --game $G --id f012/f012_002 -v

# Object classes and texture formats across all bundles; --decode also decodes every texture and mesh
nocturne-unpack census --game $G --decode

# Exploring: one object as JSON (with its type tree), clip diagnostics, numbers for cross-checks
nocturne-unpack dump dvl0x01 74 --nth 0 --tree --game $G
nocturne-unpack anim dvl0x07 --game $G
nocturne-unpack probe dvl0x01 --mesh "0x01 body" --node "p Pelvis/b Spine" --clip b00 --time 0.5 --game $G
```

`-v` names every material property that is not carried over instead of counting them.

Every bundle is opened on its own: the file is memory-mapped, its objects are parsed into memory and the `.resS`
streams are read in ranges, decompressing only the compression blocks a request touches. The sweeps (`check`,
`census`) never hold more than one bundle.

The tests in `tests/game_data.rs` run against a real install when `NOCTURNE_GAME_DIR` points at the game folder:

```sh
NOCTURNE_GAME_DIR=~/Games/smt3hd cargo test --release
```

## Container layer

The bundles are read with [rabex](https://github.com/jakobhellermann/RustyAssetBundleEXtractor) 0.2.1, which reads
UnityFS bundles and serialized files and deserializes objects through type trees. It could not open these bundles as
released, because Unity 2019.4 writes version 6 bundles and rabex read their header one byte late. `vendor/rabex` is
rabex 0.2.1 with three small changes (a patch for upstream is in `patches/`, the story is in `NOTES-upstream.md`):

- no extra byte after the header flags for UnityFS, and the 2019.4 block-info alignment done the way AssetStudio does it
- `read_at_entry_range`, which decompresses only the blocks that cover a range of a bundle entry, for `.resS` streams
- the buffer of a file read from a bundle is reserved up front, which stopped the process from growing by about 12 MB
  every time the same bundle was opened and dropped again

With that, all 3,088 bundles (2,251 in the base game, 837 in the DLC folders) open, and every object decodes through the
type tree stored in its own bundle. No field layout is hardcoded for the Unity version.

## What a demon holds

A `dvl0x..` bundle holds one prefab (four also hold a second one: an afterimage, an older or an alternate version; the one
with the most renderers is the default) and everything it uses:

- the GameObject / Transform hierarchy, with rigid meshes (`MeshFilter` + `MeshRenderer`) on many bones and a few
  `SkinnedMeshRenderer`s
- `Mesh` objects, `Material`s, `Texture2D`s (streamed from the `.resS`), an `AnimatorController` whose clips are
  Mecanim generic clips (not legacy, not humanoid), and `Avatar`s
- sounds and a `.pb.bytes` data table, which are not read

25 of the 259 demon bundles have no model at all, only the data table and sounds.

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
`Provider` (pinned to git rev `0c5d0b0e08fcff58166f2ed6ad3daf9bdc72c0a8`), like wf-unpack's.

- `list` returns every demon (id is the bundle name, `dvl0x01`, kind Character) and every field prefab that has
  something to draw (`f012/f012_002`, kind Level); prefabs of only transforms are left out.
- `load_animated` returns a validated demon with its clips as `Animation`s and `Channel`s on the model's nodes; `load`
  is the same model. For a field prefab, `load` gives its static model.
- `load_scene` (field prefabs) returns a scene: one inline model per distinct mesh with its materials, placed by an
  instance with the node's world transform. The game data has almost no instancing (83,306 placements of 82,649 models).
  A world matrix with shear cannot be a translation, rotation and scale; it is counted and the closest transform kept.
- The bridge has no place for texture samplers and tiling, vertex colours or a second UV set. Lights and cameras are not
  read.

## Conformance

`check-conformance` runs rewrite-bridge's conformance checker (`rewrite_bridge::conformance`) over the provider's output.
Over all 234 demons (`load_animated`, `--compare-load`) it reports no errors; over 45 field prefabs spread over the
install (`load_scene`) one, a mesh whose own triangles wind against its own normals. What it reports besides is
the game's data, not the provider, and stays as it is (`-v` lists it):

- `skin.bind_pose_mismatch` (info, 214 demons): Unity's bind poses are the bone's world-to-local matrix times the
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
- `material.blend_opaque_texture` (35 demons, 37 field prefabs): these are additive materials (`_DstBlend` 1) and
  materials whose alpha comes from vertex colours (not exported). glTF has no additive mode, so they come out as `BLEND`.
  `material.opaque_transparent_texture`: the alpha channel of an opaque material's texture is data for the Atlus
  shader, not transparency (render queue 2000, blend factors one and zero). Alpha modes follow Unity's keywords, and
  in every Atlus material of the demons they agree with `_DstBlend` and the render queue (blend keywords go with
  factor 1 or 10 and queue 3000, alpha test with queue 2450, the rest is opaque).
- `orientation.not_upright` (6 demons): snakes and dragons, 7 m long and 1 m high. `animation.zero_scale` (`dvl0xc0d`):
  the node `ebony` is hidden by scaling it to zero. `animation.pose_extent_mismatch` (4): a part that moves 9 m away from
  the body during a clip (`dvl0x0e` `b12`), a skeleton that retracts to a fifth of its size (`dvl0x95` `b01`). `animation.duplicate_name` / `animation.empty`
  (`dvl0x110`, `dvl0x40`, `dvl0x73`): the animator lists two clips with the same name (in `dvl0x110` the second has 119
  bindings against 171) and one of them resolves to no channel. `scene.instance_scale_extreme` (`f676_010`): an object scaled to 8e-6 to hide it.
  `geometry.winding_flipped` (`f003_001`, 120 triangles): the source mesh has every triangle against its normals;
  100,000 others agree, so this is that mesh.

## Verification

Tested against the Steam (Windows) build found in `~/Games/smt3hd` on 2026-10-23 (Unity 2019.4.9f1 and 2019.4.10f1; 29
bundles are 2018.4.13f1).

All numbers are from one run on a 16 GB machine, one bundle open at a time; "peak" is the peak resident size.

- `census --decode`: all 3,088 bundles (2,251 in the base game, 837 in the DLC folders) open, every one with type trees,
  none failing; 15,930 `Texture2D` (Alpha8 10, DXT1 10,742, DXT5 3,455, BC7 41, RGB24 1,179, RGBA32 503) and all 112,166
  `Mesh` objects decode without a failure (peak 857 MB, 40 s). The classes of its 1,468,692 objects include 435,455
  GameObject, 408,793 Transform, 119,629 MeshRenderer, 3,749 SkinnedMeshRenderer, 35,323 Material, 12,552 AudioClip,
  9,594 AnimationClip, 8,568 TextAsset and 406 Shader; `nocturne-unpack census` prints them all.
- `check`: 259 demon bundles (257 in the base game, 2 in the DLC): 234 load, 25 hold no model (a data table and sounds
  only), 0 failures. 540,397 triangles, 538,893 vertices, 4,562 meshes, 334 skins, 2,849 materials, 2,032 textures and
  1,764 clips with 251,991 channels decode in 6.4 s with a peak of 301 MB. The materials use `Atlus/Atlus_CHAR` (2,826),
  `Atlus/AtlusMat` (10), `Atlus/Atlus_CHAR2` (4) and `Standard` (9); 116 have a tiled or offset `_MainTex`.
- `check-bridge`: 234 of 234 demons load and validate as `AnimatedModel`s with textures (20 s, peak 419 MB), and all
  1,466 field prefabs load as a static model and as a validated scene (118 s, peak 422 MB without textures): 124,356
  meshes, 19.8 million vertices, 82,687 scene models placed by 83,344 instances.
- **Cross-checks** against UnityPy 1.25.4, which reads the same bundles independently, for the vertex count, first vertex
  (position, normal, UV, bone weights and indices), first indices and bind pose of a skinned mesh, a bone's local
  transform, the path CRC-32, and a clip's sampled position and rotation (UnityPy has no sampler, so the oracle
  evaluates the raw streamed keys itself, in double precision): on `dvl0x01`, `dvl0x95` and `dvl0x07` every integer and
  every mesh and transform value is identical; the largest difference in a sampled clip value is 2.9e-8 (a float
  against a double). On `dvl0x07`, where the clip is made of Euler curves, the written channel is 0.003 and 0.001 degrees
  from the true curve at two times between keys. Textures of all six formats are pixel-identical to Pillow's independent
  decoders (DXT1, DXT5, BC7) and to the raw data (RGB24, RGBA32, Alpha8).
- Of the 234 demon exports, every one passes the Khronos glTF validator with no errors (warnings only: a skinned mesh
  below the root, a clip that also animates the node that carries a skinned mesh). A sample of 44 field prefabs passes too.
- Renders, made with three.js in headless Chromium: demons in the bind pose and in the middle of a clip, and a field.
  They are not in the repository.
- `cargo test --release` with `NOCTURNE_GAME_DIR` set: 31 unit tests, 6 tests against the install and 4 against the
  install through the bridge; without it only the unit tests run. `cargo clippy --all-targets -- -D warnings` is clean,
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
- **Textures.** Row 0 is the bottom row. Only the largest level, which is the first, is read.

## Left out

Counts come from `check` (all 234 demons) and `check --kind fields` (all 201 field bundles); `-v` names the material
properties.

**Demons**

- Animation bindings that drive anything but a Transform's position, rotation, scale or Euler angles: GameObject active
  flags (3), renderer material properties (191) and one more kind of renderer property (134 curves). Bindings whose path
  is not in the prefab's hierarchy (2,542) are skipped.
- Euler rotation curves (3,720 channels in 25 demons) are written as quaternion keys. They are exact at the game's keys
  and between them within 0.05 degrees (median 0.04, 99% within 0.105), found by adding 271,216 keys; in 704 channels
  an interval still misses that after five halvings, the worst by 0.33 degrees. Mecanim interpolates the angles and
  converts them at every instant, which is not a polynomial in the quaternion, so this cannot be exact.
- Rotation keys are normalised (glTF requires unit quaternions), which moves them by the amount the stored ones differ
  from unit length. One jump inside a smooth curve is written one float step early. Keys outside a clip's start and
  stop (1,512) are cut to the range, with the value the curve has at the edge.
- Clip settings: loop flags, events, root motion and orientation settings, and the animator controller's states,
  transitions and parameters (only its list of clips is used). The bridge and glTF have no loop flag.
- Geometry: the second UV set (56 meshes), vertex colours (433 meshes whose shader does not read them, 28 whose shader
  does but blends them with parameters that are not mapped), blend shapes (the demons have none), the `h_*` nodes as
  attachment points (they stay plain nodes, no socket is declared because the data does not say what they are).
  Renderers on inactive objects or disabled are skipped (444); one renderer has no mesh. In 35 skinned renderers the
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
- Of 201 field bundles, 20 have no Unity model at all. In the others: renderers on inactive objects (5,493) are skipped,
  864 renderers have no mesh, 24,406 meshes have a second UV set, 60 sub-meshes have no material, 15 have no entry in
  the renderer's list (the last material is used), 10,562 meshes carry vertex colours the shader does not read and 96,062
  carry ones it reads (not mapped), one has blend shapes. Texture tiling is not applied in the bridge (no field for it)
  and was not set on any field material.
- 1 of 1,497 prefabs (`f804_060`) has a skinned renderer whose mesh has no bone weights; it is drawn rigid.


## Credits

- [rabex](https://github.com/jakobhellermann/RustyAssetBundleEXtractor) by Rudolf Kolbe and Jakob Hellermann (MIT or
  Apache-2.0): bundle and serialized file parsing, type tree deserialization. Vendored with the three changes above.
- [AssetStudio](https://github.com/Perfare/AssetStudio) and [AssetRipper](https://github.com/AssetRipper/AssetRipper)
  (both MIT): their public readers were the reference for the semantics of the streamed clip format, generic bindings,
  vertex stream layout and the 2019.4 bundle header. No code was copied.
- [UnityPy](https://github.com/K0lb3/UnityPy) was used as an independent oracle to cross-check numbers; it is not part of
  this project.
- [texture2ddecoder](https://crates.io/crates/texture2ddecoder) decodes the block-compressed textures,
  [png](https://crates.io/crates/png) writes PNGs.

## Disclaimer

Not affiliated with or endorsed by SEGA or Atlus. Shin Megami Tensei and Nocturne are trademarks of their owners. SEGA's
end user license agreement for the game restricts reverse engineering except where the law allows it. This is stated as
a fact, not as legal advice: read the agreement that came with your copy and check what applies where you live before
you use this tool. Use it for reading and studying game data only; do not redistribute game assets.

## License

Licensed under either of MIT or Apache-2.0, at your option. `vendor/rabex` keeps its own licenses.
