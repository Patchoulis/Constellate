//! The logistic regression the Python fitted with scikit-learn (`LogisticRegression(C=2.0, max_iter=2000,
//! class_weight="balanced")`: L2-regularised, the intercept not regularised, solved by L-BFGS), solved here by
//! L-BFGS of its own.
//!
//! The problem is the one scikit-learn solves: with labels `y` in {0, 1}, each class weighted `n / (2 n_class)`
//! (so the weights sum to `n`), find the weights `w` and intercept `b` that minimise
//!
//! ```text
//! (1/n) sum_i s_i [ ln(1 + e^z_i) - y_i z_i ]  +  |w|^2 / (2 C n),      z_i = x_i . w + b
//! ```
//!
//! It is strictly convex, so it has one minimum, and any solver that reaches it gives the same model. scikit-learn
//! stops once no component of the gradient is above 1e-4; this one goes on until none is above [`TOLERANCE`],
//! which leaves the weights within about 1e-6 of the minimum (the error is at most the gradient divided by the
//! problem's least curvature, `1/(C n)`). So the Rust classifier is the exact one, and the Python's was within
//! about 1e-3 of it in every score: `parity.rs` checks both against the Python's own numbers.
//!
//! Everything is summed in the documents' order on one thread, so a fit gives the same numbers every time,
//! whatever the engine's thread count.

use super::features::Row;

/// The version of the training: raised whenever the solver or the problem changes, which makes a kept
/// classifier stale.
pub const TRAINING: u32 = 1;
/// The inverse of the regularisation's strength, as the Python had it.
pub const C: f64 = 2.0;
/// The largest component of the gradient (of the objective above, scaled by `1/n`) at which the fit stops.
pub const TOLERANCE: f64 = 1e-10;
/// Iterations at most; a fit of the detector's size takes a few hundred.
const ITERATIONS: usize = 20_000;
/// Corrections remembered: scipy's default.
const MEMORY: usize = 10;

/// A fitted model.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub weights: Vec<f64>,
    pub intercept: f64,
    pub iterations: usize,
    /// The largest component of the gradient where it stopped.
    pub gradient: f64,
}

/// ln(1 + e^z), without overflow.
fn softplus(z: f64) -> f64 {
    if z > 0.0 { z + (-z).exp().ln_1p() } else { z.exp().ln_1p() }
}

fn sigmoid(z: f64) -> f64 {
    if z >= 0.0 {
        1.0 / (1.0 + (-z).exp())
    } else {
        let e = z.exp();
        e / (1.0 + e)
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The problem: rows, labels, the weight of each document.
struct Problem<'a> {
    rows: &'a [Row],
    labels: &'a [bool],
    sample: Vec<f64>,
    columns: usize,
    n: f64,
}

impl Problem<'_> {
    /// The objective and its gradient at `point` (the weights, then the intercept).
    fn evaluate(&self, point: &[f64], gradient: &mut [f64]) -> f64 {
        let (weights, intercept) = point.split_at(self.columns);
        let intercept = intercept[0];
        gradient.iter_mut().for_each(|value| *value = 0.0);
        let mut loss = 0.0;
        for ((row, &label), &weight) in self.rows.iter().zip(self.labels).zip(&self.sample) {
            let z = intercept + row.iter().map(|&(column, value)| weights[column as usize] * value).sum::<f64>();
            let y = if label { 1.0 } else { 0.0 };
            loss += weight * (softplus(z) - y * z);
            let residual = weight * (sigmoid(z) - y);
            for &(column, value) in row {
                gradient[column as usize] += residual * value;
            }
            gradient[self.columns] += residual;
        }
        let penalty = 1.0 / (C * self.n);
        let mut squares = 0.0;
        for (slot, &weight) in gradient[..self.columns].iter_mut().zip(weights) {
            *slot = *slot / self.n + penalty * weight;
            squares += weight * weight;
        }
        gradient[self.columns] /= self.n;
        loss / self.n + 0.5 * penalty * squares
    }
}

/// Fit the model to `rows` (of `columns` features) labelled machine (`true`) or human, each class weighted to
/// count as much as the other.
pub fn fit(rows: &[Row], labels: &[bool], columns: usize) -> Fitted {
    let n = rows.len() as f64;
    let machine = labels.iter().filter(|label| **label).count() as f64;
    let human = n - machine;
    let sample = labels.iter().map(|&label| n / (2.0 * if label { machine } else { human })).collect();
    let problem = Problem { rows, labels, sample, columns, n };
    let size = columns + 1;
    let mut point = vec![0.0; size];
    let mut gradient = vec![0.0; size];
    let mut value = problem.evaluate(&point, &mut gradient);
    let largest = |gradient: &[f64]| gradient.iter().fold(0.0f64, |most, value| most.max(value.abs()));
    let mut steps: Vec<(Vec<f64>, Vec<f64>, f64)> = Vec::with_capacity(MEMORY);
    let (mut next, mut next_gradient) = (vec![0.0; size], vec![0.0; size]);
    let mut iterations = 0;
    while iterations < ITERATIONS && largest(&gradient) > TOLERANCE {
        iterations += 1;
        // the two-loop recursion: the direction from the remembered corrections
        let mut direction: Vec<f64> = gradient.iter().map(|value| -value).collect();
        let mut alphas = Vec::with_capacity(steps.len());
        for (s, y, rho) in steps.iter().rev() {
            let alpha = rho * dot(s, &direction);
            direction.iter_mut().zip(y).for_each(|(d, y)| *d -= alpha * y);
            alphas.push(alpha);
        }
        let scale = steps
            .last()
            .map_or_else(|| 1.0 / largest(&gradient).max(f64::MIN_POSITIVE), |(s, y, _)| dot(s, y) / dot(y, y));
        direction.iter_mut().for_each(|d| *d *= scale);
        for ((s, y, rho), alpha) in steps.iter().zip(alphas.iter().rev()) {
            let beta = rho * dot(y, &direction);
            direction.iter_mut().zip(s).for_each(|(d, s)| *d += (alpha - beta) * s);
        }
        let mut slope = dot(&gradient, &direction);
        if slope >= 0.0 {
            // not a descent direction (rounding): start again from the gradient
            steps.clear();
            direction = gradient.iter().map(|value| -value).collect();
            slope = dot(&gradient, &direction);
        }
        // backtracking until the decrease is a fair share of what the slope promised (Armijo). Near the minimum the
        // decrease falls below what the objective's rounding can show; there the step is judged by the slope at the
        // point reached instead: it is taken when it went no further than the minimum along the direction (the
        // problem is convex, so the slope there is still downhill or level), as an exact line search would.
        let mut step = 1.0;
        let mut found = false;
        let resolution = 64.0 * f64::EPSILON * value.abs().max(1.0);
        for _ in 0..60 {
            next.iter_mut().zip(&point).zip(&direction).for_each(|((next, x), d)| *next = x + step * d);
            let tried = problem.evaluate(&next, &mut next_gradient);
            let armijo = tried <= value + 1e-4 * step * slope;
            let unresolved =
                (tried - value).abs() <= resolution && dot(&next_gradient, &direction) <= 0.5 * slope.abs();
            if armijo || unresolved {
                value = tried.min(value);
                found = true;
                break;
            }
            step *= 0.5;
        }
        if !found {
            break; // no decrease is left to find at this precision: as near the minimum as it gets
        }
        let s: Vec<f64> = next.iter().zip(&point).map(|(a, b)| a - b).collect();
        let y: Vec<f64> = next_gradient.iter().zip(&gradient).map(|(a, b)| a - b).collect();
        let curvature = dot(&s, &y);
        std::mem::swap(&mut point, &mut next);
        std::mem::swap(&mut gradient, &mut next_gradient);
        if curvature > 1e-300 {
            if steps.len() == MEMORY {
                steps.remove(0);
            }
            steps.push((s, y, 1.0 / curvature));
        }
    }
    let intercept = point[columns];
    point.truncate(columns);
    Fitted { weights: point, intercept, iterations, gradient: largest(&gradient) }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_fit_reaches_the_minimum_and_separates_what_can_be_separated() {
        // one feature for each class, and one both have
        let rows: Vec<Row> = (0..40)
            .map(|index| if index % 2 == 0 { vec![(0, 0.8), (2, 0.6)] } else { vec![(1, 0.8), (2, 0.6)] })
            .collect();
        let labels: Vec<bool> = (0..40).map(|index| index % 2 == 1).collect();
        let fitted = fit(&rows, &labels, 3);
        assert!(fitted.gradient <= TOLERANCE, "{fitted:?}");
        assert!(fitted.weights[1] > 0.0 && fitted.weights[0] < 0.0);
        assert!((fitted.weights[0] + fitted.weights[1]).abs() < 1e-8, "balanced classes, symmetric weights");
        assert_eq!(fit(&rows, &labels, 3), fitted, "the same numbers every time");
    }

    #[test]
    fn classes_of_different_sizes_count_alike() {
        // nine humans to one machine, all of them alike: balanced weights put the intercept at even odds
        let rows: Vec<Row> = (0..10).map(|_| vec![(0, 1.0)]).collect();
        let labels: Vec<bool> = (0..10).map(|index| index == 0).collect();
        let fitted = fit(&rows, &labels, 1);
        let z = fitted.weights[0] + fitted.intercept;
        assert!(z.abs() < 1e-8, "{fitted:?}");
    }
}
