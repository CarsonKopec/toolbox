//! Relocation core.
//!
//! Envs are built with the sentinel `__TOOLBOX_PREFIX__` baked in wherever a
//! reference to the env's own root path is needed (shebangs, embedded path
//! constants, RPATH/RUNPATH strings, etc). Before the env can be used at a
//! given mount point, the sentinel — or, on subsequent moves, the previously
//! patched prefix — is rewritten to the current mount path.
//!
//! `scan_for_sentinel` produces a `RelocateIndex` describing every file that
//! contains the sentinel. This is run once at packaging time and saved into
//! `.toolbox/relocate.json` inside the env.
//!
//! `apply` reads that index and patches every file from the previous prefix
//! (the sentinel, or `.toolbox/last-prefix` if previously activated) to the
//! current prefix. Binary files are patched in-place, preserving total file
//! size by NUL-padding shorter replacements; it fails if (new_prefix + suffix)
//! would exceed the slot recorded at scan time.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const PREFIX_SENTINEL: &str = "__TOOLBOX_PREFIX__";
pub const RELOCATE_FILE: &str = ".toolbox/relocate.json";
pub const LAST_PREFIX_FILE: &str = ".toolbox/last-prefix";

/// Files larger than this are skipped at scan time to keep memory bounded.
const MAX_SCAN_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RelocateIndex {
    pub entries: Vec<RelocateEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelocateEntry {
    /// Path within the env, forward slashes, relative to env root.
    pub path: String,
    pub kind: FileKind,
    /// Binary slot positions. Empty for Text entries.
    #[serde(default)]
    pub binary_slots: Vec<BinarySlot>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BinarySlot {
    /// Byte offset where the sentinel begins in the file.
    pub offset: u64,
    /// Bytes from `offset` up to (but not including) the next NUL.
    /// (new_prefix.len() + suffix.len()) must be <= slot_len.
    pub slot_len: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Text,
    Binary,
}

impl RelocateIndex {
    pub fn load(env_root: &Path) -> Result<Self> {
        let p = env_root.join(RELOCATE_FILE);
        if !p.exists() {
            return Ok(Self::default());
        }
        let s = fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        serde_json::from_str(&s).with_context(|| format!("parsing {}", p.display()))
    }

    pub fn save(&self, env_root: &Path) -> Result<()> {
        let p = env_root.join(RELOCATE_FILE);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

pub fn last_prefix(env_root: &Path) -> Option<PathBuf> {
    fs::read_to_string(env_root.join(LAST_PREFIX_FILE))
        .ok()
        .map(|s| PathBuf::from(s.trim_end_matches(['\r', '\n']).trim()))
}

pub fn write_last_prefix(env_root: &Path, prefix: &Path) -> Result<()> {
    let p = env_root.join(LAST_PREFIX_FILE);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(p, prefix.to_string_lossy().as_bytes())?;
    Ok(())
}

/// Walk `root` and build a RelocateIndex by finding every file that contains
/// the sentinel.
pub fn scan_for_sentinel(root: &Path) -> Result<RelocateIndex> {
    let sentinel = PREFIX_SENTINEL.as_bytes();
    let mut entries: Vec<RelocateEntry> = Vec::new();

    for dent in walkdir::WalkDir::new(root) {
        let dent = dent.context("walking env tree")?;
        if !dent.file_type().is_file() {
            continue;
        }
        let abs = dent.path();
        let rel = abs
            .strip_prefix(root)
            .with_context(|| format!("strip_prefix on {}", abs.display()))?;
        // Don't index our own metadata.
        if rel.starts_with(".toolbox") {
            continue;
        }
        let md = match fs::metadata(abs) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if md.len() > MAX_SCAN_FILE_BYTES {
            continue;
        }
        let data = match fs::read(abs) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if !contains(&data, sentinel) {
            continue;
        }

        let kind = if is_binary_data(&data) {
            FileKind::Binary
        } else {
            FileKind::Text
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let entry = match kind {
            FileKind::Text => RelocateEntry {
                path: rel_str,
                kind,
                binary_slots: vec![],
            },
            FileKind::Binary => {
                let slots = find_all(&data, sentinel)
                    .map(|off| BinarySlot {
                        offset: off as u64,
                        slot_len: compute_slot_len(&data, off) as u32,
                    })
                    .collect();
                RelocateEntry {
                    path: rel_str,
                    kind,
                    binary_slots: slots,
                }
            }
        };
        entries.push(entry);
    }

    Ok(RelocateIndex { entries })
}

/// Patch every file in `index` so that references to the previous prefix
/// become `new_prefix`. The previous prefix comes from `.toolbox/last-prefix`
/// if it exists, otherwise the sentinel. Updates `last-prefix` on success.
pub fn apply(env_root: &Path, index: &RelocateIndex, new_prefix: &Path) -> Result<()> {
    let prev: String = match last_prefix(env_root) {
        Some(p) => p.to_string_lossy().into_owned(),
        None => PREFIX_SENTINEL.to_string(),
    };
    apply_with_prev(env_root, index, &prev, new_prefix)
}

/// Like `apply`, but the caller specifies the previous prefix explicitly
/// instead of reading `.toolbox/last-prefix`. Used after `install` to patch
/// freshly-extracted files (which still carry the sentinel) without
/// disturbing files that were already relocated.
pub fn apply_with_prev(
    env_root: &Path,
    index: &RelocateIndex,
    prev: &str,
    new_prefix: &Path,
) -> Result<()> {
    let new = new_prefix.to_string_lossy().into_owned();
    if prev == new {
        return Ok(());
    }
    let prev = prev.to_string();

    // Phase 1: validate every binary slot can fit the new prefix.
    // Done up-front so we don't half-patch and leave the env inconsistent.
    for entry in &index.entries {
        if entry.kind != FileKind::Binary {
            continue;
        }
        let rel_native = entry.path.replace('/', std::path::MAIN_SEPARATOR_STR);
        let abs = env_root.join(rel_native);
        validate_binary(&abs, &entry.binary_slots, prev.as_bytes(), new.as_bytes())
            .with_context(|| format!("validating binary file {}", entry.path))?;
    }

    // Phase 2: commit.
    for entry in &index.entries {
        let rel_native = entry.path.replace('/', std::path::MAIN_SEPARATOR_STR);
        let abs = env_root.join(rel_native);
        match entry.kind {
            FileKind::Text => apply_text(&abs, prev.as_bytes(), new.as_bytes())
                .with_context(|| format!("patching text file {}", entry.path))?,
            FileKind::Binary => {
                apply_binary(&abs, &entry.binary_slots, prev.as_bytes(), new.as_bytes())
                    .with_context(|| format!("patching binary file {}", entry.path))?
            }
        }
    }

    write_last_prefix(env_root, new_prefix)?;
    Ok(())
}

fn apply_text(path: &Path, prev: &[u8], new: &[u8]) -> Result<()> {
    let data = fs::read(path)?;
    let patched = replace_bytes(&data, prev, new);
    if patched != data {
        fs::write(path, patched)?;
    }
    Ok(())
}

fn validate_binary(path: &Path, slots: &[BinarySlot], prev: &[u8], new: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new().read(true).open(path)?;
    for slot in slots {
        let slot_len = slot.slot_len as usize;
        let mut buf = vec![0u8; slot_len];
        f.seek(SeekFrom::Start(slot.offset))?;
        f.read_exact(&mut buf)?;

        if !buf.starts_with(prev) {
            bail!(
                "slot at offset {} in {} no longer starts with the expected previous prefix — env state inconsistent",
                slot.offset,
                path.display()
            );
        }
        let suffix_full = &buf[prev.len()..];
        let suffix_real_len = suffix_full
            .iter()
            .rposition(|&b| b != 0)
            .map(|i| i + 1)
            .unwrap_or(0);
        let needed = new.len() + suffix_real_len;
        if needed > slot_len {
            bail!(
                "new prefix + suffix ({} bytes) exceeds slot of {} bytes at offset {} in {} — env not relocatable to this path",
                needed,
                slot_len,
                slot.offset,
                path.display()
            );
        }
    }
    Ok(())
}

fn apply_binary(path: &Path, slots: &[BinarySlot], prev: &[u8], new: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    for slot in slots {
        let slot_len = slot.slot_len as usize;
        let mut buf = vec![0u8; slot_len];
        f.seek(SeekFrom::Start(slot.offset))?;
        f.read_exact(&mut buf)?;
        // validate_binary already checked starts_with and slot fit.
        let suffix_full = &buf[prev.len()..];
        let suffix_real_len = suffix_full
            .iter()
            .rposition(|&b| b != 0)
            .map(|i| i + 1)
            .unwrap_or(0);
        let suffix_real = &suffix_full[..suffix_real_len];

        let mut out = Vec::with_capacity(slot_len);
        out.extend_from_slice(new);
        out.extend_from_slice(suffix_real);
        out.resize(slot_len, 0u8);

        f.seek(SeekFrom::Start(slot.offset))?;
        f.write_all(&out)?;
    }
    Ok(())
}

fn is_binary_data(data: &[u8]) -> bool {
    let n = data.len().min(8192);
    data[..n].contains(&0u8)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find_all(haystack, needle).next().is_some()
}

fn find_all<'a>(data: &'a [u8], pattern: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    let plen = pattern.len();
    let mut i = 0usize;
    std::iter::from_fn(move || {
        if plen == 0 {
            return None;
        }
        while i + plen <= data.len() {
            if &data[i..i + plen] == pattern {
                let hit = i;
                i += plen;
                return Some(hit);
            }
            i += 1;
        }
        None
    })
}

fn compute_slot_len(data: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < data.len() && data[end] != 0 {
        end += 1;
    }
    end - start
}

fn replace_bytes(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if i + needle.len() <= haystack.len() && &haystack[i..i + needle.len()] == needle {
            out.extend_from_slice(replacement);
            i += needle.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("script.sh");
        // Sentinel stands in for the full prefix including its leading separator,
        // so the source has no extra slash before the sentinel.
        fs::write(&f, "#!__TOOLBOX_PREFIX__/bin/sh\necho __TOOLBOX_PREFIX__\n").unwrap();

        let idx = scan_for_sentinel(d.path()).unwrap();
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(idx.entries[0].kind, FileKind::Text);

        apply(d.path(), &idx, Path::new("/new/place")).unwrap();
        assert_eq!(
            fs::read_to_string(&f).unwrap(),
            "#!/new/place/bin/sh\necho /new/place\n"
        );
        assert_eq!(last_prefix(d.path()).unwrap(), PathBuf::from("/new/place"));
    }

    #[test]
    fn text_round_trip_repeated_move() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.cfg");
        fs::write(&f, "path=__TOOLBOX_PREFIX__/data\n").unwrap();
        let idx = scan_for_sentinel(d.path()).unwrap();

        apply(d.path(), &idx, Path::new("/x")).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "path=/x/data\n");

        apply(d.path(), &idx, Path::new("/y/z")).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "path=/y/z/data\n");
    }

    #[test]
    fn binary_round_trip_preserves_size() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("bin/a.out");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        let mut data = vec![0u8; 16]; // pretend header
        data.extend_from_slice(b"__TOOLBOX_PREFIX__/lib/python"); // 18 + 11 = 29
        data.resize(data.len() + 35, 0u8); // pad slot to 64
        data.extend_from_slice(b"\xff\xff\xff\xff"); // tail sentinel
        fs::write(&f, &data).unwrap();

        let idx = scan_for_sentinel(d.path()).unwrap();
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(idx.entries[0].kind, FileKind::Binary);
        assert_eq!(idx.entries[0].binary_slots.len(), 1);
        assert_eq!(idx.entries[0].binary_slots[0].slot_len, 29);
        assert_eq!(idx.entries[0].binary_slots[0].offset, 16);

        apply(d.path(), &idx, Path::new("/x")).unwrap();
        let after = fs::read(&f).unwrap();
        assert_eq!(after.len(), data.len());
        let slot = &after[16..16 + 29];
        assert!(slot.starts_with(b"/x/lib/python"));
        assert!(slot[b"/x/lib/python".len()..].iter().all(|&b| b == 0));
        assert_eq!(&after[after.len() - 4..], b"\xff\xff\xff\xff");
    }

    #[test]
    fn binary_round_trip_repeated_move_through_shorter_then_longer() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("bin/a.out");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        let mut data = vec![0u8; 4];
        data.extend_from_slice(b"__TOOLBOX_PREFIX__/lib/x"); // 18+6 = 24
        data.resize(data.len() + 40, 0u8); // slot total = 64
        fs::write(&f, &data).unwrap();
        let original_len = data.len();

        let idx = scan_for_sentinel(d.path()).unwrap();
        // First move to a short path.
        apply(d.path(), &idx, Path::new("/a")).unwrap();
        let mid = fs::read(&f).unwrap();
        assert_eq!(mid.len(), original_len);
        assert!(mid[4..].starts_with(b"/a/lib/x"));

        // Now move to a longer path — within slot.
        apply(d.path(), &idx, Path::new("/much/longer/path")).unwrap();
        let after = fs::read(&f).unwrap();
        assert_eq!(after.len(), original_len);
        assert!(after[4..].starts_with(b"/much/longer/path/lib/x"));
    }

    #[test]
    fn binary_too_long_fails() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("bin/a.out");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        let mut data = vec![0u8; 4];
        data.extend_from_slice(b"__TOOLBOX_PREFIX__/x"); // slot_len = 20
        data.push(0);
        fs::write(&f, &data).unwrap();

        let idx = scan_for_sentinel(d.path()).unwrap();
        let err = apply(d.path(), &idx, Path::new("/way/too/long/replacement/path"));
        assert!(err.is_err());
    }

    #[test]
    fn skips_toolbox_metadata() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join(".toolbox")).unwrap();
        fs::write(d.path().join(".toolbox/foo"), b"__TOOLBOX_PREFIX__").unwrap();
        let idx = scan_for_sentinel(d.path()).unwrap();
        assert_eq!(idx.entries.len(), 0);
    }

    #[test]
    fn apply_is_idempotent_for_same_prefix() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.sh");
        fs::write(&f, "__TOOLBOX_PREFIX__/x\n").unwrap();
        let idx = scan_for_sentinel(d.path()).unwrap();
        apply(d.path(), &idx, Path::new("/p")).unwrap();
        let before = fs::read_to_string(&f).unwrap();
        apply(d.path(), &idx, Path::new("/p")).unwrap(); // no-op
        let after = fs::read_to_string(&f).unwrap();
        assert_eq!(before, after);
    }
}
