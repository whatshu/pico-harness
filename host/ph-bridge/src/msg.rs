//! Generated protobuf types (package `picoharness.v1`), produced by build.rs
//! from `proto/ph.proto`. Cached for IDE / doc generation; the authoritative
//! source is `proto/ph.proto`.

include!(concat!(env!("OUT_DIR"), "/picoharness.v1.rs"));

// Re-export the message enum for convenient matching.
pub use ph_message::Body;