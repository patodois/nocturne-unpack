//! Readers for the Unity asset bundles of Shin Megami Tensei III Nocturne HD Remaster.

pub mod asset;
#[cfg(feature = "bridge")]
pub mod bridge;
pub mod bundle;
pub mod clip;
pub mod gltf;
pub mod install;
pub mod mesh;
pub mod prefab;
pub mod rig;
pub mod texture;
pub mod transform;
pub mod unity;

// The code in the README is compiled by `cargo test --features bridge`
#[cfg(all(doctest, feature = "bridge"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
