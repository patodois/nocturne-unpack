//! One UnityFS bundle: the container, its serialized file and the streamed resources.
//!
//! Container parsing is done by the `unity-bundle` crate. A [`Bundle`] memory-maps the file, parses
//! the main serialized file (the objects) into memory, and reads `.resS` / `.resource` streams
//! on demand, decompressing only the blocks a request touches. Objects are decoded through the
//! type trees stored in the bundle itself; nothing about field layouts is hardcoded for the
//! Unity version.

use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;
use serde::de::DeserializeOwned;
use unity_bundle::class_id::ClassId;
use unity_bundle::SerializedFile;

/// Unity class ids used here (the `m_ClassID` of an object).
pub mod class {
    pub const GAME_OBJECT: i32 = 1;
    pub const TRANSFORM: i32 = 4;
    pub const MATERIAL: i32 = 21;
    pub const MESH_RENDERER: i32 = 23;
    pub const TEXTURE_2D: i32 = 28;
    pub const MESH_FILTER: i32 = 33;
    pub const MESH: i32 = 43;
    pub const SHADER: i32 = 48;
    pub const TEXT_ASSET: i32 = 49;
    pub const ANIMATION_CLIP: i32 = 74;
    pub const AUDIO_CLIP: i32 = 83;
    pub const ANIMATOR_CONTROLLER: i32 = 91;
    pub const ANIMATOR: i32 = 95;
    pub const ANIMATION: i32 = 111;
    pub const SKINNED_MESH_RENDERER: i32 = 137;
    pub const AVATAR: i32 = 90;
    pub const ASSET_BUNDLE: i32 = 142;
}

/// A location in a streamed resource file, as stored in `m_StreamData` / `m_StreamingInfo`.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct StreamingInfo {
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub size: u32,
    #[serde(default)]
    pub path: String,
}

pub struct Bundle {
    pub path: PathBuf,
    reader: unity_bundle::Bundle<Mmap>,
    /// Name of the serialized file inside the bundle (`CAB-...`)
    pub cab: String,
    file: SerializedFile,
    data: Vec<u8>,
}

impl Bundle {
    /// Opens a bundle and parses its main serialized file.
    pub fn open(path: &Path) -> Result<Bundle> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the game files are not modified while a tool runs; a concurrent change
        // would at worst give a decoding error.
        let map = unsafe { Mmap::map(&file)? };
        let reader = unity_bundle::Bundle::new(map)
            .with_context(|| format!("read bundle header of {}", path.display()))?;
        let entry = reader
            .main_serialized_entry()
            .ok_or_else(|| anyhow!("{}: no serialized file", path.display()))?
            .clone();
        let data = reader.read_entry(&entry)?;
        let file = SerializedFile::parse(&data)
            .with_context(|| format!("parse serialized file of {}", path.display()))?;
        Ok(Bundle {
            path: path.to_path_buf(),
            reader,
            cab: entry.path,
            file,
            data,
        })
    }

    /// Unity version string of the serialized file, e.g. `2019.4.9f1`.
    pub fn unity_version(&self) -> String {
        self.file.unity_version.clone()
    }

    pub fn has_type_tree(&self) -> bool {
        self.file.has_type_tree
    }

    /// Entries of the container (serialized file, `.resS`, `.resource`) with their sizes.
    pub fn entries(&self) -> Vec<(String, u64)> {
        self.reader
            .entries()
            .iter()
            .map(|f| (f.path.clone(), f.size))
            .collect()
    }

    /// Paths of the files this one points into (`m_FileID` 1 is the first, 0 is the file itself).
    pub fn externals(&self) -> Vec<String> {
        self.file.externals.iter().map(|e| e.path.clone()).collect()
    }

    /// `(path_id, class_id)` of every object.
    pub fn objects(&self) -> Vec<(i64, i32)> {
        self.file
            .objects
            .iter()
            .map(|o| (o.path_id, o.class_id.0))
            .collect()
    }

    /// Path ids of the objects of one class.
    pub fn objects_of(&self, class_id: i32) -> Vec<i64> {
        self.file
            .objects
            .iter()
            .filter(|o| o.class_id.0 == class_id)
            .map(|o| o.path_id)
            .collect()
    }

    pub fn class_of(&self, path_id: i64) -> Option<i32> {
        self.file.object(path_id).map(|o| o.class_id.0)
    }

    /// Decodes an object through its type tree into any `serde` type.
    pub fn read<T: DeserializeOwned>(&self, path_id: i64) -> Result<T> {
        let info = self
            .file
            .object(path_id)
            .ok_or_else(|| anyhow!("no object {path_id}"))?;
        self.file
            .read::<T>(&self.data, info)
            .map_err(|e| anyhow!("object {path_id} ({:?}): {e}", info.class_id))
    }

    /// The raw bytes of an object, as stored.
    pub fn raw(&self, path_id: i64) -> Result<&[u8]> {
        let info = self
            .file
            .object(path_id)
            .ok_or_else(|| anyhow!("no object {path_id}"))?;
        self.file
            .raw(&self.data, info)
            .map_err(|_| anyhow!("object {path_id} lies outside the file"))
    }

    /// A dump of the type tree an object was written with.
    pub fn type_tree_dump(&self, path_id: i64) -> Result<String> {
        let info = self
            .file
            .object(path_id)
            .ok_or_else(|| anyhow!("no object {path_id}"))?;
        let tt = self
            .file
            .type_tree(info)
            .ok_or_else(|| anyhow!("{}", unity_bundle::Error::NoTypeTree))?;
        Ok(tt.dump())
    }

    /// Reads `size` bytes at `offset` of a streamed resource (`archive:/CAB-.../CAB-....resS`).
    /// Only the compression blocks that cover the range are decompressed.
    pub fn read_stream(&self, info: &StreamingInfo) -> Result<Vec<u8>> {
        let name = info.path.rsplit('/').next().unwrap_or(&info.path);
        let Some(entry) = self.reader.entry(name) else {
            bail!("streamed resource {} is not in the bundle", info.path);
        };
        Ok(self
            .reader
            .read_entry_range(entry, info.offset as u64, info.size as usize)?)
    }

    /// The name of a class id, if `unity-bundle` knows it (for census output).
    pub fn class_name(class_id: i32) -> String {
        ClassId(class_id)
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("Class{class_id}"))
    }
}
