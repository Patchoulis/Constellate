//! The library itself, whatever it keeps: where it lives, its schema, the plain files beside it, the one-off
//! import of the files-based library, and the few records about the app rather than about papers.
//!
//! | file              | what lives there                                                        |
//! |-------------------|-------------------------------------------------------------------------|
//! | [`place`]         | where the library lives on this computer when nobody says otherwise     |
//! | [`schema`]        | the numbered migrations, applied at open                                |
//! | [`store`]         | the few plain files that remain (a run's exported results)              |
//! | [`import`]        | the one-off move from the files-based library                           |
//! | [`preferences`]   | settings and exchange rates                                             |
//! | [`run_records`]   | what the list of runs keeps about each run between starts               |

pub mod import;
pub mod place;
pub mod preferences;
pub mod run_records;
pub mod schema;
pub mod store;
