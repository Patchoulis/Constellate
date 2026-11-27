//! k-means: papers (as points) put into k groups so that the sum of squared distances from each point to the
//! centre of its group, the objective, is small.
//!
//! Three stages, each deterministic given the seed:
//!
//! 1. **k-means++ seeding** (Arthur and Vassilvitskii, 2007): the first centre is a random point, each further one
//!    a point chosen with chance in proportion to its squared distance from the centres so far, so the centres
//!    start spread out.
//! 2. **Lloyd's rounds**: every point goes to its nearest centre, every centre moves to the mean of its points,
//!    until no point changes group. Done from [`RESTARTS`] seedings; the one with the lowest objective is kept.
//! 3. **Hartigan's moves** (Hartigan and Wong, 1979): Lloyd stops when every point is nearest its own centre, but
//!    moving a point also moves both centres, and it can pay to move a point that is not nearer another centre.
//!    Each point in turn goes to the group where the objective falls most, counting both centres' moves, until no
//!    move helps. This never makes the objective worse, and often finds a better grouping.

use ndarray::{Array1, Array2, ArrayView1, ArrayView2, Axis};
use rayon::prelude::*;

use super::random::Random;

/// How many seedings are tried; the best after Lloyd's rounds is kept.
pub const RESTARTS: usize = 4;
/// Lloyd's rounds stop here even if points still move; they settle in a few dozen.
const MOST_ROUNDS: usize = 300;
/// Hartigan's passes over all points stop here even if points still move.
const MOST_PASSES: usize = 100;
/// Hartigan's passes also stop after one that lowers the objective by less than this share of it. Papers that are
/// about equally far from several centres can be shuffled between them for a hundred passes, each a little better
/// than the last (12,487 random papers: 5e-4 in the first pass, 1e-5 by the tenth): a share this small changes no
/// community anyone would see, and each pass over 12,000 papers of 1,024 numbers is a fifth of a second.
const LEAST_PASS_GAIN: f64 = 1e-4;
/// A move is made only if it lowers the objective by more than this share of what the point costs where it is, so
/// that rounding cannot move a point back and forth.
const LEAST_GAIN: f64 = 1e-12;

/// Which group each point is in, and the sum of squared distances to the groups' centres.
#[derive(Clone, Debug, PartialEq)]
pub struct Clustering {
    pub labels: Vec<usize>,
    pub objective: f64,
}

/// The rows of `points` in `groups` groups, labelled `0..groups`, every group holding at least one point (when there
/// are at least as many points as groups; with fewer, there are only as many groups as points).
pub fn kmeans(points: ArrayView2<f32>, groups: usize, seed: u64) -> Clustering {
    let count = points.nrows();
    let groups = groups.min(count);
    if groups == 0 {
        return Clustering { labels: vec![0; count], objective: 0.0 };
    }
    let lengths: Array1<f32> = points.rows().into_iter().map(|row| row.dot(&row)).collect();
    let tried: Vec<Clustering> =
        (0..RESTARTS).into_par_iter().map(|restart| attempt(points, &lengths, groups, seed, restart)).collect();
    // the first of the best, in the order of the restarts: the same one whichever finished first
    let mut best: Option<Clustering> = None;
    for clustering in tried {
        if best.as_ref().is_none_or(|kept| clustering.objective < kept.objective) {
            best = Some(clustering);
        }
    }
    let start = best.unwrap_or_else(|| Clustering { labels: vec![0; count], objective: 0.0 });
    let labels = hartigan(points, start, groups);
    let objective = objective(points, &labels, &Groups::of(points, &labels, groups));
    Clustering { labels, objective }
}

/// One restart: its seeding, then Lloyd's rounds.
///
/// The restarts run side by side, yet each draws the numbers it would have drawn after the restarts before it,
/// so the answer is the one restarts made one after another give: a seeding draws exactly `groups` numbers (its
/// first centre and one for each further centre), and the generator can be moved on by any count of draws at
/// once ([`Random::skip`]). Nothing is shared between restarts but the points, which nobody writes.
fn attempt(points: ArrayView2<f32>, lengths: &Array1<f32>, groups: usize, seed: u64, restart: usize) -> Clustering {
    let mut random = Random::new(seed);
    random.skip((restart * groups) as u64);
    let centres = seed_centres(points, groups, &mut random);
    let labels = lloyd(points, lengths, centres);
    let objective = objective(points, &labels, &Groups::of(points, &labels, groups));
    Clustering { labels, objective }
}

/// `|point - centre|^2`, added up in 64 bits.
fn squared_distance(point: ArrayView1<f32>, centre: ArrayView1<f64>) -> f64 {
    point.iter().zip(centre).map(|(&x, &c)| (f64::from(x) - c).powi(2)).sum()
}

/// k-means++: `groups` starting centres, each further one chosen with chance in proportion to its squared
/// distance from the nearest centre so far.
fn seed_centres(points: ArrayView2<f32>, groups: usize, random: &mut Random) -> Array2<f32> {
    let count = points.nrows();
    let mut centres = Array2::<f32>::zeros((groups, points.ncols()));
    let mut closest = vec![f64::INFINITY; count];
    let mut chosen = random.below(count);
    for group in 0..groups {
        centres.row_mut(group).assign(&points.row(chosen));
        if group + 1 == groups {
            break;
        }
        let centre = points.row(chosen).mapv(f64::from);
        closest.par_iter_mut().enumerate().for_each(|(point, nearest)| {
            *nearest = nearest.min(squared_distance(points.row(point), centre.view()));
        });
        chosen = pick_by_weight(&closest, random);
    }
    centres
}

/// An index chosen with chance in proportion to its weight; uniformly when every weight is 0 (every point is
/// already a centre).
fn pick_by_weight(weights: &[f64], random: &mut Random) -> usize {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return random.below(weights.len());
    }
    let target = random.uniform() * total;
    let mut sum = 0.0;
    for (index, weight) in weights.iter().enumerate() {
        sum += weight;
        if sum > target {
            return index;
        }
    }
    // rounding left the target past the last sum: the last point that has any weight
    weights.iter().rposition(|weight| *weight > 0.0).unwrap_or(0)
}

/// Points whose nearest centres one thread finds together, in one matrix product.
const POINTS_PER_PIECE: usize = 256;

/// Each point's nearest centre and its squared distance: `|x|^2 - 2 x.c + |c|^2`, the middle term for a piece of
/// points and every centre in one matrix product, a piece per thread. A point's answer depends on its own row
/// alone, and the pieces are put back in order.
fn nearest_centres(points: ArrayView2<f32>, lengths: &Array1<f32>, centres: &Array2<f32>) -> Vec<(usize, f32)> {
    let centre_lengths: Vec<f32> = centres.rows().into_iter().map(|row| row.dot(&row)).collect();
    let pieces: Vec<Vec<(usize, f32)>> = points
        .axis_chunks_iter(Axis(0), POINTS_PER_PIECE)
        .into_par_iter()
        .zip(lengths.axis_chunks_iter(Axis(0), POINTS_PER_PIECE))
        .map(|(piece, lengths)| {
            let products = piece.dot(&centres.t());
            products
                .outer_iter()
                .zip(lengths)
                .map(|(row, length)| {
                    row.iter()
                        .zip(&centre_lengths)
                        .map(|(product, centre)| (length - 2.0 * product + centre).max(0.0))
                        .enumerate()
                        .fold(
                            (0, f32::INFINITY),
                            |best, (group, distance)| if distance < best.1 { (group, distance) } else { best },
                        )
                })
                .collect()
        })
        .collect();
    pieces.into_iter().flatten().collect()
}

/// Lloyd's rounds from the given centres, until no point changes its nearest centre. Returns each point's group.
///
/// Settling is judged on the nearest centres before empty groups are refilled: with fewer distinct points than
/// groups (duplicate papers), a refilled point is nearest its old group again every round, and judged after the
/// refill the rounds would never settle.
fn lloyd(points: ArrayView2<f32>, lengths: &Array1<f32>, mut centres: Array2<f32>) -> Vec<usize> {
    let groups = centres.nrows();
    let mut previous: Vec<usize> = Vec::new();
    let mut labels = Vec::new();
    for _ in 0..MOST_ROUNDS {
        let nearest = nearest_centres(points, lengths, &centres);
        let chosen: Vec<usize> = nearest.iter().map(|(group, _)| *group).collect();
        let settled = chosen == previous;
        labels = chosen.clone();
        refill_empty_groups(&mut labels, &nearest, groups);
        if settled {
            break;
        }
        previous = chosen;
        centres = Groups::of(points, &labels, groups).centres.mapv(|value| value as f32);
    }
    labels
}

/// A group that no point is nearest to gets the point farthest from its own centre (from a group that can spare
/// it), so that there are always `groups` groups.
fn refill_empty_groups(labels: &mut [usize], nearest: &[(usize, f32)], groups: usize) {
    let mut sizes = vec![0_usize; groups];
    for &label in labels.iter() {
        sizes[label] += 1;
    }
    let mut farthest_first: Vec<usize> = (0..labels.len()).collect();
    farthest_first.sort_by(|&a, &b| nearest[b].1.total_cmp(&nearest[a].1).then(a.cmp(&b)));
    let mut candidates = farthest_first.into_iter();
    for group in 0..groups {
        if sizes[group] > 0 {
            continue;
        }
        if let Some(point) = candidates.by_ref().find(|&point| sizes[labels[point]] > 1) {
            sizes[labels[point]] -= 1;
            labels[point] = group;
            sizes[group] = 1;
        }
    }
}

/// The sum of squared distances from each point to its group's centre.
fn objective(points: ArrayView2<f32>, labels: &[usize], groups: &Groups) -> f64 {
    labels
        .iter()
        .enumerate()
        .map(|(point, &group)| squared_distance(points.row(point), groups.centres.row(group)))
        .sum()
}

/// The groups' centres and sizes, in 64 bits so that moving one point at a time does not wear them down.
struct Groups {
    centres: Array2<f64>,
    sizes: Vec<usize>,
}

/// One point moved from its group to another, and by how much the objective falls.
struct Move {
    to: usize,
    gain: f64,
}

impl Groups {
    /// The mean and size of every group. A group with no points has its centre at 0.
    fn of(points: ArrayView2<f32>, labels: &[usize], groups: usize) -> Self {
        let mut centres = Array2::<f64>::zeros((groups, points.ncols()));
        let mut sizes = vec![0; groups];
        for (point, &group) in labels.iter().enumerate() {
            centres.row_mut(group).zip_mut_with(&points.row(point), |sum, &value| *sum += f64::from(value));
            sizes[group] += 1;
        }
        for (mut centre, &size) in centres.axis_iter_mut(Axis(0)).zip(&sizes) {
            if size > 0 {
                centre /= size as f64;
            }
        }
        Self { centres, sizes }
    }

    /// How much the objective falls when `point` leaves its `group` of `n` points: its own distance counts, and
    /// so does the centre moving away from it, together `n / (n - 1)` times its squared distance. A point alone in
    /// its group cannot leave (the group would vanish), so there is nothing to gain.
    fn saving_from_leaving(&self, point: ArrayView1<f32>, group: usize) -> Option<f64> {
        let size = self.sizes[group] as f64;
        (self.sizes[group] > 1).then(|| size / (size - 1.0) * squared_distance(point, self.centres.row(group)))
    }

    /// How much the objective rises when `point` joins `group` of `n` points: the centre moves toward it, so it
    /// costs `n / (n + 1)` times its squared distance.
    fn cost_of_joining(&self, point: ArrayView1<f32>, group: usize) -> f64 {
        let size = self.sizes[group] as f64;
        size / (size + 1.0) * squared_distance(point, self.centres.row(group))
    }

    /// The group `point` should move to from `own`, if any lowers the objective.
    fn best_move(&self, point: ArrayView1<f32>, own: usize) -> Option<Move> {
        let saving = self.saving_from_leaving(point, own)?;
        let cheapest = (0..self.sizes.len())
            .filter(|&group| group != own)
            .map(|group| Move { to: group, gain: saving - self.cost_of_joining(point, group) })
            .fold(None, |best: Option<Move>, candidate| match best {
                Some(kept) if kept.gain >= candidate.gain => Some(kept),
                _ => Some(candidate),
            })?;
        (cheapest.gain > LEAST_GAIN * saving).then_some(cheapest)
    }

    /// Move `point` from group `from` to group `to`, updating both centres and sizes exactly.
    fn move_point(&mut self, point: ArrayView1<f32>, from: usize, to: usize) {
        let (left, joined) = (self.sizes[from] as f64, self.sizes[to] as f64);
        self.centres.row_mut(from).zip_mut_with(&point, |centre, &value| {
            *centre = (left * *centre - f64::from(value)) / (left - 1.0);
        });
        self.centres.row_mut(to).zip_mut_with(&point, |centre, &value| {
            *centre = (joined * *centre + f64::from(value)) / (joined + 1.0);
        });
        self.sizes[from] -= 1;
        self.sizes[to] += 1;
    }
}

/// Hartigan's moves from a grouping: each point in turn moves where the objective falls most, until a whole pass
/// moves none, or lowers the objective by less than [`LEAST_PASS_GAIN`] of it.
fn hartigan(points: ArrayView2<f32>, start: Clustering, groups: usize) -> Vec<usize> {
    let Clustering { mut labels, mut objective } = start;
    let mut state = Groups::of(points, &labels, groups);
    for _ in 0..MOST_PASSES {
        let gained = hartigan_pass(points, &mut labels, &mut state);
        let settled = gained <= LEAST_PASS_GAIN * objective;
        objective -= gained;
        if settled {
            break;
        }
    }
    labels
}

/// One pass of Hartigan's moves over every point. Returns how much the objective fell.
fn hartigan_pass(points: ArrayView2<f32>, labels: &mut [usize], state: &mut Groups) -> f64 {
    let mut gained = 0.0;
    for (index, label) in labels.iter_mut().enumerate() {
        let point = points.row(index);
        if let Some(change) = state.best_move(point, *label) {
            state.move_point(point, *label, change.to);
            *label = change.to;
            gained += change.gain;
        }
    }
    gained
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Three well-separated groups of 40 points in 16 dimensions, and the group of each.
    pub(crate) fn blobs() -> (Array2<f32>, Vec<usize>) {
        let mut random = Random::new(0);
        let centres = Array2::from_shape_simple_fn((3, 16), || 4.0 * random.normal());
        let truth: Vec<usize> = (0..120).map(|point| point / 40).collect();
        let points = Array2::from_shape_fn((120, 16), |(point, axis)| centres[[truth[point], axis]] + random.normal());
        (points.mapv(|value| value as f32), truth)
    }

    /// Do two labellings put the same points together (whatever the groups are called)?
    pub(crate) fn same_grouping(a: &[usize], b: &[usize]) -> bool {
        a.len() == b.len() && (0..a.len()).all(|i| (0..a.len()).all(|j| (a[i] == a[j]) == (b[i] == b[j])))
    }

    #[test]
    fn planted_groups_are_found() {
        let (points, truth) = blobs();
        let found = kmeans(points.view(), 3, 0);
        assert!(same_grouping(&found.labels, &truth));
    }

    #[test]
    fn the_same_seed_gives_the_same_groups() {
        let (points, _) = blobs();
        assert_eq!(kmeans(points.view(), 5, 7), kmeans(points.view(), 5, 7));
    }

    /// Points enough for several pieces of [`POINTS_PER_PIECE`], in eight loose groups.
    fn many_points() -> Array2<f32> {
        let mut random = Random::new(3);
        let centres = Array2::from_shape_simple_fn((8, 24), || 2.0 * random.normal());
        Array2::from_shape_fn((3 * POINTS_PER_PIECE + 41, 24), |(point, axis)| {
            (centres[[point % 8, axis]] + random.normal()) as f32
        })
    }

    #[test]
    fn restarts_side_by_side_choose_what_restarts_one_after_another_chose() {
        let points = many_points();
        let lengths: Array1<f32> = points.rows().into_iter().map(|row| row.dot(&row)).collect();
        // the restarts as they were made before they ran side by side: one generator, drawn from in turn
        let mut random = Random::new(9);
        let one_after_another: Vec<Clustering> = (0..RESTARTS)
            .map(|_| {
                let centres = seed_centres(points.view(), 6, &mut random);
                let labels = lloyd(points.view(), &lengths, centres);
                let objective = objective(points.view(), &labels, &Groups::of(points.view(), &labels, 6));
                Clustering { labels, objective }
            })
            .collect();
        let side_by_side: Vec<Clustering> =
            (0..RESTARTS).map(|restart| attempt(points.view(), &lengths, 6, 9, restart)).collect();
        assert_eq!(side_by_side, one_after_another);
    }

    #[test]
    fn k_means_gives_the_same_groups_on_one_thread_and_on_many() {
        let points = many_points();
        let (one, many) = crate::threads::on_one_and_many(|| kmeans(points.view(), 8, 4));
        assert_eq!(one.labels, many.labels);
        assert_eq!(one.objective.to_bits(), many.objective.to_bits());
    }

    #[test]
    fn every_group_has_a_point_even_with_repeated_points() {
        // six copies of one point and one other: three groups must still all be used
        let points = Array2::from_shape_fn((7, 2), |(point, _)| if point == 6 { 5.0 } else { 1.0 });
        let found = kmeans(points.view(), 3, 0);
        let mut used = found.labels.clone();
        used.sort_unstable();
        used.dedup();
        assert_eq!(used, vec![0, 1, 2]);
    }

    #[test]
    fn more_groups_than_points_gives_one_group_per_point() {
        let points = ndarray::array![[0.0_f32, 0.0], [1.0, 1.0]];
        let found = kmeans(points.view(), 5, 0);
        assert_ne!(found.labels[0], found.labels[1]);
        assert_eq!(found.objective, 0.0);
        assert!(kmeans(Array2::<f32>::zeros((0, 2)).view(), 3, 0).labels.is_empty());
    }

    #[test]
    fn leaving_and_joining_count_both_centres_moving() {
        // groups {0, 2} (centre 1) and {10} (centre 10) on a line; the point at 2 is 1 from its centre, 8 from the other
        let points = ndarray::array![[0.0_f32], [2.0], [10.0]];
        let groups = Groups::of(points.view(), &[0, 0, 1], 2);
        let point = points.row(1);
        assert_eq!(groups.saving_from_leaving(point, 0), Some(2.0 / 1.0 * 1.0));
        assert_eq!(groups.cost_of_joining(point, 1), 1.0 / 2.0 * 64.0);
        assert!(groups.best_move(point, 0).is_none());
        // a point alone in its group stays
        assert_eq!(groups.saving_from_leaving(points.row(2), 1), None);
    }

    #[test]
    fn a_move_updates_both_centres_to_the_new_means() {
        let points = ndarray::array![[0.0_f32], [2.0], [4.0], [10.0]];
        let mut groups = Groups::of(points.view(), &[0, 0, 0, 1], 2);
        groups.move_point(points.row(2), 0, 1);
        assert_eq!(groups.centres, ndarray::array![[1.0], [7.0]]);
        assert_eq!(groups.sizes, vec![2, 2]);
    }

    #[test]
    fn hartigan_moves_a_point_lloyd_would_leave_and_lowers_the_objective() {
        // Groups {0, 2} (centre 1) and {3.2, 3.2, 3.2} (centre 3.2) on a line. Lloyd is settled: the point at 2 is 1
        // from its own centre and 1.2 from the other. Yet moving it lowers the objective from 2 to 1.08: leaving
        // saves 2/1 x 1, joining costs only 3/4 x 1.44.
        let points = ndarray::array![[0.0_f32], [2.0], [3.2], [3.2], [3.2]];
        let settled = vec![0, 0, 1, 1, 1];
        let lengths: Array1<f32> = points.rows().into_iter().map(|row| row.dot(&row)).collect();
        let centres = Groups::of(points.view(), &settled, 2).centres.mapv(|value| value as f32);
        assert_eq!(lloyd(points.view(), &lengths, centres), settled, "Lloyd leaves the point where it is");
        let before = objective(points.view(), &settled, &Groups::of(points.view(), &settled, 2));
        let moved = hartigan(points.view(), Clustering { labels: settled, objective: before }, 2);
        let after = objective(points.view(), &moved, &Groups::of(points.view(), &moved, 2));
        assert_eq!(moved, vec![0, 1, 1, 1, 1]);
        assert!((before - 2.0).abs() < 1e-6 && (after - 1.08).abs() < 1e-5, "{before} {after}");
    }

    #[test]
    fn picking_by_weight_never_picks_a_weight_of_zero() {
        let mut random = Random::new(5);
        for _ in 0..1_000 {
            let index = pick_by_weight(&[0.0, 1.0, 0.0, 3.0], &mut random);
            assert!(index == 1 || index == 3);
        }
        assert!(pick_by_weight(&[0.0, 0.0], &mut random) < 2);
    }
}
