//! A fake `gh` on PATH for the end-to-end tests: each test file passes its
//! own script, which keeps state in $GH_STATE and logs to $GH_STATE/log.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub struct FakeGh {
    pub state: PathBuf,
    path: String,
}

impl FakeGh {
    /// Install `script` as `gh` under `dir`.
    pub fn install(dir: &Path, script: &str) -> Self {
        let (bin, state) = (dir.join("bin"), dir.join("gh"));
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&state).unwrap();
        let gh = bin.join("gh");
        fs::write(&gh, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        FakeGh { state, path }
    }

    /// Run `cmd` with this `gh` first on PATH; echo its output for failing tests.
    pub fn run(&self, cmd: &mut Command) -> Output {
        let out = cmd.env("PATH", &self.path).env("GH_STATE", &self.state).output().unwrap();
        eprintln!(
            "--- {:?}\n{}{}",
            cmd.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    pub fn log(&self) -> String {
        fs::read_to_string(self.state.join("log")).unwrap_or_default()
    }
}
