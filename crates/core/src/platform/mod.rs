//! Everything that differs between operating systems: who owns a file, how a file or
//! directory is restricted to its owner, where the home directory is, and which system
//! paths the guards refuse. The rest of the crate goes through this module only, so a
//! new platform is one new file here.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(not(unix))]
compile_error!("passdrop supports macOS and Linux for now; Windows support is tracked in akcware/passdrop#3");
