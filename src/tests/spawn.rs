//! What a child spawned through `spawn_command` gets from the compositor: the
//! open-file limit driftwm started with, not the one it raised for itself.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::real::TempDir;

/// Run `cmd` through `spawn_command` and return what it printed.
fn output_of(cmd: &str) -> String {
    let dir = TempDir::new();
    let out = dir.path().join("out");
    let partial = dir.path().join("out.partial");
    crate::state::spawn_command(
        &format!(
            "{cmd} > '{}' && mv '{}' '{}'",
            partial.display(),
            partial.display(),
            out.display()
        ),
        &HashMap::new(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&out) {
            return text;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("`{cmd}` wrote nothing within 5s");
}

fn nofile() -> libc::rlimit {
    let mut rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit only writes the struct it is handed.
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rlim) },
        0
    );
    rlim
}

#[test]
fn a_child_gets_the_open_file_limit_driftwm_started_with() {
    // Start one under the hard limit, so the raise shows whatever this machine
    // hands the test.
    let hard = nofile().rlim_max;
    let started = hard - 1;
    let lowered = libc::rlimit {
        rlim_cur: started,
        rlim_max: hard,
    };
    // SAFETY: setrlimit only reads the struct it is handed.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lowered) }, 0);

    crate::state::raise_nofile_limit();
    assert_eq!(nofile().rlim_cur, hard, "driftwm runs at the hard limit");
    assert_eq!(
        output_of("ulimit -Sn").trim(),
        started.to_string(),
        "the child starts at the original soft limit"
    );
}
