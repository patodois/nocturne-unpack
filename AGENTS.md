# Agent guide

nocturne-unpack reads Shin Megami Tensei III Nocturne HD Remaster assets for developers studying or exporting their own game installation.
It supplies glTF exports and an optional rewrite-bridge provider for demons, animation and field prefabs.

## Toolchain, build and CI

- Rust 1.88 minimum (`Cargo.toml`); normal CI uses stable Rust on Linux, macOS and Windows.
- `bridge` is optional and off by default; CI checks both configurations.
- Exact CI commands from the root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features bridge -- -D warnings
cargo test
cargo test --features bridge
```

The README builds with `cargo build --release`; add `--features bridge` for provider commands.
Tests that read the game use `NOCTURNE_GAME_DIR` and skip without it.
Release CI builds with `cargo build --release --locked --target <target> -p nocturne-unpack --features bridge`.
Its targets are `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`.

## Code map

- `src/install.rs`, `bundle.rs`, `asset.rs`, `unity.rs`: installed files, containers and object access.
- `src/mesh.rs`, `texture.rs`, `clip.rs`, `transform.rs`: geometry, images and animation.
- `src/prefab.rs`, `rig.rs`: hierarchy and demon rigs.
- `src/bridge.rs`, `gltf.rs`: provider and export conversion.
- `src/main.rs`, `tests/`: command-line tools and synthetic/install regressions.

## Project rules

- Read the Steam Windows build's files from the user's own copy; the folder holds `smt3hd_Data`.
- Use unity-bundle for the container/type-tree layer and bcn-decode for compressed textures.
- Preserve the documented Unity version scope and report unsupported data rather than inventing it.
- No extracted textures, meshes, dumps, screenshots or game-derived fixtures may be committed.
- Read `.resS` byte ranges through the container layer rather than expanding every streamed resource.
- Whole-field/conformance work can approach gigabytes; the README records a 2 GB level-conformance peak.
- Maintain the distinction between Unity field prefabs and console-era Atlus data that is only listed.

## Public files

- Never commit game files, extracted assets, renders, decompiled code or secrets.
- The public-tree gate needs Python 3.9+ and Git; run it from the repository root:
  `python3 tools/check_public_tree.py` (checks Git's index, including staged new files).
- CI checks committed changes with `python3 tools/check_public_tree.py --range BASE..HEAD`.

## Contributing, license and AI

- Open an issue to discuss a problem or send a pull request with the change.
- Before a pull request, run the applicable CI commands above and the public-tree gate.
- License: MIT OR Apache-2.0, at your option; see `LICENSE-MIT` and `LICENSE-APACHE`.

The code was written with AI coding agents (Anthropic's Claude and OpenAI's GPT models) under the maintainer's direction.
AI-assisted contributions are welcome when their use is disclosed and the changes are reviewed.
