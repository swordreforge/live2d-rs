use std::path::{Path, PathBuf};
use std::collections::HashMap;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "PascalCase")]
pub struct Model3Json {
    pub version: u32,
    pub file_references: FileReferences,
    pub groups: Option<Vec<Group>>,
    pub hit_areas: Option<Vec<HitArea>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "PascalCase")]
pub struct FileReferences {
    pub moc: String,
    pub textures: Vec<String>,
    pub physics: Option<String>,
    pub pose: Option<String>,
    pub display_info: Option<String>,
    pub expressions: Option<Vec<ExpressionRef>>,
    pub motions: Option<HashMap<String, Vec<MotionRef>>>,
    pub user_data: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "PascalCase")]
pub struct ExpressionRef {
    pub name: String,
    pub file: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "PascalCase")]
pub struct MotionRef {
    pub file: String,
    pub fade_in_time: Option<f64>,
    pub fade_out_time: Option<f64>,
    pub sound: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "PascalCase")]
pub struct Group {
    pub target: String,
    pub name: String,
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "PascalCase")]
pub struct HitArea {
    pub id: String,
    pub name: String,
}

impl Model3Json {
    pub fn from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn texture_paths(&self) -> &[String] {
        &self.file_references.textures
    }

    pub fn moc3_path(&self) -> &str {
        &self.file_references.moc
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
        let moc3_path = base_dir.join(json.moc3_path());
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
