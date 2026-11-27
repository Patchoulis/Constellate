//! What the app asks about one paper that is not a feature of its own: its PDF, fetched when it is not kept
//! ([`pdf`]), and what its authors declare in it ([`declarations`]). The routes are in [`crate::api::papers`].
//!
//! | module           | what lives there                                                                 |
//! |------------------|----------------------------------------------------------------------------------|
//! | [`pdf`]          | a paper's PDF on demand: kept, or fetched politely and once, with its outcome     |
//! | [`declarations`] | the authors' declarations read from the PDF, kept by version and by the PDF's hash |

pub mod declarations;
pub mod pdf;
