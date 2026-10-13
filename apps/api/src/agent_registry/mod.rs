//! The agent registry: many agents per workspace (a lead, system desks seeded from packs, custom
//! agents and one personal agent per member), each with versioned specs, routing and autonomy.

pub mod spec;
pub mod store;
