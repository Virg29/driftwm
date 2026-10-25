//! How a canvas screenshot reaches its target path: through a fresh file
//! beside it, renamed into place, so a symlink at the target is replaced
//! rather than written through, a failed write leaves the target as it was,
//! and no temp file outlives the capture.
//!
//! `#[ignore]`d like the other GL scenarios; each takes [`gl::lock`] first.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use smithay::utils::{Logical, Rectangle};

use super::real::TempDir;
use super::{Fixture, gl};

fn capture_to(f: &mut Fixture, path: &Path) -> Result<(), String> {
    let mut backend = f
        .state()
        .backend
        .take()
        .expect("the fixture has a renderer");
    let region = Rectangle::<i32, Logical>::new((0, 0).into(), (32, 16).into());
    let result = crate::render::capture_region_to_png(
        f.state(),
        backend.renderer(),
        region,
        1.0,
        true,
        None,
        path,
    );
    f.state().backend = Some(backend);
    result.map(|_| ())
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn setup(test: &str) -> Option<Fixture> {
    let renderer = gl::surfaceless_renderer(test)?;
    let mut f = Fixture::with_config(super::config("[background]\ntype = \"none\"\n"));
    f.add_output(1, (640, 400));
    gl::install(&mut f, renderer);
    Some(f)
}

#[test]
#[ignore = "needs Mesa surfaceless EGL; run with --include-ignored"]
fn a_symlink_at_the_target_is_replaced_not_written_through() {
    let _gl = gl::lock();
    let Some(mut f) = setup("screenshot over a symlink") else {
        return;
    };
    let temp = TempDir::new();
    let elsewhere = temp.path().join("elsewhere.txt");
    std::fs::write(&elsewhere, "not a screenshot").unwrap();
    let target = temp.path().join("shot.png");
    std::os::unix::fs::symlink(&elsewhere, &target).unwrap();

    capture_to(&mut f, &target).expect("capture");

    assert_eq!(
        std::fs::read_to_string(&elsewhere).unwrap(),
        "not a screenshot"
    );
    assert!(std::fs::symlink_metadata(&target).unwrap().is_file());
    image::open(&target).expect("the target is the PNG");
    assert_eq!(
        entries(temp.path()),
        ["elsewhere.txt", "shot.png"],
        "no temp file left"
    );

    gl::uninstall(&mut f);
}

#[test]
#[ignore = "needs Mesa surfaceless EGL; run with --include-ignored"]
fn a_capture_that_cannot_be_written_leaves_the_target_as_it_was() {
    let _gl = gl::lock();
    let Some(mut f) = setup("screenshot into a read-only dir") else {
        return;
    };
    let temp = TempDir::new();
    let dir = temp.path().join("locked");
    std::fs::create_dir(&dir).unwrap();
    let target = dir.join("shot.png");
    std::fs::write(&target, "the previous shot").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();

    let result = capture_to(&mut f, &target);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(result.is_err(), "the capture reports it could not write");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "the previous shot"
    );
    assert_eq!(entries(&dir), ["shot.png"], "no temp file left");

    gl::uninstall(&mut f);
}

#[test]
#[ignore = "needs Mesa surfaceless EGL; run with --include-ignored"]
fn a_capture_that_cannot_take_the_targets_place_leaves_no_temp_file() {
    let _gl = gl::lock();
    let Some(mut f) = setup("screenshot onto a directory") else {
        return;
    };
    let temp = TempDir::new();
    // The write succeeds; the rename over a directory doesn't.
    let target = temp.path().join("shot.png");
    std::fs::create_dir(&target).unwrap();

    assert!(capture_to(&mut f, &target).is_err());
    assert_eq!(entries(temp.path()), ["shot.png"], "no temp file left");

    gl::uninstall(&mut f);
}
