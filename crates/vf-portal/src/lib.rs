//! The firm's own Vibe Portal (PA-37): a client file portal on the firm's appliance, reached with
//! one firm key. Vibe Files shares cabinet documents with a client's portal users, invites them,
//! answers their access requests, and fetches what they upload into a Portal inbox for Intake.
//!
//! The portal address and key are kept in the cabinet (`_Vibe/keys/portal.json`, never synced)
//! so every PC that opens the cabinet uses them, as the shared sync settings are. Calls go only
//! to that address; nothing here runs until the firm sets one.

pub mod api;
pub mod filed;
pub mod inbox;
pub mod settings;

pub use api::{Portal, PortalError};
pub use settings::PortalSettings;
