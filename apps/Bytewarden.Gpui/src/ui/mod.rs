//! Fluent 2 UI subsystem: tokens, spring physics, brand assets, reusable components,
//! custom titlebar, reorganized sidebar, command palette, vault list, and detail/editor views.

pub mod brand;
pub mod components;
pub mod detail_view;
pub mod palette;
pub mod shell;
pub mod spring;
pub mod state;
pub mod titlebar;
pub mod toast;
pub mod tokens;
pub mod vault_view;

pub use state::{SettingsTab, UiState};
