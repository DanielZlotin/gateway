use serde::Serialize;
use std::fs;
use std::path::Path;

pub struct SaveJsonLabels {
    pub create_dir: &'static str,
    pub write: &'static str,
    pub replace: &'static str,
}

pub fn save_pretty_json<T: Serialize>(
    path: &Path,
    value: &T,
    labels: SaveJsonLabels,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", labels.create_dir))?;
    }
    let mut data = serde_json::to_vec_pretty(value).map_err(|err| err.to_string())?;
    data.push(b'\n');
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, data).map_err(|err| format!("{}: {err}", labels.write))?;
    fs::rename(&tmp, path).map_err(|err| format!("{}: {err}", labels.replace))
}
