//! Marketplace for discovering and installing Emergent primitives.
//!
//! The marketplace allows users to:
//! - Browse available primitives (sources, handlers, sinks)
//! - Install prebuilt binaries from GitHub Releases
//! - Manage installed primitives (update, remove)
//! - Search for primitives by name, description, or tags
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                   Marketplace CLI                    │
//! │  ┌─────────┐  ┌──────────┐  ┌───────────────────┐  │
//! │  │  List   │  │ Install  │  │   Search/Info     │  │
//! │  └─────────┘  └──────────┘  └───────────────────┘  │
//! └─────────────────────────────────────────────────────┘
//!         │                 │                 │
//!         ▼                 ▼                 ▼
//!    ┌──────────────────────────────────────────┐
//!    │             Registry (Git)               │
//!    │    index.toml + primitive manifests      │
//!    └──────────────────────────────────────────┘
//!         │
//!         ▼
//!    ┌──────────────────────────────────────────┐
//!    │      GitHub Releases (Binaries)          │
//!    │   tar.gz (Unix) / zip (Windows)          │
//!    └──────────────────────────────────────────┘
//!         │
//!         ▼
//!    ┌──────────────────────────────────────────┐
//!    │         XDG Directories                  │
//!    │  Config | Cache | Data (primitives/bin)  │
//!    └──────────────────────────────────────────┘
//! ```
//!
//! # Example Usage
//!
//! ```bash
//! # List available primitives
//! emergent marketplace list
//!
//! # Install a primitive
//! emergent marketplace install slack-source
//!
//! # Show info about a primitive
//! emergent marketplace info slack-source
//!
//! # Search for primitives
//! emergent marketplace search slack
//!
//! # Update all installed primitives
//! emergent marketplace update
//! ```

pub mod cli;
pub mod error;
pub mod installer;
pub mod platform;
pub mod registry;
pub mod storage;

pub use cli::{execute, MarketplaceArgs, MarketplaceCommand};
pub use error::{MarketplaceError, Result};
pub use installer::{InstallOptions, InstallResult, Installer};
pub use platform::TargetPlatform;
pub use registry::{
    ArgumentInfo, BinaryInfo, MessageInfo, PrimitiveEntry, PrimitiveInfo, PrimitiveManifest,
    Registry, RegistryInfo, RegistryMetadata,
};
pub use storage::{InstalledPrimitive, InstallationManifest, MarketplaceConfig, MarketplaceStorage};
