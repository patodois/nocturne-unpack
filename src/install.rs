//! Finding bundles in a game install, and following references between them.
//!
//! The base game's bundles are in `smt3hd_Data/StreamingAssets/PC` (the file called `PC` there
//! is the AssetBundleManifest); downloadable content has more in `dlc/<n>/`. Bundles have no file
//! extension; the file name is the bundle name other bundles list as a dependency.

use std::collections::HashMap;
use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};

use crate::bundle::{class, Bundle};
use crate::unity::{PPtr, Shader};

pub const DEFAULT_GAME_DIR: &str = "~/Games/smt3hd";

/// Expands a leading `~/`.
pub fn expand_home(p: &Path) -> PathBuf {
    if let Ok(rest) = p.strip_prefix("~") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    p.to_path_buf()
}

pub struct Install {
    /// The game folder (or the folder holding the bundles, when pointed straight at it)
    pub game: PathBuf,
    /// `StreamingAssets/PC`
    pub pc: PathBuf,
    /// `dlc/<n>` folders
    pub dlc: Vec<PathBuf>,
    cab_index: Mutex<Option<HashMap<String, PathBuf>>>,
    shader_names: Mutex<HashMap<(String, i64), String>>,
}

impl Install {
    /// Opens a game folder (`~/Games/smt3hd`) or directly its `StreamingAssets/PC` folder.
    pub fn open(game: &Path) -> Result<Install> {
        let game = expand_home(game);
        let pc = [
            game.join("smt3hd_Data/StreamingAssets/PC"),
            game.join("StreamingAssets/PC"),
            game.clone(),
        ]
        .into_iter()
        .find(|p| p.join("PC").is_file())
        .with_context(|| {
            format!(
                "{} holds no smt3hd_Data/StreamingAssets/PC folder (the one with the `PC` manifest)",
                game.display()
            )
        })?;
        let mut dlc: Vec<PathBuf> = std::fs::read_dir(game.join("dlc"))
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        dlc.sort();
        Ok(Install {
            game,
            pc,
            dlc,
            cab_index: Mutex::new(None),
            shader_names: Mutex::new(HashMap::new()),
        })
    }

    fn list_dir(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Every bundle file of the base game, without the manifest.
    pub fn base_bundles(&self) -> Vec<PathBuf> {
        Self::list_dir(&self.pc)
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n != "PC"))
            .collect()
    }

    /// Every bundle file of the DLC folders.
    pub fn dlc_bundles(&self) -> Vec<PathBuf> {
        self.dlc.iter().flat_map(|d| Self::list_dir(d)).collect()
    }

    /// Demon bundles (`dvl0x..`) of the base game and the DLC, sorted by name.
    pub fn demon_bundles(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .base_bundles()
            .into_iter()
            .chain(self.dlc_bundles())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("dvl0x"))
            })
            .collect();
        v.sort_by_key(|p| p.file_name().map(|n| n.to_os_string()));
        v
    }

    /// The file of a bundle by name, looking in the base game first and then the DLC.
    pub fn bundle_path(&self, name: &str) -> Option<PathBuf> {
        let p = self.pc.join(name);
        if p.is_file() {
            return Some(p);
        }
        self.dlc.iter().map(|d| d.join(name)).find(|p| p.is_file())
    }

    /// Resolves what the user typed: a path, or a bundle name such as `dvl0x01`.
    pub fn resolve(&self, what: &str) -> Result<PathBuf> {
        let direct = expand_home(Path::new(what));
        if direct.is_file() {
            return Ok(direct);
        }
        match self.bundle_path(what) {
            Some(p) => Ok(p),
            None => bail!("no bundle {what:?} in {}", self.game.display()),
        }
    }

    /// Names of the serialized files in a bundle, read from its header only.
    fn cabs_of(path: &Path) -> Result<Vec<String>> {
        let file = File::open(path)?;
        let map = unsafe { Mmap::map(&file)? };
        let reader = BundleFileReader::from_reader(
            Cursor::new(map),
            &ExtractionConfig::default().assume_recent_unity(),
        )?;
        Ok(reader
            .files()
            .iter()
            .filter(|f| !f.path.contains('.'))
            .map(|f| f.path.clone())
            .collect())
    }

    fn with_cab_index<T>(&self, f: impl FnOnce(&HashMap<String, PathBuf>) -> T) -> T {
        let mut guard = self.cab_index.lock().unwrap();
        let index = guard.get_or_insert_with(|| {
            let mut m = HashMap::new();
            for p in self.base_bundles().into_iter().chain(self.dlc_bundles()) {
                if let Ok(cabs) = Self::cabs_of(&p) {
                    for c in cabs {
                        m.entry(c).or_insert_with(|| p.clone());
                    }
                }
            }
            m
        });
        f(index)
    }

    /// The bundle that holds a serialized file, by its `CAB-...` name.
    pub fn bundle_of_cab(&self, cab: &str) -> Option<PathBuf> {
        self.with_cab_index(|m| m.get(cab).cloned())
    }

    /// The name of the file a pointer with a non-zero file id refers to (`CAB-...`).
    pub fn external_cab(bundle: &Bundle, file_id: i32) -> Option<String> {
        let ext = bundle.externals();
        let path = ext.get(usize::try_from(file_id).ok()?.checked_sub(1)?)?;
        // `archive:/CAB-x/CAB-x` or a plain file name such as `Resources/unity_builtin_extra`
        path.starts_with("archive:/")
            .then(|| path.rsplit('/').next().unwrap_or(path).to_string())
    }

    /// Name of the shader a material points at, following the pointer into another bundle when
    /// needed. Anything unresolvable is described instead, never guessed.
    pub fn shader_name(&self, bundle: &Bundle, ptr: &PPtr) -> String {
        if ptr.m_PathID == 0 {
            return "(none)".into();
        }
        if ptr.m_FileID == 0 {
            return match bundle.read::<Shader>(ptr.m_PathID) {
                Ok(s) => s.m_ParsedForm.m_Name,
                Err(_) => format!("(unreadable shader {})", ptr.m_PathID),
            };
        }
        let Some(cab) = Self::external_cab(bundle, ptr.m_FileID) else {
            let ext = bundle.externals();
            let what = usize::try_from(ptr.m_FileID)
                .ok()
                .and_then(|i| ext.get(i.wrapping_sub(1)))
                .cloned()
                .unwrap_or_default();
            return format!("(built-in {what})");
        };
        let key = (cab.clone(), ptr.m_PathID);
        if let Some(n) = self.shader_names.lock().unwrap().get(&key) {
            return n.clone();
        }
        // Load every shader of that file once
        let mut found = HashMap::new();
        if let Some(path) = self.bundle_of_cab(&cab) {
            if let Ok(other) = Bundle::open(&path) {
                for id in other.objects_of(class::SHADER) {
                    if let Ok(s) = other.read::<Shader>(id) {
                        found.insert((cab.clone(), id), s.m_ParsedForm.m_Name);
                    }
                }
            }
        }
        let mut cache = self.shader_names.lock().unwrap();
        cache.extend(found);
        cache
            .get(&key)
            .cloned()
            .unwrap_or_else(|| format!("(shader {} in {cab})", ptr.m_PathID))
    }
}
