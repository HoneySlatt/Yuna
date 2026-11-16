//! The kernel's DRM usage statistics (`Documentation/gpu/drm-usage-stats.rst`), read in the
//! `fdinfo` of each file a process holds on `/dev/dri/`: each engine's busy time, and the memory
//! of its buffers ([ADR-0030](../../../docs/adr/0030-a-frames-gpu-time-from-the-kernel.md)).
//!
//! The statistics belong to a DRM *client*, which the kernel names `drm-client-id`: two files of
//! one process, or one file shared by two processes, may be the same client. A client is counted
//! once across every process read, by its device and its id.

use std::collections::BTreeMap;

use crate::procfs::Proc;

/// What one `fdinfo` of a DRM file says.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrmInfo {
    /// The driver, such as `amdgpu`.
    pub driver: Option<String>,
    /// The device's PCI slot, `drm-pdev`.
    pub device: Option<String>,
    /// The client's id on its device.
    pub client: Option<String>,
    /// Each engine's busy time in nanoseconds, by its name: `gfx`, `compute`, `dec`, …
    pub engines: BTreeMap<String, u64>,
    /// Each memory key, without its `drm-` prefix, in KiB: `memory-vram`, `total-gtt`, …
    pub memory_kib: BTreeMap<String, u64>,
}

/// One DRM file of a process.
#[derive(Debug, Clone, PartialEq)]
pub struct DrmFile {
    /// The process holding it.
    pub pid: u32,
    /// Its descriptor there.
    pub fd: u32,
    /// The node it opened, such as `/dev/dri/renderD128`.
    pub node: String,
    /// Its `fdinfo`, as the kernel wrote it.
    pub raw: String,
    /// What the `fdinfo` says.
    pub info: DrmInfo,
}

impl DrmFile {
    /// The key a client is counted once by: its device and its id; a file without an id is its
    /// own client.
    pub fn client_key(&self) -> String {
        match (&self.info.device, &self.info.client) {
            (device, Some(client)) => format!("{}/{client}", device.as_deref().unwrap_or("?")),
            (_, None) => format!("pid {} fd {}", self.pid, self.fd),
        }
    }
}

/// Reads an `fdinfo`'s DRM keys.
pub fn parse_fdinfo(text: &str) -> DrmInfo {
    let mut info = DrmInfo::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key {
            "drm-driver" => info.driver = Some(value.to_owned()),
            "drm-pdev" => info.device = Some(value.to_owned()),
            "drm-client-id" => info.client = Some(value.to_owned()),
            _ => {
                if let Some(engine) = key.strip_prefix("drm-engine-") {
                    // `drm-engine-capacity-<engine>` is a count of engines, not a time.
                    if !engine.starts_with("capacity-")
                        && let Some(ns) = value.strip_suffix(" ns").and_then(|n| n.parse().ok())
                    {
                        info.engines.insert(engine.to_owned(), ns);
                    }
                } else if let Some(name) = key.strip_prefix("drm-")
                    && is_memory_key(name)
                    && let Some(kib) = parse_size_kib(value)
                {
                    info.memory_kib.insert(name.to_owned(), kib);
                }
            }
        }
    }
    info
}

/// Whether a key after `drm-` is one of the memory keys: `memory-`, `total-`, `shared-`,
/// `resident-`, `purgeable-` and `active-` followed by a region.
fn is_memory_key(name: &str) -> bool {
    [
        "memory-",
        "total-",
        "shared-",
        "resident-",
        "purgeable-",
        "active-",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

/// A size such as `1234 KiB`, `5 MiB` or `512` (bytes, without a unit), in KiB rounded down.
fn parse_size_kib(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let number: u64 = parts.next()?.parse().ok()?;
    let factor_bytes: u64 = match parts.next() {
        None => 1,
        Some("KiB") => 1 << 10,
        Some("MiB") => 1 << 20,
        Some("GiB") => 1 << 30,
        Some(_) => return None,
    };
    Some(number.saturating_mul(factor_bytes) / 1024)
}

/// Every DRM file of the processes `pids`, its `fdinfo` read.
pub fn drm_files(proc: &Proc, pids: &[u32]) -> Vec<DrmFile> {
    let mut files = Vec::new();
    for &pid in pids {
        for (fd, target) in proc.open_files(pid) {
            let node = target.to_string_lossy();
            if !node.starts_with("/dev/dri/") {
                continue;
            }
            let Some(raw) = proc.fdinfo(pid, fd) else {
                continue;
            };
            let info = parse_fdinfo(&raw);
            files.push(DrmFile {
                pid,
                fd,
                node: node.into_owned(),
                raw,
                info,
            });
        }
    }
    files
}

/// The files of `files` that are distinct clients: the first file of each client.
pub fn distinct_clients(files: &[DrmFile]) -> BTreeMap<String, &DrmFile> {
    let mut clients = BTreeMap::new();
    for file in files {
        clients.entry(file.client_key()).or_insert(file);
    }
    clients
}

/// Each engine's busy time of `files`, in nanoseconds, each client counted once.
pub fn engines(files: &[DrmFile]) -> BTreeMap<String, u64> {
    let mut sum = BTreeMap::new();
    for file in distinct_clients(files).values() {
        for (engine, ns) in &file.info.engines {
            *sum.entry(engine.clone()).or_default() += ns;
        }
    }
    sum
}

/// Each memory key of `files`, in KiB, each client counted once.
pub fn memory_kib(files: &[DrmFile]) -> BTreeMap<String, u64> {
    let mut sum = BTreeMap::new();
    for file in distinct_clients(files).values() {
        for (key, kib) in &file.info.memory_kib {
            *sum.entry(key.clone()).or_default() += kib;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMDGPU: &str = "pos:\t0\nflags:\t02100002\nmnt_id:\t26\nino:\t1068\n\
        drm-driver:\tamdgpu\ndrm-client-id:\t%ID%\ndrm-pdev:\t0000:0d:00.0\n\
        pasid:\t32787\ndrm-memory-vram:\t%VRAM% KiB\ndrm-memory-gtt:\t2048 KiB\n\
        drm-memory-cpu:\t0 KiB\namd-memory-visible-vram:\t1000 KiB\n\
        drm-engine-gfx:\t%GFX% ns\ndrm-engine-compute:\t500 ns\ndrm-engine-dec:\t0 ns\n\
        drm-engine-capacity-gfx:\t1\n";

    fn amdgpu(id: u32, gfx: u64, vram: u64) -> String {
        AMDGPU
            .replace("%ID%", &id.to_string())
            .replace("%GFX%", &gfx.to_string())
            .replace("%VRAM%", &vram.to_string())
    }

    fn file(pid: u32, fd: u32, raw: &str) -> DrmFile {
        DrmFile {
            pid,
            fd,
            node: "/dev/dri/renderD128".to_owned(),
            raw: raw.to_owned(),
            info: parse_fdinfo(raw),
        }
    }

    #[test]
    fn an_fdinfo_gives_its_client_its_engines_and_its_memory() {
        let info = parse_fdinfo(&amdgpu(42, 1_000_000, 4096));
        assert_eq!(info.driver.as_deref(), Some("amdgpu"));
        assert_eq!(info.device.as_deref(), Some("0000:0d:00.0"));
        assert_eq!(info.client.as_deref(), Some("42"));
        assert_eq!(
            info.engines,
            BTreeMap::from([
                ("compute".to_owned(), 500),
                ("dec".to_owned(), 0),
                ("gfx".to_owned(), 1_000_000),
            ])
        );
        assert_eq!(info.memory_kib["memory-vram"], 4096);
        assert_eq!(info.memory_kib["memory-gtt"], 2048);
        assert!(!info.memory_kib.contains_key("engine-capacity-gfx"));
        assert_eq!(parse_size_kib("3 MiB"), Some(3072));
        assert_eq!(parse_size_kib("2048"), Some(2));
        assert_eq!(parse_size_kib("12 parsecs"), None);
    }

    #[test]
    fn two_clients_in_two_processes_count_once_each() {
        let files = [
            file(100, 5, &amdgpu(1, 1_000, 10)),
            file(200, 9, &amdgpu(2, 3_000, 30)),
        ];
        assert_eq!(engines(&files)["gfx"], 4_000);
        assert_eq!(memory_kib(&files)["memory-vram"], 40);
        assert_eq!(distinct_clients(&files).len(), 2);
    }

    #[test]
    fn one_client_in_two_files_counts_once() {
        // The same client seen through two descriptors of one process, and through a descriptor
        // a child inherited.
        let raw = amdgpu(7, 2_000, 64);
        let files = [file(100, 5, &raw), file(100, 6, &raw), file(101, 5, &raw)];
        assert_eq!(engines(&files)["gfx"], 2_000);
        assert_eq!(engines(&files)["compute"], 500);
        assert_eq!(memory_kib(&files)["memory-vram"], 64);
        assert_eq!(distinct_clients(&files).len(), 1);
    }

    #[test]
    fn files_without_a_client_id_are_each_their_own_client() {
        let raw = "drm-driver:\ti915\ndrm-engine-render:\t100 ns\n";
        let files = [file(100, 5, raw), file(100, 6, raw)];
        assert_eq!(engines(&files)["render"], 200);
    }
}
