//! The in-process Wayland server. The nested host presents both screens in one desktop window.

mod geometry;
mod handlers;
mod input;
mod keyboard;
mod nested;
mod render;
mod server;
mod touch;

pub use nested::run;
pub use server::Process;

#[cfg(test)]
mod tests;
