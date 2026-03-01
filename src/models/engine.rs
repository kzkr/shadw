use std::path::Path;

use indicatif::{ProgressBar, ProgressStyle};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

/// Run inference on a GGUF model file using the model's built-in chat template.
/// Generates up to `max_tokens` tokens and returns the generated text.
///
/// `prefix` is appended to the templated prompt so the model continues from it.
/// The prefix is included in the returned string to force structured output.
pub fn infer(
    model_path: &Path,
    system: &str,
    user: &str,
    prefix: &str,
    max_tokens: u32,
    n_ctx: u32,
) -> Result<String, String> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    spinner.set_message("Loading model...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));

    let mut backend = LlamaBackend::init()
        .map_err(|e| format!("failed to init llama backend: {e}"))?;

    // Silence llama.cpp's verbose C++ logging (Metal shader compilation, etc.)
    backend.void_logs();

    let model_params = LlamaModelParams::default();
    let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
        .map_err(|e| format!("failed to load model: {e}"))?;

    let n_batch: u32 = 2048;
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(n_ctx))
        .with_n_batch(n_batch);
    let mut ctx = model
        .new_context(&backend, ctx_params)
        .map_err(|e| format!("failed to create context: {e}"))?;

    spinner.set_message("Extracting decisions...");

    // Use the model's built-in chat template (Harmony for gpt-oss, ChatML for others, etc.)
    let tmpl = model
        .chat_template(None)
        .map_err(|e| format!("model has no chat template: {e}"))?;

    let messages = [
        LlamaChatMessage::new("system".into(), system.into())
            .map_err(|e| format!("invalid message: {e}"))?,
        LlamaChatMessage::new("user".into(), user.into())
            .map_err(|e| format!("invalid message: {e}"))?,
    ];

    // add_ass=true leaves the prompt open for the assistant to continue
    let mut prompt = model
        .apply_chat_template(&tmpl, &messages, true)
        .map_err(|e| format!("failed to apply chat template: {e}"))?;

    // Append the prefix so the model is forced to continue from it
    prompt.push_str(prefix);

    let mut tokens = model
        .str_to_token(&prompt, llama_cpp_2::model::AddBos::Always)
        .map_err(|e| format!("tokenization failed: {e}"))?;

    // Ensure prompt leaves enough room for generation (at least 4096 tokens)
    let max_prompt_tokens = (n_ctx as usize).saturating_sub(4096);
    if tokens.len() > max_prompt_tokens {
        tracing::warn!(
            "prompt too long ({} tokens), truncating to {}",
            tokens.len(),
            max_prompt_tokens
        );
        tokens.truncate(max_prompt_tokens);
    }

    // Process prompt in chunks — avoids exceeding n_batch on long prompts
    let batch_size = n_batch as usize;
    let mut pos = 0;
    while pos < tokens.len() {
        let end = (pos + batch_size).min(tokens.len());
        let chunk = &tokens[pos..end];
        let mut batch = LlamaBatch::new(chunk.len(), 1);
        for (j, &tok) in chunk.iter().enumerate() {
            let is_last = pos + j == tokens.len() - 1;
            batch.add(tok, (pos + j) as i32, &[0], is_last)
                .map_err(|e| format!("batch add failed: {e}"))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| format!("decode failed at position {pos}: {e}"))?;
        pos = end;
    }

    // Sampling loop
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(0.2),
        LlamaSampler::top_p(0.9, 1),
        LlamaSampler::dist(42),
    ]);

    let mut output = String::new();
    let mut n_cur = tokens.len() as i32;
    let mut gen_batch = LlamaBatch::new(1, 1);

    for _ in 0..max_tokens {
        let token = sampler.sample(&ctx, -1);

        if model.is_eog_token(token) {
            break;
        }

        let piece_bytes = model
            .token_to_piece_bytes(token, 64, true, None)
            .map_err(|e| format!("detokenize failed: {e}"))?;
        let piece = String::from_utf8_lossy(&piece_bytes);
        output.push_str(&piece);

        gen_batch.clear();
        gen_batch
            .add(token, n_cur, &[0], true)
            .map_err(|e| format!("batch add failed: {e}"))?;
        n_cur += 1;

        ctx.decode(&mut gen_batch)
            .map_err(|e| format!("decode failed: {e}"))?;
    }

    spinner.finish_and_clear();

    // Prepend the prefix to the generated continuation
    let full = format!("{prefix}{}", output.trim());
    Ok(full)
}
