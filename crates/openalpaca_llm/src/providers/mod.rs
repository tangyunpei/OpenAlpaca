#[cfg(any(feature = "anthropic", feature = "openai"))]
mod utf8;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "ollama")]
pub mod ollama;
