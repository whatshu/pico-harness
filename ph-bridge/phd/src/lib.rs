//! Pico Harness daemon runtime. Only this workspace's hardware modules open
//! device handles; all external callers cross the authenticated socket seam.
pub mod credentials;
pub mod runtime;
pub mod server;
mod session;
mod tasks;
