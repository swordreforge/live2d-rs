use std::path::Path;
use std::sync::Mutex;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::mtmd::*;
use llama_cpp_2::sampling::LlamaSampler;

static MODEL: Mutex<Option<VisionModel>> = Mutex::new(None);

struct VisionModel {
    model: *const LlamaModel,
    mtmd: MtmdContext,
    ctx: llama_cpp_2::context::LlamaContext<'static>,
}

unsafe impl Send for VisionModel {}

fn ensure_loaded(gguf: &str, mmproj: &str) -> Result<(), String> {
    let mut guard = MODEL.lock().unwrap();
    if guard.is_some() {
        return Ok(());
    }

    let backend = LlamaBackend::init().map_err(|e| format!("backend: {e}"))?;
    let model: &'static LlamaModel =
        Box::leak(Box::new(
            LlamaModel::load_from_file(&backend, Path::new(gguf), &LlamaModelParams::default())
                .map_err(|e| format!("load model: {e}"))?,
        ));

    let ctx = model
        .new_context(&backend, LlamaContextParams::default())
        .map_err(|e| format!("context: {e}"))?;

    let mtmd = MtmdContext::init_from_file(mmproj, model, &MtmdContextParams::default())
        .map_err(|e| format!("mmproj: {e}"))?;

    std::mem::forget(backend);

    *guard = Some(VisionModel { model, mtmd, ctx });
    log::info!("Vision model loaded: {gguf}");
    Ok(())
}

pub fn infer_with_image(
    gguf_path: &str,
    mmproj_path: &str,
    jpeg_base64: &str,
    prompt: &str,
) -> Result<String, String> {
    ensure_loaded(gguf_path, mmproj_path)?;
    let mut guard = MODEL.lock().unwrap();
    let vm = guard.as_mut().ok_or("model not loaded")?;

    let jpeg_data = decode_base64(jpeg_base64)?;
    let bitmap = MtmdBitmap::from_buffer(&vm.mtmd, &jpeg_data, false)
        .map_err(|e| format!("bitmap: {e}"))?;

    let marker = mtmd_default_marker();
    let text = MtmdInputText {
        text: format!("{marker}\n{prompt}"),
        add_special: true,
        parse_special: true,
    };
    let chunks = vm.mtmd.tokenize(text, &[&bitmap]).map_err(|e| format!("tokenize: {e}"))?;
    chunks.eval_chunks(&vm.mtmd, &vm.ctx, 0, 0, 512, true)
        .map_err(|e| format!("eval: {e}"))?;

    let model = unsafe { &*vm.model };
    let eos = model.token_eos();
    let mut sampler = LlamaSampler::chain_simple([LlamaSampler::temp(0.7)]);
    let mut tokens = Vec::new();

    for _ in 0..256 {
        let token = sampler.sample(&vm.ctx, -1);
        if token == eos { break; }
        tokens.push(token);
        let mut batch = LlamaBatch::new(1, 1);
        batch.add(token, 0, &[0], true).ok();
        vm.ctx.decode(&mut batch).ok();
    }

    #[allow(deprecated)]
    let result = model.tokens_to_str(&tokens, llama_cpp_2::model::Special::Plaintext);
    result.map(|s| s.trim().to_string()).map_err(|e| format!("decode: {e}"))
}

fn decode_base64(b64: &str) -> Result<Vec<u8>, String> {
    let data = b64.strip_prefix("data:image/jpeg;base64,").unwrap_or(b64);
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| format!("b64: {e}"))
}
