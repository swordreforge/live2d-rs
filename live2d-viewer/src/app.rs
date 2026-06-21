use std::path::PathBuf;
use live2d_core::{Moc, Model};

pub struct ModelEntry {
    pub name: String,
    pub dir: PathBuf,
    pub loaded: bool,
}

pub struct AppState {
    pub model_list: Vec<ModelEntry>,
    pub current_idx: Option<usize>,
    pub current_moc: Option<Moc>,
    pub current_model: Option<Model<'static>>,
    pub parameter_values: Vec<f32>,
    pub parameter_names: Vec<String>,
    pub parameter_mins: Vec<f32>,
    pub parameter_maxs: Vec<f32>,
    pub texture_paths: Vec<PathBuf>,
    pub error_message: Option<String>,
    pub mouse_down: bool,
    pub last_mouse_x: f64,
    pub last_mouse_y: f64,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            model_list: Vec::new(),
            current_idx: None,
            current_moc: None,
            current_model: None,
            parameter_values: Vec::new(),
            parameter_names: Vec::new(),
            parameter_mins: Vec::new(),
            parameter_maxs: Vec::new(),
            texture_paths: Vec::new(),
            error_message: None,
            mouse_down: false,
            last_mouse_x: 0.0,
            last_mouse_y: 0.0,
        }
    }

    pub fn add_model_dir(&mut self, path: PathBuf) {
        let name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        self.model_list.push(ModelEntry { name, dir: path, loaded: false });
    }

    pub fn switch_to(&mut self, idx: usize) -> Result<(), String> {
        if idx >= self.model_list.len() {
            return Err("index out of range".into());
        }

        // Drop order: Model first, then Moc (struct field order: current_model after current_moc)
        self.current_model = None;
        self.current_moc = None;
        self.parameter_values.clear();
        self.parameter_names.clear();
        self.parameter_mins.clear();
        self.parameter_maxs.clear();

        let entry = &self.model_list[idx];
        let loaded = crate::model_loader::LoadedModel::load(&entry.dir)
            .map_err(|e| format!("load model: {e}"))?;

        let moc = Moc::revive(&loaded.moc3_data)
            .map_err(|e| format!("revive moc: {e}"))?;

        // Store Moc first, then create Model referencing it via raw pointer
        // to avoid borrow-checker conflicts with PhantomData<&Moc> in Model.
        let moc_ptr: *const Moc = &moc as *const Moc;
        // SAFETY: moc_ptr is valid; Moc lives in self.current_moc, outliving Model in self.current_model
        let model = unsafe { Model::initialize(&*moc_ptr) }
            .map_err(|e| format!("init model: {e}"))?;
        // SAFETY: field order guarantees Moc outlives Model
        let model: Model<'static> = unsafe { std::mem::transmute(model) };

        let params = model.parameters();
        for id in params.ids() {
            self.parameter_names.push(id.to_string_lossy().into_owned());
        }
        self.parameter_values = params.default_values().to_vec();
        self.parameter_mins = params.minimum_values().to_vec();
        self.parameter_maxs = params.maximum_values().to_vec();

        self.current_moc = Some(moc);
        self.current_model = Some(model);
        self.current_idx = Some(idx);
        self.texture_paths = loaded.texture_paths();
        self.model_list[idx].loaded = true;
        Ok(())
    }

    pub fn update_parameters(&mut self) {
        if let Some(ref mut model) = self.current_model {
            let mut params = model.parameters();
            let mut vals = params.values_mut();
            for (i, &v) in self.parameter_values.iter().enumerate() {
                vals.set(i, v);
            }
        }
    }
}
