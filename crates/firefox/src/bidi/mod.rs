//! WebDriver BiDi transport + high-level session (vendored from
//! agent-browser-firefox).

pub mod client;
pub mod session;

pub use client::BidiClient;
pub use session::BidiSession;
