//! What stands between a request and the routes: how the engine listens, the limits on connections, the checks
//! every request passes, and, across a network, the owner's sign-in.
//!
//! | module          | what lives there                                                                   |
//! |-----------------|------------------------------------------------------------------------------------|
//! | [`config`]      | how the engine listens, from the environment: this computer only, or a network     |
//! | [`connections`] | how many connections there may be, and how long one may stay silent                |
//! | [`security`]    | the checks every request passes (address, origin, token or session) and its headers |
//! | [`login`]       | across a network: the owner's sign-in, with Google or a password, and sessions     |

pub mod config;
pub mod connections;
pub mod login;
pub mod security;
