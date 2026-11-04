use super::Result;
// Bounded systems (at most 131 rows) on normalized, aspect-correct coordinates.
pub(super) fn linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Result<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs())).ok_or("empty system")?;
        if a[pivot][col].abs() < 1e-10 {
            return Err("points are coincident, collinear or poorly distributed".into());
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let v = a[col][col];
        for k in col..n {
            a[col][k] /= v;
        }
        b[col] /= v;
        for row in 0..n {
            if row != col {
                let f = a[row][col];
                for k in col..n {
                    a[row][k] -= f * a[col][k];
                }
                b[row] -= f * b[col];
            }
        }
    }
    if b.iter().any(|v| !v.is_finite()) {
        return Err("unstable fit".into());
    }
    Ok(b)
}
pub(super) fn least_squares(a: Vec<Vec<f64>>, b: Vec<f64>, n: usize) -> Result<Vec<f64>> {
    let mut normal = vec![vec![0.; n]; n];
    let mut rhs = vec![0.; n];
    for (row, v) in a.iter().zip(b) {
        for i in 0..n {
            rhs[i] += row[i] * v;
            for j in 0..n {
                normal[i][j] += row[i] * row[j];
            }
        }
    }
    linear(normal, rhs)
}
