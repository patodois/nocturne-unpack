# Notes for upstream

`nocturne-unpack` reads UnityFS bundles with [rabex](https://github.com/jakobhellermann/RustyAssetBundleEXtractor)
0.2.1 (MIT OR Apache-2.0). That release cannot open the bundles of this game (Unity 2019.4, bundle
format version 6), so `vendor/rabex` is a copy of 0.2.1 with three small changes, wired in through
`[patch.crates-io]`. The same changes are in `patches/rabex-0.2.1.patch` (a `diff -ru` of `src/`,
`-p1` applies it to a checkout of the crate). With them all 2,251 base-game bundles and the DLC
bundles open and decode.

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
time. For a 90 MB serialized file that means a chain of freed buffers up to the size of the file, and with
macOS's allocator the process kept about 12 MB more resident every time the same bundle was opened and dropped
again (a 62 MB bundle opened 12 times: 393 MB, 415, 423, ... 541 MB, peak 844 MB). Reserving
`file.size` plus the part of the first block that precedes the file removes it (393 MB, 394 MB, ... 395 MB, peak
463 MB). Opening every field bundle prefab by prefab (1,497 times) is the case where it matters.

## Not changed, but worth knowing

- `rabex` with `default-features = false, features = ["compression-lz4", "compression-lzma"]` works
  without the embedded type tree blob, because every bundle here carries its own type trees; this
  crate passes `NullTypeTreeProvider`.
- `typetree::provider` is private; `NullTypeTreeProvider` is re-exported from `typetree`, which is enough.
