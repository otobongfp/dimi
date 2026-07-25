use crate::common::{not_implemented, InferenceBackend, PromptContext, Result, Token, TokenStream};
use async_trait::async_trait;

#[async_trait]
pub trait InferenceEngine: Send + Sync {
    async fn generate(&self, context: PromptContext) -> Result<TokenStream>;
    async fn embed_prompt_tokens(&self, text: &str) -> Result<Vec<Token>>;
    fn backend(&self) -> InferenceBackend;
    fn is_loaded(&self) -> bool;
}

pub struct StubInferenceEngine;

#[async_trait]
impl InferenceEngine for StubInferenceEngine {
    async fn generate(&self, _context: PromptContext) -> Result<TokenStream> {
        not_implemented("InferenceEngine::generate")
    }
    async fn embed_prompt_tokens(&self, _text: &str) -> Result<Vec<Token>> {
        not_implemented("InferenceEngine::embed_prompt_tokens")
    }
    fn backend(&self) -> InferenceBackend {
        InferenceBackend::LlamaCpp
    }
    fn is_loaded(&self) -> bool {
        false
    }
}

pub struct SwappableInferenceEngine {
    inner: std::sync::RwLock<Arc<dyn InferenceEngine>>,
}

impl SwappableInferenceEngine {
    pub fn new(initial: Arc<dyn InferenceEngine>) -> Self {
        Self {
            inner: std::sync::RwLock::new(initial),
        }
    }

    pub fn swap(&self, new_engine: Arc<dyn InferenceEngine>) {
        *self.inner.write().unwrap() = new_engine;
    }
}

#[async_trait]
impl InferenceEngine for SwappableInferenceEngine {
    async fn generate(&self, context: PromptContext) -> Result<TokenStream> {
        let engine = self.inner.read().unwrap().clone();
        engine.generate(context).await
    }

    async fn embed_prompt_tokens(&self, text: &str) -> Result<Vec<Token>> {
        let engine = self.inner.read().unwrap().clone();
        engine.embed_prompt_tokens(text).await
    }

    fn backend(&self) -> InferenceBackend {
        self.inner.read().unwrap().backend()
    }

    fn is_loaded(&self) -> bool {
        self.inner.read().unwrap().is_loaded()
    }
}

use crate::common::DimiError;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

const MAX_GENERATED_TOKENS: usize = 2048;
const SAMPLING_SEED: u32 = 1234;

// Chaining `dist` (probabilistic sampling) followed by `greedy` (always
// takes the single argmax token) makes `dist` a no-op — greedy always wins
// the final selection. That made every generation fully deterministic, which
// is exactly wrong when a task is hard for the model (e.g. translating a
// paragraph into Swahili): argmax decoding collapses to the single "safest"
// continuation at each step, which in practice means the model gives up and
// repeats a generic canned reply instead of attempting the task. top_k/top_p
// truncate the tail before `temp` reshapes the remaining distribution, then
// `dist` actually draws from it — real variety, but bounded (moderate
// temperature) so document-grounded answers don't get looser along with it.
const TOP_K: i32 = 40;
const TOP_P: f32 = 0.9;
const TEMPERATURE: f32 = 0.6;
static BACKEND: std::sync::Mutex<Option<Arc<LlamaBackend>>> = std::sync::Mutex::new(None);

fn shared_backend() -> Result<Arc<LlamaBackend>> {
    let mut guard = BACKEND.lock().unwrap();
    if let Some(backend) = guard.as_ref() {
        return Ok(backend.clone());
    }
    let backend = Arc::new(
        LlamaBackend::init()
            .map_err(|e| DimiError::Internal(format!("backend init failed: {e}")))?,
    );
    *guard = Some(backend.clone());
    Ok(backend)
}

const REPEAT_PENALTY_LAST_N: i32 = 64;
const REPEAT_PENALTY_REPEAT: f32 = 1.1;

struct GenerationJob {
    prompt: String,
    sender: tokio_mpsc::UnboundedSender<Result<String>>,
}

pub struct LlamaCppInferenceEngine {
    job_tx: std_mpsc::Sender<GenerationJob>,
    loaded: Arc<AtomicBool>,
    needs_no_think: bool,
}

impl LlamaCppInferenceEngine {
    pub fn new(
        model_path: PathBuf,
        kv_bytes_per_token: u64,
        needs_no_think: bool,
        memory_budget_bytes: Option<u64>,
    ) -> Result<Self> {
        let backend = shared_backend()?;
        let model_size_bytes = std::fs::metadata(&model_path).map(|m| m.len()).unwrap_or(0);
        let (job_tx, job_rx) = std_mpsc::channel::<GenerationJob>();
        let loaded = Arc::new(AtomicBool::new(false));
        let loaded_for_worker = loaded.clone();
        let (ready_tx, ready_rx) = std_mpsc::channel::<std::result::Result<(), String>>();

        std::thread::spawn(move || {
            run_worker(
                backend,
                model_path,
                kv_bytes_per_token,
                memory_budget_bytes,
                model_size_bytes,
                job_rx,
                &loaded_for_worker,
                ready_tx,
            );
        });

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                job_tx,
                loaded,
                needs_no_think,
            }),
            Ok(Err(e)) => Err(DimiError::Internal(format!("failed to load model: {e}"))),
            Err(_) => Err(DimiError::Internal(
                "inference worker thread died while loading the model".into(),
            )),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    backend: Arc<LlamaBackend>,
    model_path: PathBuf,
    kv_bytes_per_token: u64,
    memory_budget_bytes: Option<u64>,
    model_size_bytes: u64,
    job_rx: std_mpsc::Receiver<GenerationJob>,
    loaded: &AtomicBool,
    ready_tx: std_mpsc::Sender<std::result::Result<(), String>>,
) {
    let mut model_params = LlamaModelParams::default();
    let budget = memory_budget_bytes.unwrap_or_else(|| {
        let available = crate::kernel::hardware::available_ram_bytes();
        let total = crate::kernel::hardware::total_ram_bytes();
        let half_total = total / 2;
        let safe_available = available.saturating_sub(2 * 1024 * 1024 * 1024);
        half_total.min(safe_available).max(1024 * 1024 * 1024)
    });

    let target_model_budget = if budget > 1024 * 1024 * 1024 {
        budget.saturating_sub(1024 * 1024 * 1024)
    } else {
        0
    };

    if target_model_budget < model_size_bytes {
        let layer_size = model_size_bytes.max(1) / 36;
        let n_gpu_layers = (target_model_budget / layer_size.max(1)) as u32;
        model_params = model_params.with_n_gpu_layers(n_gpu_layers);
    }

    let model = match LlamaModel::load_from_file(&backend, &model_path, &model_params) {
        Ok(m) => m,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("load_from_file failed: {e}")));
            return;
        }
    };
    loaded.store(true, Ordering::SeqCst);
    let _ = ready_tx.send(Ok(()));

    for job in job_rx {
        run_generation(
            &backend,
            &model,
            kv_bytes_per_token,
            memory_budget_bytes,
            model_size_bytes,
            &job,
        );
    }
}

fn run_generation(
    backend: &LlamaBackend,
    model: &LlamaModel,
    kv_bytes_per_token: u64,
    memory_budget_bytes: Option<u64>,
    model_size_bytes: u64,
    job: &GenerationJob,
) {
    let tokens_list = match model.str_to_token(&job.prompt, AddBos::Never) {
        Ok(t) => t,
        Err(e) => {
            let _ = job
                .sender
                .send(Err(DimiError::Internal(format!("tokenize failed: {e}"))));
            return;
        }
    };
    if tokens_list.is_empty() {
        let _ = job
            .sender
            .send(Err(DimiError::InvalidArgument("empty prompt".into())));
        return;
    }

    let cpu_temp = crate::kernel::hardware::current_cpu_temp_celsius();
    let thermal_scale = crate::kernel::hardware::thermal_context_scale(cpu_temp);
    let ram_budget = crate::kernel::hardware::dynamic_context_tokens(
        kv_bytes_per_token,
        memory_budget_bytes,
        model_size_bytes,
        thermal_scale,
    );
    let needed = tokens_list.len() as u32 + MAX_GENERATED_TOKENS as u32;
    let context_tokens = ram_budget.max(needed);
    tracing::debug!(
        context_tokens,
        ram_budget,
        thermal_scale,
        ?cpu_temp,
        prompt_tokens = tokens_list.len(),
        "sized context window for this generation"
    );
    tracing::trace!("Starting generation with prompt:\n{}", job.prompt);

    let n_threads = crate::kernel::hardware::physical_core_count().saturating_sub(1).max(1);

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(context_tokens))
        .with_n_threads(n_threads)
        .with_n_threads_batch(n_threads);
    let mut ctx = match model.new_context(backend, ctx_params) {
        Ok(c) => c,
        Err(e) => {
            let _ = job.sender.send(Err(DimiError::Internal(format!(
                "context init failed: {e}"
            ))));
            return;
        }
    };

    let n_batch = ctx.n_batch() as usize;
    let mut batch = LlamaBatch::new(n_batch.min(tokens_list.len().max(1)), 1);
    let mut decode_failed = false;
    for (chunk_index, chunk) in tokens_list.chunks(n_batch.max(1)).enumerate() {
        batch.clear();
        let chunk_start = chunk_index * n_batch.max(1);
        let is_last_chunk = chunk_start + chunk.len() >= tokens_list.len();
        for (i, token) in chunk.iter().enumerate() {
            let pos = (chunk_start + i) as i32;
            let is_last_token = is_last_chunk && i == chunk.len() - 1;
            if let Err(e) = batch.add(*token, pos, &[0], is_last_token) {
                let _ = job
                    .sender
                    .send(Err(DimiError::Internal(format!("batch add failed: {e}"))));
                return;
            }
        }
        if let Err(e) = ctx.decode(&mut batch) {
            decode_failed = true;
            let _ = job.sender.send(Err(DimiError::Internal(format!(
                "initial decode failed (chunk {chunk_index}): {e}"
            ))));
            break;
        }
    }
    if decode_failed {
        return;
    }

    let mut n_cur = tokens_list.len() as i32;
    let n_len = n_cur + MAX_GENERATED_TOKENS as i32;
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::penalties(REPEAT_PENALTY_LAST_N, REPEAT_PENALTY_REPEAT, 0.0, 0.0),
        LlamaSampler::top_k(TOP_K),
        LlamaSampler::top_p(TOP_P, 1),
        LlamaSampler::temp(TEMPERATURE),
        LlamaSampler::dist(SAMPLING_SEED),
    ]);
    let mut decoder = encoding_rs::UTF_8.new_decoder();

    while n_cur < n_len {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);

        if model.is_eog_token(token) {
            tracing::debug!("Hit EOG token: {}", token);
            break;
        }

        let piece = match model.token_to_piece(token, &mut decoder, true, None) {
            Ok(p) => p,
            Err(e) => {
                let _ = job
                    .sender
                    .send(Err(DimiError::Internal(format!("detokenize failed: {e}"))));
                return;
            }
        };
        tracing::trace!("Generated token piece: {:?}", piece);
        if job.sender.send(Ok(piece)).is_err() {
            return;
        }

        batch.clear();
        if let Err(e) = batch.add(token, n_cur, &[0], true) {
            let _ = job
                .sender
                .send(Err(DimiError::Internal(format!("batch add failed: {e}"))));
            return;
        }
        if let Err(e) = ctx.decode(&mut batch) {
            let _ = job
                .sender
                .send(Err(DimiError::Internal(format!("decode failed: {e}"))));
            return;
        }
        n_cur += 1;
    }
}

fn render_chatml(context: &crate::common::PromptContext, needs_no_think: bool) -> String {
    let mut out = String::new();
    let tool_instructions = if context.tools.is_empty() {
        None
    } else {
        Some(render_tool_instructions(&context.tools))
    };

    let mut header_applied = false;
    for msg in &context.messages {
        if msg.role == "system" && !header_applied {
            header_applied = true;
            let mut content = String::new();
            if let Some(instructions) = &tool_instructions {
                content.push_str(instructions);
                content.push_str("\n\n");
            }
            content.push_str(&msg.content);
            if needs_no_think {
                content.push_str("\n/no_think");
            }
            out.push_str(&format!("<|im_start|>{}\n{}<|im_end|>\n", msg.role, content));
            continue;
        }
        out.push_str(&format!(
            "<|im_start|>{}\n{}<|im_end|>\n",
            msg.role, msg.content
        ));
    }
    out.push_str("<|im_start|>assistant\n");
    if needs_no_think {
        out.push_str("<think>\n\n</think>\n\n");
    }
    out
}

fn render_tool_instructions(tools: &[crate::common::ToolSchema]) -> String {
    let schemas: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect();
    format!(
        "You have access to the following tools. Only call one when it's actually \
         needed to answer the user's request — most messages (greetings, general \
         knowledge, casual conversation) need no tool at all; answer those \
         directly and immediately, with no step-by-step reasoning before you \
         answer.\n\nTools:\n{}\n\nTo call a tool, respond with ONLY a single line \
         of this exact form and nothing else — no explanation before or after \
         it:\n<tool_call>\n{{\"name\": \"<tool name>\", \"arguments\": <JSON \
         object matching that tool's parameters>}}\n</tool_call>\nYou will then \
         be given the tool's result and can continue from there.",
        serde_json::to_string_pretty(&schemas).unwrap_or_default()
    )
}

#[async_trait]
impl InferenceEngine for LlamaCppInferenceEngine {
    async fn generate(&self, context: PromptContext) -> Result<TokenStream> {
        let prompt = render_chatml(&context, self.needs_no_think);
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        self.job_tx
            .send(GenerationJob { prompt, sender: tx })
            .map_err(|_| DimiError::Internal("inference worker thread is gone".into()))?;
        Ok(Box::pin(UnboundedReceiverStream::new(rx)))
    }

    async fn embed_prompt_tokens(&self, _text: &str) -> Result<Vec<Token>> {
        crate::common::not_implemented("InferenceEngine::embed_prompt_tokens")
    }

    fn backend(&self) -> InferenceBackend {
        InferenceBackend::LlamaCpp
    }

    fn is_loaded(&self) -> bool {
        self.loaded.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ChatMessage;
    use futures_core::Stream;
    use std::pin::Pin;

    #[tokio::test]
    #[ignore = "needs a real GGUF file at DIMI_DEV_MODEL_PATH; loads a multi-GB model"]
    async fn generates_real_tokens_from_local_model() {
        let model_path = std::env::var("DIMI_DEV_MODEL_PATH")
            .expect("set DIMI_DEV_MODEL_PATH to a local .gguf file to run this test");

        let engine = LlamaCppInferenceEngine::new(
            std::path::PathBuf::from(model_path),
            147_456,
            true,
            None,
        )
        .expect("model should load");
        assert!(engine.is_loaded());

        let context = PromptContext {
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "You are a concise assistant.".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "Say the single word: hello".to_string(),
                },
            ],
            tools: vec![],
        };

        let mut stream: Pin<Box<dyn Stream<Item = Result<String>> + Send>> =
            engine.generate(context).await.unwrap();

        let mut collected = String::new();
        for _ in 0..40 {
            match tokio_stream::StreamExt::next(&mut stream).await {
                Some(Ok(piece)) => collected.push_str(&piece),
                Some(Err(e)) => panic!("generation error: {e}"),
                None => break,
            }
        }

        println!("generated: {collected:?}");
        assert!(!collected.trim().is_empty(), "model produced no output");
    }

    #[tokio::test]
    #[ignore = "needs a real GGUF file at DIMI_DEV_MODEL_PATH; loads a multi-GB model"]
    async fn generates_from_a_prompt_longer_than_n_batch() {
        let model_path = std::env::var("DIMI_DEV_MODEL_PATH")
            .expect("set DIMI_DEV_MODEL_PATH to a local .gguf file to run this test");

        let engine =
            LlamaCppInferenceEngine::new(std::path::PathBuf::from(model_path), 28_672, true, None)
                .expect("model should load");

        let long_document = "The quick brown fox jumps over the lazy dog. ".repeat(600);
        let context = PromptContext {
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "Summarize the following text in one sentence.".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: long_document,
                },
            ],
            tools: vec![],
        };

        let mut stream: Pin<Box<dyn Stream<Item = Result<String>> + Send>> =
            engine.generate(context).await.unwrap();

        let mut collected = String::new();
        for _ in 0..40 {
            match tokio_stream::StreamExt::next(&mut stream).await {
                Some(Ok(piece)) => collected.push_str(&piece),
                Some(Err(e)) => panic!("generation error: {e}"),
                None => break,
            }
        }

        println!("generated: {collected:?}");
        assert!(!collected.trim().is_empty(), "model produced no output");
    }

    #[tokio::test]
    #[ignore = "needs a real GGUF file at DIMI_DEV_MODEL_PATH; loads a multi-GB model"]
    async fn attempts_a_hard_task_instead_of_repeating_a_canned_deflection() {
        // Regression test for a real failure: chained `dist()` then
        // `greedy()` made every generation fully deterministic argmax, which
        // made the model give up on hard requests (translating a paragraph
        // into Swahili) and repeat the exact same generic "I'm here to help"
        // non-answer for two different phrasings of the same ask.
        let model_path = std::env::var("DIMI_DEV_MODEL_PATH")
            .expect("set DIMI_DEV_MODEL_PATH to a local .gguf file to run this test");

        let engine = LlamaCppInferenceEngine::new(
            std::path::PathBuf::from(model_path),
            28_672,
            true,
            None,
        )
        .expect("model should load");

        let story = "Once upon a time, there was a little bird named Maka who lived in a tree near the river.";
        let context = PromptContext {
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "You are a helpful assistant.".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "Tell me a short story".to_string(),
                },
                ChatMessage {
                    role: "assistant".to_string(),
                    content: story.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "convert the story to Swahili version".to_string(),
                },
            ],
            tools: vec![],
        };

        let mut stream: Pin<Box<dyn Stream<Item = Result<String>> + Send>> =
            engine.generate(context).await.unwrap();

        let mut collected = String::new();
        for _ in 0..120 {
            match tokio_stream::StreamExt::next(&mut stream).await {
                Some(Ok(piece)) => collected.push_str(&piece),
                Some(Err(e)) => panic!("generation error: {e}"),
                None => break,
            }
        }

        println!("generated: {collected:?}");
        assert!(!collected.trim().is_empty(), "model produced no output");
        assert!(
            !collected.contains("I am here to help you with anything you need"),
            "model deflected with the canned non-answer instead of attempting the task: {collected:?}"
        );
    }

    #[tokio::test]
    #[ignore = "needs a real GGUF file at DIMI_DEV_MODEL_PATH; loads a multi-GB model"]
    async fn does_not_hallucinate_a_content_policy_refusal_on_a_cold_language_request() {
        // Regression test for a real failure observed live: a fresh "write a
        // short story in Swahili" (no prior Swahili context in the
        // conversation) got refused with "I can't generate content that
        // violates community guidelines" — a fabricated policy violation,
        // not a real one. See ANTI_REFUSAL_GUIDANCE in services::context.
        let model_path = std::env::var("DIMI_DEV_MODEL_PATH")
            .expect("set DIMI_DEV_MODEL_PATH to a local .gguf file to run this test");

        let engine = LlamaCppInferenceEngine::new(
            std::path::PathBuf::from(model_path),
            28_672,
            false,
            None,
        )
        .expect("model should load");

        let context = PromptContext {
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "You are Dimi, a private local AI assistant. No library is attached \
                        to this conversation, so you have no documents to search — answer from \
                        general knowledge and say so if a question needs a specific library's \
                        documents.\n\nOrdinary requests — creative writing, translation, or \
                        answering in any language the user asks for (including less common ones) \
                        — do not violate any content policy. Don't invent a guidelines violation \
                        or claim you're limited to English or any other language; attempt the \
                        task directly. Only decline requests that are genuinely harmful."
                        .to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "Hi".to_string(),
                },
                ChatMessage {
                    role: "assistant".to_string(),
                    content: "Hello! How can I assist you today?".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "can you write a short story in Swahili?".to_string(),
                },
            ],
            tools: vec![],
        };

        let mut stream: Pin<Box<dyn Stream<Item = Result<String>> + Send>> =
            engine.generate(context).await.unwrap();

        let mut collected = String::new();
        for _ in 0..900 {
            match tokio_stream::StreamExt::next(&mut stream).await {
                Some(Ok(piece)) => collected.push_str(&piece),
                Some(Err(e)) => panic!("generation error: {e}"),
                None => break,
            }
        }

        println!("generated: {collected:?}");
        let answer = collected.split("</think>").last().unwrap_or(&collected);
        assert!(!answer.trim().is_empty(), "model produced no output");
        let lower = answer.to_lowercase();
        assert!(
            !lower.contains("community guidelines")
                && !lower.contains("i can't generate")
                && !lower.contains("i operate in english")
                && !lower.contains("i only operate in english"),
            "model refused with a hallucinated restriction instead of attempting the task: {answer:?}"
        );
    }

    #[test]
    #[ignore = "needs a real GGUF file at DIMI_DEV_MODEL_PATH; loads a multi-GB model twice"]
    fn loading_a_second_engine_while_the_first_is_still_alive_does_not_fail_backend_init() {
        let model_path = std::env::var("DIMI_DEV_MODEL_PATH")
            .expect("set DIMI_DEV_MODEL_PATH to a local .gguf file to run this test");

        let first = LlamaCppInferenceEngine::new(
            std::path::PathBuf::from(&model_path),
            28_672,
            true,
            None,
        )
        .expect("first engine should load");

        let second =
            LlamaCppInferenceEngine::new(std::path::PathBuf::from(&model_path), 28_672, true, None)
                .expect("second engine should load while the first is still alive (this is exactly what a model switch does)");

        assert!(first.is_loaded());
        assert!(second.is_loaded());
    }
}
