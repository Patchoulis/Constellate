//! What is worked out from the papers: the similarity graph of their embeddings and, from it, the communities;
//! the citation network of their reference lists.
//!
//! | module       | what lives there                                                              |
//! |--------------|-------------------------------------------------------------------------------|
//! | [`citations`] | who cites whom, from the papers' own reference lists                         |
//! | [`graph`]    | the similarity graph itself, its degrees and modularity                       |
//! | [`links`]    | each paper's closest papers: the links drawn, and the nearest-neighbour graph |
//! | [`eigen`]    | eigenvectors of symmetric matrices, small and dense or large and matrix-free  |
//! | [`spectral`] | the normalised graph's leading eigenvectors, and the Fiedler cut of one group |
//! | [`kmeans`]   | k-means++ seeding, Lloyd's rounds and Hartigan's moves                        |
//! | [`cluster`]  | the clustering methods and the tree of sub-communities                        |
//! | [`random`]   | the seeded random generator they share                                        |
//! | [`reviews`]  | reviewers' ratings and words taken together: per paper, community and run     |

pub mod citations;
pub mod cluster;
pub mod eigen;
pub mod graph;
pub mod kmeans;
pub mod links;
pub mod random;
pub mod reviews;
pub mod spectral;
pub mod stances;
