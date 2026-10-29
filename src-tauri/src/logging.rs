//! File logging with size-based rotation: `lumza.log` plus `lumza.1.log` … `lumza.4.log`,
//! 1 MB each (spec §6: 5 × 1 MB). Falls back to stderr when the folder is not writable.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

const MAX_BYTES: u64 = 1024 * 1024;
const FILES: usize = 5;

struct Rotating {
    dir: PathBuf,
    file: File,
    size: u64,
}

impl Rotating {
    fn path(dir: &std::path::Path, n: usize) -> PathBuf {
        if n == 0 {
            dir.join("lumza.log")
        } else {
            dir.join(format!("lumza.{n}.log"))
        }
    }

    fn open(dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let path = Self::path(&dir, 0);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self { dir, file, size })
    }

    fn rotate(&mut self) -> io::Result<()> {
        // lumza.4.log is dropped, the rest shift up by one, a fresh lumza.log starts.
        let _ = fs::remove_file(Self::path(&self.dir, FILES - 1));
        for n in (0..FILES - 1).rev() {
            let from = Self::path(&self.dir, n);
            if from.exists() {
                let _ = fs::rename(&from, Self::path(&self.dir, n + 1));
            }
        }
        self.file = OpenOptions::new().create(true).append(true).open(Self::path(&self.dir, 0))?;
        self.size = 0;
        Ok(())
    }
}

impl Write for Rotating {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size + buf.len() as u64 > MAX_BYTES {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[derive(Clone)]
struct Shared(Arc<Mutex<Rotating>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).flush()
    }
}

pub fn init(dir: PathBuf) {
    let builder = tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).with_target(false).with_ansi(false);
    match Rotating::open(dir) {
        Ok(r) => {
            let shared = Shared(Arc::new(Mutex::new(r)));
            builder.with_writer(move || shared.clone()).init();
        }
        Err(_) => builder.init(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_and_keeps_five_files() {
        let dir = std::env::temp_dir().join(format!("lumza-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut r = Rotating::open(dir.clone()).unwrap();
        let chunk = vec![b'x'; 600 * 1024];
        for _ in 0..12 {
            r.write_all(&chunk).unwrap();
        }
        let names: Vec<String> = {
            let mut v: Vec<_> =
                fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        };
        assert_eq!(names, ["lumza.1.log", "lumza.2.log", "lumza.3.log", "lumza.4.log", "lumza.log"]);
        for n in &names {
            assert!(fs::metadata(dir.join(n)).unwrap().len() <= MAX_BYTES);
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
