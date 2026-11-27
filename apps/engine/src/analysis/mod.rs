//! What is worked out from the papers: the similarity graph of their embeddings and, from it, the communities;
//! the citation network of their reference lists.
//!
//! | module       | what lives there                                                              |
//! |--------------|-------------------------------------------------------------------------------|
//! | [`graph`]    | the similarity graph, its links, eigenvectors, k-means and the clustering into communities |
//! | [`citations`] | who cites whom, from the papers' own reference lists                         |
//! | [`reviews`]  | reviewers' ratings and words taken together: per paper, community and run     |

pub mod citations;
pub mod graph;
pub mod reviews;
