//! The agent registry: many agents per workspace (a lead, system desks seeded from packs, custom
//! agents and one personal agent per member), each with versioned specs, routing and autonomy.

pub mod routing;
pub mod runtime;
pub mod seed;
pub mod spec;
pub mod store;
pub mod templates;
pub mod versions;

/// Observing agents may start at most this many shadow cases per workspace per UTC day.
pub const SHADOW_MAX_PER_DAY: i64 = 200;
/// A shadow case takes at most this many planner turns.
pub const SHADOW_MAX_STEPS: i64 = 4;
