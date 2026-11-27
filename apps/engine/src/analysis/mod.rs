//! What is worked out from the papers' embeddings: the similarity graph and, from it, the communities.
//!
//! | module       | what lives there                                                              |
//! |--------------|-------------------------------------------------------------------------------|
//! | [`graph`]    | the similarity graph itself, its degrees and modularity                       |
//! | [`links`]    | each paper's closest papers: the links drawn, and the nearest-neighbour graph |
//! | [`eigen`]    | eigenvectors of symmetric matrices, small and dense or large and matrix-free  |
//! | [`spectral`] | the normalised graph's leading eigenvectors, and the Fiedler cut of one group |
//! | [`random`]   | the seeded random generator they share                                        |

pub mod eigen;
pub mod graph;
pub mod links;
pub mod random;
pub mod spectral;
