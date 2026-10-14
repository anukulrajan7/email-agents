//! Learning-lift eval: re-draft recent edited cases with and without the learned preferences and
//! report how much closer the preferences get to the final human text. It needs the learning loop's
//! preference store, which has not landed, so a run ends as `error` with a plain reason.

pub const UNAVAILABLE: &str = "Learning lift needs learned preferences, which this workspace does not have yet.";
