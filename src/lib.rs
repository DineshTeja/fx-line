pub mod agent;
mod cmux;
pub mod line;
mod model;
#[cfg(target_os = "macos")]
mod platform;
mod project;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
