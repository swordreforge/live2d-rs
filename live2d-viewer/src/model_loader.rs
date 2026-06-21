use std::path::{Path, PathBuf};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Model3Json {
    pub version: u32,
    #[serde(default)]
    pub file_references: Vec<FileReference>,
    pub groups: Option<Vec<Group>>,
    pub hit_areas: Option<Vec<HitArea>>,
}

#[derive(Debug, Deserialize)]
pub struct FileReference {
    pub id: Option<String>,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct Group {
    pub target: String,
    pub name: String,
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct HitArea {
    pub id: String,
    pub name: String,
}

impl Model3Json {
    pub fn from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn texture_paths(&self) -> Vec<&str> {
        self.file_references.iter()
            .filter(|r| r.path.ends_with(".png") || r.path.ends_with(".jpg"))
            .map(|r| r.path.as_str())
            .collect()
    }

    pub fn moc3_path(&self) -> Option<&str> {
        self.file_references.iter()
            .find(|r| r.path.ends_with(".moc3"))
            .map(|r| r.path.as_str())
    }
}

pub struct LoadedModel {
    pub model3_json: Model3Json,
    pub moc3_data: Vec<u8>,
    pub base_dir: PathBuf,
}

impl LoadedModel {
    pub fn load<P: AsRef<Path>>(model_dir: P) -> anyhow::Result<Self> {
        let model_dir = model_dir.as_ref();
        let model3_path = find_model3_json(model_dir)?;
        let base_dir = model3_path.parent().unwrap_or(model_dir).to_path_buf();

        let json = Model3Json::from_file(&model3_path)?;
        let moc3_rel = json.moc3_path()
            .ok_or_else(|| anyhow::anyhow!("No .moc3 in model3.json"))?;
        let moc3_path = base_dir.join(moc3_rel);
        let moc3_data = std::fs::read(&moc3_path)?;

        Ok(Self { model3_json: json, moc3_data, base_dir })
    }

    pub fn texture_paths(&self) -> Vec<PathBuf> {
        self.model3_json.texture_paths()
            .iter()
            .map(|p| self.base_dir.join(p))
            .collect()
    }
}

fn find_model3_json(dir: &Path) -> anyhow::Result<PathBuf> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_stem() {
                if name.to_string_lossy().ends_with(".model3") {
                    return Ok(path);
                }
            }
        }
    }
    anyhow::bail!("No *.model3.json found in {:?}", dir)
}
