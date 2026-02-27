use std::path::PathBuf;

use super::registry::{self, ModelSpec};

/// Return the global models directory (~/.shadw/models/).
pub fn models_dir() -> PathBuf {
    let home = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".shadw/models")
}

/// Return the local path for a model's GGUF file.
pub fn model_path(spec: &ModelSpec) -> PathBuf {
    models_dir().join(spec.hf_file)
}

/// Ensure the model GGUF file is downloaded. Returns the path to the file.
pub fn ensure_model(spec: &ModelSpec) -> Result<PathBuf, String> {
    let path = model_path(spec);

    if path.exists() {
        return Ok(path);
    }

    std::fs::create_dir_all(models_dir())
        .map_err(|e| format!("failed to create models dir: {e}"))?;

    println!(
        "Downloading {} ({})...",
        spec.name,
        registry::human_size(spec.size_bytes)
    );

    let api = hf_hub::api::sync::Api::new()
        .map_err(|e| format!("failed to init HuggingFace API: {e}"))?;

    let repo = api.model(spec.hf_repo.to_string());

    // hf-hub downloads to its own cache; we get the cached path back
    let cached = repo
        .get(spec.hf_file)
        .map_err(|e| format!("download failed: {e}"))?;

    // Symlink (or copy) from hf-hub cache to our models dir so we have a stable path
    if !path.exists() {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&cached, &path)
                .map_err(|e| format!("failed to symlink model: {e}"))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::copy(&cached, &path)
                .map_err(|e| format!("failed to copy model: {e}"))?;
        }
    }

    println!("Model installed: {}", path.display());
    Ok(path)
}

