//! Ask (#195, plan-ai-assistant.md §12): answer questions about the
//! dashboard on screen from the panel results the asker may already see.
//!
//! - [`core`]: the tools without transport or HTTP — the catalog, the view
//!   message, `get_panel`'s checks, masking and caps, `answer` with its
//!   verified citations. Pure, so the builder (M2-5, through the wasm
//!   `AskSession`) runs exactly the code the server does.
//! - `server` (feature `auth`): the server's loop — the model calls, limits,
//!   conversations held on the server, the log. `POST /api/ask` (M2-4) and
//!   the static render server (M2-6) call [`Ask::ask`].

pub mod catalog;
pub mod cite;
pub mod core;
pub mod mask;
pub mod session;

#[cfg(feature = "auth")]
pub mod config;
#[cfg(feature = "auth")]
pub mod limits;
#[cfg(feature = "auth")]
mod server;
#[cfg(feature = "auth")]
pub mod store;

pub use self::core::*;
pub use catalog::Catalog;
pub use cite::Citation;
pub use mask::Mask;

#[cfg(feature = "auth")]
pub use config::AskConfig;
#[cfg(feature = "auth")]
pub use server::*;
