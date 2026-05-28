//! `git cat-file --batch` reader. Mirrors `vkquery.tags.TagReader`.
//!
//! Opens a single long-lived subprocess so reading hundreds of chapter blobs
//! per tag doesn't pay spawn cost.

use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::docs_source::DocsSource;

pub struct TagReader {
    repo: PathBuf,
    proc: Child,
    /// `Option` so `Drop` can close stdin before `wait()` to avoid a deadlock.
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl TagReader {
    pub fn open(source: &DocsSource) -> Result<Self> {
        source.ensure_cloned()?;
        let mut proc = Command::new("git")
            .arg("-C")
            .arg(&source.path)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning git cat-file --batch")?;
        let stdin = proc.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = BufReader::new(proc.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        Ok(Self { repo: source.path.clone(), proc, stdin: Some(stdin), stdout })
    }

    /// Read a single file's bytes at `ref:path`.
    pub fn read_blob(&mut self, refspec: &str, path: &str) -> Result<Vec<u8>> {
        let spec = format!("{refspec}:{path}");
        self.read_one(&spec)
    }

    fn read_one(&mut self, spec: &str) -> Result<Vec<u8>> {
        let stdin = self.stdin.as_mut().ok_or_else(|| anyhow!("stdin already closed"))?;
        stdin.write_all(spec.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        let mut header = String::new();
        let n = self.stdout.read_line(&mut header)?;
        if n == 0 {
            return Err(anyhow!("git cat-file closed unexpectedly while reading {spec:?}"));
        }
        let trimmed = header.trim_end_matches(['\r', '\n']);
        if trimmed.ends_with(" missing") {
            return Err(anyhow!("{spec} not present in repo"));
        }
        // Format: "<sha> <type> <size>"
        let parts: Vec<&str> = trimmed.split(' ').collect();
        if parts.len() == 3 && parts[1] == "blob" {
            let size: usize = parts[2]
                .parse()
                .with_context(|| format!("bad size in header {trimmed:?}"))?;
            let mut buf = vec![0u8; size];
            self.stdout.read_exact(&mut buf)?;
            // consume trailing newline after the object
            let mut nl = [0u8; 1];
            let _ = self.stdout.read(&mut nl);
            return Ok(buf);
        }
        Err(anyhow!("unexpected git cat-file header: {trimmed:?}"))
    }

    /// All file paths under any of `prefixes` at `ref` (recursive).
    pub fn list_paths(&self, refspec: &str, prefixes: &[&str]) -> Result<Vec<String>> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["ls-tree", "-r", "--name-only", refspec, "--"])
            .args(prefixes)
            .output()
            .context("git ls-tree")?;
        if !out.status.success() {
            return Err(anyhow!(
                "git ls-tree failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(String::from_utf8(out.stdout)?
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub fn list_adoc(&self, refspec: &str, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .list_paths(refspec, &[prefix])?
            .into_iter()
            .filter(|p| p.ends_with(".adoc"))
            .collect())
    }

    pub fn repo_path(&self) -> &Path {
        &self.repo
    }
}

impl Drop for TagReader {
    fn drop(&mut self) {
        // Closing stdin first lets cat-file exit cleanly; otherwise wait() blocks.
        drop(self.stdin.take());
        let _ = self.proc.wait();
    }
}
