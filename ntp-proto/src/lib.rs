#![forbid(unsafe_code)]

mod clock;
mod clock_select;
mod filter;
mod identifiers;
mod packet;
mod peer;
mod time_types;

pub use clock::NtpClock;
#[cfg(feature = "fuzz")]
pub use clock_select::fuzz_find_interval;
#[cfg(feature = "fuzz")]
pub use filter::fuzz_tuple_from_packet_default;
pub use identifiers::ReferenceId;
pub use packet::NtpHeader;
#[cfg(feature = "fuzz")]
pub use time_types::fuzz_duration_from_seconds;
pub use time_types::{NtpDuration, NtpTimestamp};
