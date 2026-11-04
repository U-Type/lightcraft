use super::*;
fn close(a: Point, b: Point) {
    assert!((a[0] - b[0]).hypot(a[1] - b[1]) < 1e-7, "{a:?} != {b:?}");
}
#[test]
fn rotation_scale_different_aspects() {
    let p = [Pair::new([0.1, 0.2], [0.65, 0.25]), Pair::new([0.8, 0.6], [0.55, 0.6])];
    let t = fit(Model::Similarity, &p, 2., 1., 0.).unwrap();
    close(t.map([0.4, 0.4]).unwrap(), [0.6, 0.4]);
}
#[test]
fn weights_and_disabled() {
    let mut p = vec![Pair::new([0.1, 0.1], [0.2, 0.3]), Pair::new([0.8, 0.5], [0.9, 0.7]), Pair::new([0.3, 0.4], [0.1, 0.1])];
    p[2].enabled = false;
    let t = fit(Model::Translation, &p, 1., 1., 0.).unwrap();
    close(t.map([0.2, 0.2]).unwrap(), [0.3, 0.4]);
    assert_eq!(t.residuals[2], None);
    p[2].enabled = true;
    p[2].weight = 0.01;
    assert!((fit(Model::Translation, &p, 1., 1., 0.).unwrap().map([0.2, 0.2]).unwrap()[0] - 0.3).abs() < 0.002);
}
#[test]
fn affine_and_perspective() {
    let points = [[0.1, 0.1], [0.9, 0.1], [0.1, 0.9], [0.9, 0.9], [0.5, 0.4]];
    for model in [Model::Affine, Model::Perspective] {
        let map = |p: Point| {
            let d = if model == Model::Perspective { 1. + 0.1 * p[0] } else { 1. };
            [(0.7 * p[0] + 0.1 * p[1] + 0.05) / d, (0.05 * p[0] + 0.8 * p[1] + 0.04) / d]
        };
        let pairs: Vec<_> = points.iter().map(|&p| Pair::new(p, map(p))).collect();
        let t = fit(model, &pairs, 1., 1., 0.).unwrap();
        close(t.map([0.3, 0.7]).unwrap(), map([0.3, 0.7]));
    }
}
#[test]
fn local_warp_and_reflection() {
    let p = vec![
        Pair::new([0., 0.], [0., 0.]),
        Pair::new([1., 0.], [1., 0.]),
        Pair::new([0., 1.], [0., 1.]),
        Pair::new([1., 1.], [1., 1.]),
        Pair::new([0.5, 0.5], [0.52, 0.48]),
    ];
    let t = fit(Model::ThinPlate, &p, 1., 1., 0.).unwrap();
    for p in &p {
        close(t.map(p.reference).unwrap(), p.target);
    }
    let flipped: Vec<_> = p.iter().map(|p| Pair::new(p.reference, [1. - p.reference[0], p.reference[1]])).collect();
    assert!(fit(Model::Affine, &flipped, 1., 1., 0.).is_err());
}
#[test]
fn degenerate_and_invalid() {
    let p = vec![Pair::new([0.5, 0.5], [0.2, 0.2]); 4];
    for m in [Model::Similarity, Model::Affine, Model::Perspective, Model::ThinPlate] {
        assert!(fit(m, &p, 1., 1., 0.).is_err());
    }
    assert!(fit(Model::Translation, &[Pair::new([f64::NAN, 0.], [0., 0.])], 1., 1., 0.).is_err());
    assert!(fit(Model::Translation, &p, 0., 1., 0.).is_err());
}
#[test]
fn crop_and_pixel_centres() {
    let image = lightcraft_raster::Rgba8::from_fn(8, 6, |x, y| [(x * 30) as u8, (y * 40) as u8, 20, 255]);
    let id = Transform::identity(8. / 6.);
    assert_eq!(warp(&image, &id, [0., 0., 1., 1.], 8, 6).unwrap(), image);
    let t = fit(Model::Translation, &[Pair::new([0., 0.], [0.25, 0.])], 1., 1., 0.).unwrap();
    let c = common_crop(&[Transform::identity(1.), t.clone()]).unwrap();
    assert_eq!(c, [0., 0., 0.75, 1.]);
    assert!(warp(&image, &t, c, 6, 6).unwrap().data.iter().all(|p| p[3] == 255));
    assert!(warp(&image, &id, [0., 0., 1., 1.], usize::MAX, 2).is_err());
}
