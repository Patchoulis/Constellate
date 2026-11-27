//! What asking the language model costs, kept so that nothing is paid for twice and no call paid for is lost.
//!
//! | file        | what lives there                                                             |
//! |-------------|------------------------------------------------------------------------------|
//! | [`answers`] | what the language model answered for a feature, so none is paid twice        |
//! | [`asking`]  | what is about to be asked, so that a call paid for is never lost unseen      |
//! | [`ledger`]  | one row per call to the language model; totals are queries                   |

pub mod answers;
pub mod asking;
pub mod ledger;
