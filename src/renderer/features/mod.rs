//! Vertical renderer feature boundaries.
//!
//! These modules are the target home for renderer work that is currently split
//! across facade/system/routing/context layers. New renderer behavior should be
//! placed under a feature module first; shared state belongs in `document`,
//! `gpu`, or `mutation` only when multiple features genuinely need it.

pub mod apply;
pub(crate) mod brush;
pub(crate) mod color_sampler;
pub mod composite;
pub mod filter;
pub mod selection;
pub(crate) mod tool_preview;
pub mod view;

pub(crate) mod transform;
