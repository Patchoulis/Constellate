//! What a run makes and keeps: the terms that set its communities apart, the community records and the readable
//! report, what the graph view draws, and the run's folder of files; and how good a clustering is.
//!
//! | module          | what lives there                                                       |
//! |-----------------|------------------------------------------------------------------------|
//! | [`vocabulary`]  | the words of the papers, and the terms that set a group apart          |
//! | [`report`]      | the community records (`communities.json`) and the readable report     |
//! | [`drawing`]     | the papers and links of the graph view (`graph.json`)                  |
//! | [`files`]       | a run's folder and files, as the Python laid them out                  |
//! | [`agreement`]   | how good a clustering is, by scores that cover each other's blind spots |

pub mod agreement;
pub mod drawing;
pub mod files;
pub mod report;
pub mod vocabulary;
