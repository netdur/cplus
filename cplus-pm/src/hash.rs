use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum HashError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    NonUtf8Path {
        path: PathBuf,
    },
}

pub fn source_tree_hash(root: impl AsRef<Path>) -> Result<String, HashError> {
    let root = root.as_ref();
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.relative.cmp(&b.relative));

    let mut sha = Sha256::new();
    for file in files {
        sha.update(b"file\0");
        sha.update(file.relative.as_bytes());
        sha.update(b"\0");
        let mut handle = fs::File::open(&file.absolute).map_err(|source| HashError::Io {
            path: file.absolute.clone(),
            source,
        })?;
        let mut bytes = Vec::new();
        handle
            .read_to_end(&mut bytes)
            .map_err(|source| HashError::Io {
                path: file.absolute.clone(),
                source,
            })?;
        sha.update(bytes.len().to_string().as_bytes());
        sha.update(b"\0");
        sha.update(&bytes);
        sha.update(b"\0");
    }

    Ok(format!("sha256:{}", hex(&sha.finish())))
}

#[derive(Debug)]
struct HashFile {
    absolute: PathBuf,
    relative: String,
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<HashFile>) -> Result<(), HashError> {
    let mut entries = fs::read_dir(dir)
        .map_err(|source| HashError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| HashError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| HashError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            if entry.file_name() == ".git" {
                continue;
            }
            collect_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative_path = path.strip_prefix(root).unwrap_or(&path);
            let relative = relative_path
                .to_str()
                .ok_or_else(|| HashError::NonUtf8Path { path: path.clone() })?
                .replace('\\', "/");
            files.push(HashFile {
                absolute: path,
                relative,
            });
        }
    }

    Ok(())
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

struct Sha256 {
    state: [u32; 8],
    len: u64,
    buffer: Vec<u8>,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            len: 0,
            buffer: Vec::with_capacity(64),
        }
    }

    fn update(&mut self, mut input: &[u8]) {
        self.len += input.len() as u64;

        if !self.buffer.is_empty() {
            let needed = 64 - self.buffer.len();
            let take = needed.min(input.len());
            self.buffer.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.buffer.len() == 64 {
                let block: [u8; 64] = self.buffer[..].try_into().unwrap();
                self.compress(&block);
                self.buffer.clear();
            }
        }

        while input.len() >= 64 {
            let block: [u8; 64] = input[..64].try_into().unwrap();
            self.compress(&block);
            input = &input[64..];
        }

        if !input.is_empty() {
            self.buffer.extend_from_slice(input);
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let bit_len = self.len * 8;
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0);
        }
        self.buffer.extend_from_slice(&bit_len.to_be_bytes());

        let blocks = std::mem::take(&mut self.buffer);
        for chunk in blocks.chunks_exact(64) {
            let block: [u8; 64] = chunk.try_into().unwrap();
            self.compress(&block);
        }

        let mut out = [0u8; 32];
        for (idx, word) in self.state.iter().enumerate() {
            out[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];

        let mut w = [0u32; 64];
        for (idx, chunk) in block.chunks_exact(4).take(16).enumerate() {
            w[idx] = u32::from_be_bytes(chunk.try_into().unwrap());
        }
        for idx in 16..64 {
            let s0 =
                w[idx - 15].rotate_right(7) ^ w[idx - 15].rotate_right(18) ^ (w[idx - 15] >> 3);
            let s1 = w[idx - 2].rotate_right(17) ^ w[idx - 2].rotate_right(19) ^ (w[idx - 2] >> 10);
            w[idx] = w[idx - 16]
                .wrapping_add(s0)
                .wrapping_add(w[idx - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for idx in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[idx])
                .wrapping_add(w[idx]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
}

impl fmt::Display for HashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HashError::Io { path, source } => {
                write!(f, "failed to hash {}: {source}", path.display())
            }
            HashError::NonUtf8Path { path } => {
                write!(f, "cannot hash non-UTF-8 path {}", path.display())
            }
        }
    }
}

impl std::error::Error for HashError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        let mut sha = Sha256::new();
        sha.update(b"abc");
        assert_eq!(
            hex(&sha.finish()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn source_hash_ignores_git_directory() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("pkg.toml"), "[package]\n").unwrap();
        fs::create_dir(temp.path().join(".git")).unwrap();
        fs::write(temp.path().join(".git/HEAD"), "ref: main\n").unwrap();

        let first = source_tree_hash(temp.path()).unwrap();
        fs::write(temp.path().join(".git/HEAD"), "changed\n").unwrap();
        let second = source_tree_hash(temp.path()).unwrap();

        assert_eq!(first, second);
    }
}
