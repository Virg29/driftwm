//! What a child spawned through `spawn_command` gets from the compositor: the
//! open-file limit driftwm started with, not the one it raised for itself, no
//! fd driftwm left open, and a session of its own.

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

#[test]
fn a_child_inherits_no_fd_the_compositor_left_open() {
    let file = std::fs::File::open("/dev/null").unwrap();
    // A dup drops CLOEXEC, like an fd a library opened without it.
    // SAFETY: duplicating a valid open fd.
    let leaky = unsafe { libc::dup(std::os::fd::AsRawFd::as_raw_fd(&file)) };
    assert!(leaky > 2);

    let fds = output_of("ls /proc/$$/fd");
    // SAFETY: closing the fd dup'd above, which nothing else owns.
    unsafe { libc::close(leaky) };
    assert!(
        !fds.lines().any(|fd| fd.trim() == leaky.to_string()),
        "fd {leaky} reached the child: {fds:?}"
    );
}

#[test]
fn a_child_leads_its_own_session() {
    let stat = output_of("cat /proc/$$/stat");
    // `pid (comm) state ppid pgrp session ...`; comm may hold spaces.
    let pid = stat.split_whitespace().next().unwrap();
    let after_comm = &stat[stat.rfind(')').unwrap() + 1..];
    let session = after_comm.split_whitespace().nth(3).unwrap();
    assert_eq!(session, pid, "the shell is its own session leader");
}
