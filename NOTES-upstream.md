> **Historical.** `nocturne-unpack` no longer depends on rabex or on this fork: it reads bundles with `unity-bundle`
> (see the README, "Container layer"). The notes below describe the three changes to rabex that were proposed in
> jakobhellermann/RustyAssetBundleEXtractor#1 and are kept for that pull request.

# Notes for upstream

`nocturne-unpack` reads UnityFS bundles with [rabex](https://github.com/jakobhellermann/RustyAssetBundleEXtractor)
0.2.1 (MIT OR Apache-2.0). That release cannot open the bundles of this game (Unity 2019.4, bundle
format version 6), so `Cargo.toml` depends on the branch `nocturne` of our fork,
[patodois/RustyAssetBundleEXtractor](https://github.com/patodois/RustyAssetBundleEXtractor): rabex 0.2.1
plus three small changes, one commit each on top of upstream's `main` (e9bd237, `release v0.2.1`), merged by
that branch. It is a git dependency and not a `[patch]` section, because Cargo applies `[patch]` only in the
root of a build, so a crate that depends on this one would have got the unpatched rabex and failed to compile.
The changes are proposed upstream as
[jakobhellermann/RustyAssetBundleEXtractor#1](https://github.com/jakobhellermann/RustyAssetBundleEXtractor/pull/1);
when they are released, the dependency goes back to a version number. With them all 2,251 base-game
bundles and the DLC bundles open and decode.

| fork branch | change |
|---|---|
| `unityfs-2019-4-header` | section 1 |
| `bundle-read-at-entry-range` | section 2 |
| `bundle-reserve-read-buffer` | section 3 |
| `nocturne` | the three merged: what this crate builds against |

## 1. UnityFS header: no extra byte, and 2019.4 block-info padding

File: `src/files/bundlefile/mod.rs`, function `read_unityfs_info`.

- After the header's `size`, `compressed_size`, `uncompressed_size` and `flags`, rabex reads one
  more byte when the signature is **UnityFS**. That byte exists only for the older signatures
  (AssetStudio reads it when the signature is *not* `UnityFS`). With version 7 bundles the stray byte is
  hidden by the 16-byte alignment that follows (unless the header happens to end on a 16-byte
  boundary), but version 6 bundles, which Unity 2019.4 writes, have no alignment step, so the block
  info is read one byte late and decompression fails with
  `the offset to copy is not contained in the decompressed buffer`.
- The existing `// TODO else if version >= (2019, 4)` is implemented the way AssetStudio does it: for
  a version 6 bundle written by Unity 2019.4 or later, the position is aligned to 16 only if the
  skipped bytes are all zero. In this game's bundles they are not (the block info starts right after
  the header), so nothing is skipped.

Evidence: header of `dvl0x01` (Unity 2019.4.9f1): `UnityFS\0`, version 6, `5.x.x\0`, `2019.4.9f1\0`,
size 2,761,073, block info 298 bytes compressed (LZ4HC) and 599 uncompressed, flags `0x43`. The LZ4
data starts at offset 49, directly after the flags.

## 2. A ranged read for files inside a bundle

File: `src/files/bundlefile/reader.rs`.

`BundleFileReader::read_at_entry_range(entry, start, len)` decompresses only the blocks that cover
`start..start + len` of a file entry. `read_at_entry` has to decompress a whole entry, which is
wasteful for `.resS` files (streamed textures and meshes) when one object needs a few kilobytes.
It reuses the existing `read_single_file`.

## 3. Reserve the buffer for a file read from a bundle

File: `src/files/bundlefile/reader.rs`, function `read_single_file`.

The decompressed blocks are appended to a `Vec::new()`, which grows by doubling and copies the data each
time, leaving a chain of freed buffers up to the size of the file. On macOS the process kept about 12 MB more
resident every time the same bundle was loaded and dropped again (resident size after each of 12 loads of a
48 MB bundle with an 88 MB serialized file: 383 MB, 415, 423, ... 541 MB, peak 844 MB). Reserving the size of
the file plus at most one block on each side stops it (393 MB, 394 MB, ... 395 MB, peak 463 MB). Loading every
field prefab of the game one by one (1,497 times) is where it mattered.

The directory of a bundle is data from the file, so the reservation is also bounded by what the remaining
blocks can decompress to and by 256 MiB (`try_reserve_exact`, so a failed reservation falls back to the usual
growth). With the first version a bundle whose directory claims a file of 2^60 bytes aborted the process with a
failed allocation; the fork's `tests/bundle_read_files.rs` has that case.

## Not changed, but worth knowing

- `rabex` with `default-features = false, features = ["compression-lz4", "compression-lzma"]` works
  without the embedded type tree blob, because every bundle here carries its own type trees; this
  crate passes `NullTypeTreeProvider`.
- `typetree::provider` is private; `NullTypeTreeProvider` is re-exported from `typetree`, which is enough.
