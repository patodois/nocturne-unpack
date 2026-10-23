//! One UnityFS bundle: the container, its serialized file and the streamed resources.
//!
//! Container parsing is done by the `rabex` crate. A [`Bundle`] memory-maps the file, parses
//! the main serialized file (the objects) into memory, and reads `.resS` / `.resource` streams
//! on demand, decompressing only the blocks a request touches. Objects are decoded through the
//! type trees stored in the bundle itself; nothing about field layouts is hardcoded for the
//! Unity version.

use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;
use rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
use rabex::files::SerializedFile;
use rabex::objects::ClassId;
use rabex::typetree::NullTypeTreeProvider;
use serde::de::DeserializeOwned;

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
    reader: BundleFileReader<Cursor<Mmap>>,
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
        let config = ExtractionConfig::default().assume_recent_unity();
        let reader = BundleFileReader::from_reader(Cursor::new(map), &config)
            .with_context(|| format!("read bundle header of {}", path.display()))?;
        let entry = reader
            .main_serializedfile()
            .ok_or_else(|| anyhow!("{}: no serialized file", path.display()))?
            .clone();
        let data = reader.read_at_entry(&entry)?;
        let file = SerializedFile::from_reader(&mut Cursor::new(&data[..]))
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
        self.file
            .m_UnityVersion
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default()
    }

    pub fn has_type_tree(&self) -> bool {
        self.file.m_EnableTypeTree
    }

    /// Entries of the container (serialized file, `.resS`, `.resource`) with their sizes.
    pub fn entries(&self) -> Vec<(String, u64)> {
        self.reader
            .files()
            .iter()
            .map(|f| (f.path.clone(), f.size.max(0) as u64))
            .collect()
    }

    /// Paths of the files this one points into (`m_FileID` 1 is the first, 0 is the file itself).
    pub fn externals(&self) -> Vec<String> {
        self.file.externals_paths().map(str::to_string).collect()
    }

    /// `(path_id, class_id)` of every object.
    pub fn objects(&self) -> Vec<(i64, i32)> {
        self.file
            .objects()
            .map(|o| (o.m_PathID, o.m_ClassID.0))
            .collect()
    }

    /// Path ids of the objects of one class.
    pub fn objects_of(&self, class_id: i32) -> Vec<i64> {
        self.file
            .objects()
            .filter(|o| o.m_ClassID.0 == class_id)
            .map(|o| o.m_PathID)
            .collect()
    }

    pub fn class_of(&self, path_id: i64) -> Option<i32> {
        self.file.get_object_info(path_id).map(|o| o.m_ClassID.0)
    }

    /// Decodes an object through its type tree into any `serde` type.
    pub fn read<T: DeserializeOwned>(&self, path_id: i64) -> Result<T> {
        let info = self
            .file
            .get_object_info(path_id)
            .ok_or_else(|| anyhow!("no object {path_id}"))?;
        let mut cur = Cursor::new(&self.data[..]);
        self.file
            .read::<T>(info, NullTypeTreeProvider, &mut cur)
            .map_err(|e| anyhow!("object {path_id} ({:?}): {e}", info.m_ClassID))
    }

    /// The raw bytes of an object, as stored.
    pub fn raw(&self, path_id: i64) -> Result<&[u8]> {
        let info = self
            .file
            .get_object_info(path_id)
            .ok_or_else(|| anyhow!("no object {path_id}"))?;
        let start = info.m_Offset as usize;
        self.data
            .get(start..start + info.m_Size as usize)
            .ok_or_else(|| anyhow!("object {path_id} lies outside the file"))
    }

    /// A dump of the type tree an object was written with.
    pub fn type_tree_dump(&self, path_id: i64) -> Result<String> {
        let info = self
            .file
            .get_object_info(path_id)
            .ok_or_else(|| anyhow!("no object {path_id}"))?;
        let tt = self
            .file
            .get_typetree_for(info, &NullTypeTreeProvider)
            .map_err(|e| anyhow!("{e}"))?;
        Ok(tt.dump())
    }

    /// Reads `size` bytes at `offset` of a streamed resource (`archive:/CAB-.../CAB-....resS`).
    /// Only the compression blocks that cover the range are decompressed.
    pub fn read_stream(&self, info: &StreamingInfo) -> Result<Vec<u8>> {
        let name = info.path.rsplit('/').next().unwrap_or(&info.path);
        let Some(entry) = self.reader.files().iter().find(|f| f.path == name) else {
            bail!("streamed resource {} is not in the bundle", info.path);
        };
        Ok(self
            .reader
            .read_at_entry_range(entry, info.offset as u64, info.size as usize)?)
    }

    /// Whether a class id is one rabex knows by name (for census output).
    pub fn class_name(class_id: i32) -> String {
        ClassId(class_id)
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("Class{class_id}"))
    }
}
