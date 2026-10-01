//! OMP CLI discovery and invocation: locating the executable on disk and
//! probing its version. Shared infrastructure lives in [`crate::platform`].

use crate::platform::{cli_command, home};
use std::{env, path::PathBuf};

pub fn omp_executable() -> Option<PathBuf> {
    omp_candidates().find(|program| program.is_file())
}

fn omp_candidates() -> impl Iterator<Item = PathBuf> {
    let executable = if cfg!(windows) { "omp.exe" } else { "omp" };
    // Finder does not inherit the user's interactive shell PATH.
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .chain(
            home()
                .ok()
                .into_iter()
                .flat_map(|home| [home.join(".bun/bin"), home.join(".local/bin")]),
        )
        .chain(
            ["/opt/homebrew/bin", "/usr/local/bin"]
                .into_iter()
                .map(PathBuf::from),
        )
        .map(move |directory| directory.join(executable))
}

pub fn omp_version() -> (bool, String) {
    let mut found = false;
    for program in omp_candidates() {
        if !program.is_file() {
            continue;
        }
        found = true;
        if let Ok(mut command) = cli_command(&program) {
            if let Ok(output) = command.arg("--version").output() {
                if output.status.success() {
                    return (
                        true,
                        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                    );
                }
            }
        }
    }
    (found, String::new())
}
