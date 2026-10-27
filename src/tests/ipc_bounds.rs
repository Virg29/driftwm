//! IPC requests carrying coordinates or sizes the canvas math can't hold are
//! refused before they change anything, and the bookmark registry stays
//! bounded however it is filled: IPC, the `set-bookmark` action, or a restored
//! session.

use std::collections::BTreeMap;

use driftwm::config::Action;
use driftwm::session::{self, SessionEnvelope};

use super::real::TempDir;
use super::{Fixture, config, map_window, window_by_app_id, window_position};
use crate::ipc::dispatch;
use crate::ipc::protocol::{Request, ScreenshotTarget};

fn set_bookmark(f: &mut Fixture, name: &str, to: (f64, f64)) -> crate::ipc::protocol::Reply {
    dispatch(
        Request::Bookmark {
            name: Some(name.to_string()),
            to: Some(to),
            delete: false,
        },
        f.state(),
    )
}

fn region(x: i32, y: i32, w: i32, h: i32) -> ScreenshotTarget {
    ScreenshotTarget::Region {
        x,
        y,
        w,
        h,
        from_screen: false,
    }
}

#[test]
fn a_camera_target_past_the_canvas_range_is_refused() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let before = f.state().landing_view();

    // Finite, but far past where f64 still resolves a pixel.
    assert!(dispatch(Request::Camera(Some((1e17, 0.0))), f.state()).is_err());
    assert!(dispatch(Request::Camera(Some((0.0, -2e9))), f.state()).is_err());
    assert_eq!(f.state().landing_view(), before, "nothing moved");

    assert!(dispatch(Request::Camera(Some((1e9, -1e9))), f.state()).is_ok());
}

#[test]
fn a_bookmark_past_the_canvas_range_is_refused() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    assert!(set_bookmark(&mut f, "far", (1e17, 0.0)).is_err());
    assert!(!f.state().bookmarks.contains_key("far"));
    assert!(set_bookmark(&mut f, "edge", (-1e9, 1e9)).is_ok());
}

#[test]
fn a_move_past_the_coordinate_range_is_refused() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let client = f.add_client();
    map_window(&mut f, client, "app", (400, 300));
    let window = window_by_app_id(&mut f, "app").unwrap();
    let before = window_position(&mut f, &window);

    for to in [
        (i32::MAX, 0),
        (0, i32::MIN),
        (crate::state::POSITION_LIMIT + 1, 0),
    ] {
        let reply = dispatch(
            Request::Move {
                window: None,
                to: Some(to),
            },
            f.state(),
        );
        assert!(reply.is_err(), "a move to {to:?} is refused");
    }
    assert_eq!(
        window_position(&mut f, &window),
        before,
        "the window stayed put"
    );

    let reply = dispatch(
        Request::Move {
            window: None,
            to: Some((crate::state::POSITION_LIMIT, -crate::state::POSITION_LIMIT)),
        },
        f.state(),
    );
    assert!(reply.is_ok(), "the range's own edge is accepted");
}

#[test]
fn a_resize_past_the_size_limit_is_refused() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let client = f.add_client();
    map_window(&mut f, client, "app", (400, 300));

    for to in [(i32::MAX, 300), (400, 32768)] {
        let reply = dispatch(
            Request::Resize {
                window: None,
                to: Some(to),
            },
            f.state(),
        );
        assert!(reply.is_err(), "a resize to {to:?} is refused");
    }
    let reply = dispatch(
        Request::Resize {
            window: None,
            to: Some((32767, 300)),
        },
        f.state(),
    );
    assert!(reply.is_ok(), "the limit itself is accepted");
}

#[test]
fn a_screenshot_region_past_the_range_is_refused() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    for target in [
        region(i32::MAX, 0, 100, 100),
        region(0, i32::MIN, 100, 100),
        region(0, 0, 32768, 100),
        region(0, 0, 100, i32::MAX),
    ] {
        assert!(
            crate::ipc::resolve_screenshot_region(&target, f.state()).is_err(),
            "{target:?} is refused"
        );
    }
    assert!(
        crate::ipc::resolve_screenshot_region(
            &region(crate::state::POSITION_LIMIT, 0, 32767, 100),
            f.state()
        )
        .is_ok()
    );
}

#[test]
fn the_registry_takes_no_new_name_past_256_bookmarks() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let mut n = 0;
    while f.state().bookmarks.len() < 256 {
        assert!(set_bookmark(&mut f, &format!("b{n}"), (0.0, 0.0)).is_ok());
        n += 1;
    }

    assert!(set_bookmark(&mut f, "one-more", (0.0, 0.0)).is_err());
    f.state()
        .execute_action(&Action::SetBookmark("one-more".into()));
    assert!(!f.state().bookmarks.contains_key("one-more"));
    assert_eq!(f.state().bookmarks.len(), 256);

    // A name already there is an update, not a new entry.
    assert!(set_bookmark(&mut f, "b0", (777.0, 777.0)).is_ok());
    f.state().execute_action(&Action::SetBookmark("b0".into()));
    assert_ne!(f.state().bookmarks["b0"], [777.0, 777.0]);
}

#[test]
fn a_bookmark_name_longer_than_256_bytes_is_refused() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    // 129 two-byte characters: 129 chars, 258 bytes.
    let long = "é".repeat(129);

    assert!(set_bookmark(&mut f, &long, (0.0, 0.0)).is_err());
    f.state().execute_action(&Action::SetBookmark(long.clone()));
    assert!(!f.state().bookmarks.contains_key(&long));

    let longest = "a".repeat(256);
    assert!(set_bookmark(&mut f, &longest, (0.0, 0.0)).is_ok());
}

#[test]
fn a_restored_session_drops_bookmarks_out_of_bounds() {
    let tmp = TempDir::new();
    let path = tmp.path().join("session.json");
    let mut bookmarks = BTreeMap::from([
        ("kept".to_string(), [1.0, 2.0]),
        ("far".to_string(), [1e17, 0.0]),
        ("x".repeat(257), [0.0, 0.0]),
    ]);
    for n in 0..300 {
        bookmarks.insert(format!("n{n:03}"), [0.0, 0.0]);
    }
    let envelope = SessionEnvelope {
        version: session::VERSION,
        saved_at: 0,
        entries: Vec::new(),
        outputs: BTreeMap::new(),
        bookmarks,
    };
    session::write(&path, &envelope, false).unwrap();
    // JSON has no infinity, but serde reads an overflowing literal as one.
    let json = std::fs::read_to_string(&path)
        .unwrap()
        .replace("1e17", "1e999");
    std::fs::write(&path, json).unwrap();

    let mut f = Fixture::with_config(config("[session]\nrestore_bookmarks = true\n"));
    f.add_output(1, (1920, 1080));
    let seeds = f.state().bookmarks.len();
    f.state().session_store.path = Some(path);
    f.state().load_session();

    let bookmarks = &f.state().bookmarks;
    assert_eq!(bookmarks["kept"], [1.0, 2.0]);
    assert!(
        !bookmarks.contains_key("far"),
        "a non-finite point is dropped"
    );
    assert!(
        !bookmarks.keys().any(|k| k.len() > 256),
        "an overlong name is dropped"
    );
    assert!(
        bookmarks.len() <= seeds + 256,
        "at most 256 bookmarks restore, got {}",
        bookmarks.len() - seeds
    );
    assert!(
        f.state().session_store.durable_bookmarks.len() <= 256,
        "the carried-forward registry is bounded too"
    );
}
