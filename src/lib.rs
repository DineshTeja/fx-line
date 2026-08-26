pub mod agent;
#[cfg(target_os = "macos")]
mod capture;
pub mod cmux;
pub mod context;
pub mod fx;
pub mod intent;
pub mod output;
pub mod service;
pub mod wispr;

#[cfg(target_os = "macos")]
pub mod hotkey;
#[cfg(target_os = "macos")]
pub mod indicator;
