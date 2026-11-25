//! The offline data folder: aircraft models and transit maps live in plain files, so nothing has to be
//! downloaded from GitHub (or anywhere) each time the program is built or started.
//!
//! ```text
//! data/
//!   models/<id>.bin     baked aircraft (see tools/bake_plane.py)
//!   transit/<id>.tmap   compact transit maps (see `rustymaps --fetch-transit`)
//! ```
//!
//! Where it is looked for: `$RUSTYMAPS_DATA`, else a `data` folder next to the program, else
//! `%LOCALAPPDATA%\rustymaps\data` (or the platform equivalent).

use std::path::{Path, PathBuf};

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The per-user fallback location (also where downloads are saved when no `data` folder exists).
pub fn user_data_dir() -> PathBuf {
    let base = if cfg!(windows) {
        env_path("LOCALAPPDATA").or_else(|| env_path("TEMP"))
    } else if cfg!(target_os = "macos") {
        env_path("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        env_path("XDG_DATA_HOME").or_else(|| env_path("HOME").map(|h| h.join(".local").join("share")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("rustymaps").join("data")
}

pub fn data_dir() -> PathBuf {
    if let Some(d) = env_path("RUSTYMAPS_DATA") {
        return d;
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("data"))) {
        if d.is_dir() {
            return d;
        }
    }
    user_data_dir()
}

/// A baked aircraft, still as bytes (the GPU copy is made by `plane::PlaneGpu::add_model`).
pub struct ModelData {
    pub bytes: Vec<u8>,
    pub vertices: usize,
    pub indices: usize,
    /// Radius (metres) of a sphere around the model origin that contains every vertex.
    pub radius_m: f64,
}

pub const MODEL_HEADER: usize = 12;
pub const VERTEX_SIZE: usize = 20;

pub fn parse_model(bytes: Vec<u8>) -> Result<ModelData, String> {
    if bytes.len() < MODEL_HEADER {
        return Err("model file is too short".into());
    }
    let u = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let (nv, ni) = (u(0) as usize, u(4) as usize);
    let radius_m = f32::from_bits(u(8)) as f64;
    if nv == 0 || ni == 0 || ni % 3 != 0 || bytes.len() != MODEL_HEADER + nv * VERTEX_SIZE + ni * 4 {
        return Err("model file has the wrong size (was it baked with an older tools/bake_plane.py?)".into());
    }
    if !(1.0..1000.0).contains(&radius_m) {
        return Err("model file has an implausible radius".into());
    }
    Ok(ModelData { bytes, vertices: nv, indices: ni, radius_m })
}

pub fn load_model(dir: &Path, id: &str) -> Result<ModelData, String> {
    let path = dir.join("models").join(format!("{id}.bin"));
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_model(bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&21.5f32.to_le_bytes());
        b.extend_from_slice(&vec![0u8; 3 * VERTEX_SIZE]);
        for i in 0..3u32 {
            b.extend_from_slice(&i.to_le_bytes());
        }
        b
    }

    #[test]
    fn model_header_is_checked() {
        let m = parse_model(tiny()).unwrap();
        assert_eq!((m.vertices, m.indices), (3, 3));
        assert!((m.radius_m - 21.5).abs() < 1e-6);
        let mut bad = tiny();
        bad.pop();
        assert!(parse_model(bad).is_err());
        assert!(parse_model(vec![1, 2, 3]).is_err());
    }
}
