//! Custom Google Cast CASTV2 client (protobuf + TLS + mDNS / subnet scan).

pub mod channel;
pub mod client;
pub mod discovery;
pub mod proto;

pub use client::{CastDeviceInfo, CastService};
