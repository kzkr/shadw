/// A curated model that Shadw knows how to download and run.
pub struct ModelSpec {
    pub name: &'static str,
    pub tagline: &'static str,
    pub hf_repo: &'static str,
    pub hf_file: &'static str,
    pub size_bytes: u64,
    pub params: &'static str,
    pub license: &'static str,
    pub n_ctx: u32,
}

static MODELS: &[ModelSpec] = &[ModelSpec {
    name: "gpt-oss",
    tagline: "OpenAI's open-weight model for reasoning, agentic tasks, and developer use cases.",
    hf_repo: "ggml-org/gpt-oss-20b-GGUF",
    hf_file: "gpt-oss-20b-mxfp4.gguf",
    size_bytes: 12_109_568_423,
    params: "20B MoE",
    license: "Apache 2.0",
    n_ctx: 16384,
}];

pub fn list_models() -> &'static [ModelSpec] {
    MODELS
}

pub fn get_model(name: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.name == name)
}

pub fn human_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.0} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{bytes} B")
    }
}
